use crate::{
    files::{self, UploadGuard},
    mobile, network,
    state::State as App,
    updates, upgrade,
};
use anyhow::Context;
use axum::{
    Extension, Json, Router,
    body::Body,
    extract::{
        DefaultBodyLimit, Multipart, Path, Query, Request, State, WebSocketUpgrade,
        ws::Message as WsMessage,
    },
    http::{HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use lanlink_protocol::*;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tower_http::services::ServeDir;
use uuid::Uuid;

macro_rules! ensure { ($condition:expr, $($message:tt)*) => { if !$condition { return Err(ApiError(anyhow::anyhow!($($message)*))); } }; }

pub struct ApiError(anyhow::Error);
impl<E: Into<anyhow::Error>> From<E> for ApiError {
    fn from(e: E) -> Self {
        Self(e.into())
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        tracing::debug!(error=?self.0, "API request failed");
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": self.0.to_string()})),
        )
            .into_response()
    }
}
type Api<T> = std::result::Result<Json<T>, ApiError>;
fn ok() -> Json<Value> {
    Json(json!({"ok": true}))
}
pub fn router(state: Arc<App>, web_dir: std::path::PathBuf, updater: upgrade::Updater) -> Router {
    Router::new()
        .route("/api/v1/health", get(health))
        .route("/api/v1/updates/check", post(check_update))
        .route(
            "/api/v1/updates/apply",
            post(apply_update).layer(DefaultBodyLimit::max(1024)),
        )
        .route("/api/v1/session", get(session))
        .route("/api/v1/device", get(device).patch(rename))
        .route("/api/v1/devices", get(peers))
        .route("/api/v1/devices/{id}/pair", post(pair).delete(revoke))
        .route("/api/v1/pairings", get(pairings))
        .route("/api/v1/pairings/{id}/confirm", post(confirm))
        .route("/api/v1/messages", get(messages).post(send_message))
        .route("/api/v1/messages/{id}/retry", post(retry_message))
        .route("/api/v1/transfers", get(transfers).post(upload))
        .route("/api/v1/transfers/{id}/{action}", post(transfer_action))
        .route("/api/v1/transfers/{id}/download", get(download))
        .route("/api/v1/settings", get(settings).patch(update_settings))
        .route("/api/v1/ws", get(events))
        .merge(mobile::desktop_router(web_dir.clone()))
        .fallback_service(ServeDir::new(web_dir).append_index_html_on_directories(true))
        .layer(DefaultBodyLimit::max(
            (MAX_FILE_SIZE + 1024 * 1024) as usize,
        ))
        .layer(Extension(updater))
        .layer(middleware::from_fn_with_state(state.clone(), protect))
        .with_state(state)
}
async fn health(State(state): State<Arc<App>>) -> Json<Value> {
    let mut health =
        json!({"service":"lanlink","status":"ok","version":updates::INSTALLED_VERSION});
    if let Some(version) = upgrade::failure(&state.root) {
        health["update_failed_version"] = json!(version);
    }
    Json(health)
}

async fn check_update(Extension(updater): Extension<upgrade::Updater>) -> Response {
    match tokio::task::spawn_blocking(move || updater.check()).await {
        Ok(Ok(update)) => Json(update).into_response(),
        result => {
            tracing::debug!(?result, "Update check failed");
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({"error":"无法检查更新，请确认网络连接后重试。"})),
            )
                .into_response()
        }
    }
}

#[derive(Deserialize)]
struct ApplyUpdate {
    version: String,
}

async fn apply_update(
    State(state): State<Arc<App>>,
    Extension(updater): Extension<upgrade::Updater>,
    Json(input): Json<ApplyUpdate>,
) -> Response {
    let root = state.root.clone();
    let origin = state.origin.clone();
    let result = tokio::task::spawn_blocking(move || {
        updater.prepare(&input.version, root, origin)?;
        Ok::<_, anyhow::Error>(input.version)
    })
    .await;
    match result {
        Ok(Ok(version)) => Json(json!({"version":version})).into_response(),
        result => {
            tracing::warn!(?result, "Could not prepare update");
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({"error":"暂时无法安装更新，当前版本可以继续使用。请稍后重试。"})),
            )
                .into_response()
        }
    }
}

async fn protect(State(state): State<Arc<App>>, request: Request, next: Next) -> Response {
    let deny = || {
        (
            StatusCode::FORBIDDEN,
            Json(json!({"error":"local origin or authentication rejected"})),
        )
            .into_response()
    };
    let headers = request.headers();
    if headers.get(header::HOST).and_then(|v| v.to_str().ok()) != Some(&state.host) {
        return deny();
    }
    if let Some(origin) = headers.get(header::ORIGIN)
        && origin.to_str().ok() != Some(&state.origin)
    {
        return deny();
    }
    if request.uri().path() == "/api/v1/session"
        && headers.get("sec-fetch-site").and_then(|v| v.to_str().ok()) != Some("same-origin")
    {
        return deny();
    }
    if !matches!(
        *request.method(),
        axum::http::Method::GET | axum::http::Method::HEAD
    ) && (headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) != Some(&state.origin)
        || headers.get("x-lanlink-token").and_then(|v| v.to_str().ok()) != Some(&state.token))
    {
        return deny();
    }
    let mut response = next.run(request).await;
    if (response.status().is_client_error() || response.status().is_server_error())
        && response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_none_or(|v| !v.starts_with("application/json"))
    {
        response = (response.status(), Json(json!({"error":"request rejected"}))).into_response();
    }
    let headers = response.headers_mut();
    headers.insert("content-security-policy", HeaderValue::from_static("default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self'; object-src 'none'; frame-ancestors 'none'; base-uri 'none'"));
    headers.insert("x-frame-options", HeaderValue::from_static("DENY"));
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}
async fn session(State(s): State<Arc<App>>) -> Json<Value> {
    Json(json!({"token":s.token}))
}
async fn device(State(s): State<Arc<App>>) -> Json<Device> {
    Json(s.device.read().await.clone())
}
#[derive(Deserialize)]
struct Rename {
    name: String,
}
async fn rename(State(s): State<Arc<App>>, Json(input): Json<Rename>) -> Api<Device> {
    let name = input.name.trim();
    ensure!(
        !name.is_empty() && name.chars().count() <= 80 && !name.chars().any(char::is_control),
        "name must contain 1 to 80 characters"
    );
    let mut device = s.device.write().await;
    device.name = name.into();
    s.store.put("device", &*device).await?;
    s.changed();
    Ok(Json(device.clone()))
}
async fn peers(State(s): State<Arc<App>>) -> Api<Vec<Peer>> {
    Ok(Json(s.peers().await?))
}
async fn pair(State(s): State<Arc<App>>, Path(id): Path<String>) -> Api<Value> {
    ensure!(valid_id(&id), "invalid device ID");
    ensure!(
        !s.active.lock().await.contains_key(&id),
        "device is already connected"
    );
    ensure!(
        s.observations.read().await.contains_key(&id),
        "device address is unavailable"
    );
    ensure!(
        !s.dialing.lock().await.contains_key(&id),
        "connection attempt already in progress"
    );
    let attempt = Uuid::new_v4().to_string();
    {
        let gate = s.gate(&id).await;
        let _guard = gate.lock().await;
        s.allow_pair.lock().await.insert(
            id.clone(),
            crate::state::PairGrant {
                attempt: attempt.clone(),
                expires_at: now_ms() + 120_000,
            },
        );
        s.denied.lock().await.remove(&id);
    }
    let for_task = attempt.clone();
    tokio::spawn(async move {
        if let Err(e) = network::dial(s.clone(), id, true, Some(for_task)).await {
            tracing::warn!(error=%e, "pairing connection ended");
            s.changed();
        }
    });
    Ok(Json(json!({"id":attempt})))
}
async fn revoke(State(s): State<Arc<App>>, Path(id): Path<String>) -> Api<Value> {
    s.revoke(&id).await?;
    Ok(ok())
}
async fn pairings(State(s): State<Arc<App>>) -> Json<Vec<Pairing>> {
    Json(
        s.pairings
            .lock()
            .await
            .values()
            .map(|p| p.value.clone())
            .collect(),
    )
}
#[derive(Deserialize)]
struct Confirm {
    accept: bool,
}
async fn confirm(
    State(s): State<Arc<App>>,
    Path(id): Path<String>,
    Json(input): Json<Confirm>,
) -> Api<Value> {
    let sender = {
        let attempts = s.pairings.lock().await;
        let p = attempts
            .get(&id)
            .context("pairing expired or disconnected")?;
        ensure!(p.value.expires_at > now_ms(), "pairing expired");
        p.decisions.clone()
    };
    sender
        .try_send(input.accept)
        .context("pairing is no longer awaiting confirmation")?;
    Ok(ok())
}
#[derive(Deserialize)]
struct PeerQuery {
    peer_id: String,
}
async fn messages(State(s): State<Arc<App>>, Query(q): Query<PeerQuery>) -> Api<Vec<Message>> {
    Ok(Json(s.store.messages(&q.peer_id).await?))
}
#[derive(Deserialize)]
struct SendMessage {
    peer_id: String,
    body: String,
}
async fn send_message(State(s): State<Arc<App>>, Json(input): Json<SendMessage>) -> Api<Message> {
    ensure!(
        !input.body.trim().is_empty() && input.body.len() <= MAX_MESSAGE_SIZE,
        "message must contain 1 to 16384 UTF-8 bytes"
    );
    let gate = s.gate(&input.peer_id).await;
    let _guard = gate.lock().await;
    s.store.trusted(&input.peer_id).await?;
    let m = Message {
        id: Uuid::new_v4().to_string(),
        peer_id: input.peer_id,
        direction: Direction::Outgoing,
        body: input.body,
        created_at: now_ms(),
        status: MessageStatus::Pending,
    };
    Frame::Control(m.control())
        .encode()
        .map_err(anyhow::Error::msg)?;
    s.store.insert_message(&m).await?;
    s.outbox.notify_waiters();
    s.changed();
    Ok(Json(m))
}
async fn retry_message(State(s): State<Arc<App>>, Path(id): Path<String>) -> Api<Value> {
    let m = s.store.message(&id).await?;
    let gate = s.gate(&m.peer_id).await;
    let _guard = gate.lock().await;
    s.store.trusted(&m.peer_id).await?;
    let mut m = s.store.message(&id).await?;
    ensure!(
        m.direction == Direction::Outgoing
            && matches!(m.status, MessageStatus::Pending | MessageStatus::Failed),
        "message cannot be retried"
    );
    Frame::Control(m.control())
        .encode()
        .map_err(anyhow::Error::msg)?;
    m.status = MessageStatus::Pending;
    s.store.save_message(&m).await?;
    s.outbox.notify_waiters();
    s.changed();
    Ok(ok())
}
async fn transfers(State(s): State<Arc<App>>) -> Api<Vec<Transfer>> {
    Ok(Json(s.store.transfers().await?))
}
async fn transfer_action(
    State(s): State<Arc<App>>,
    Path((id, action)): Path<(String, String)>,
) -> Api<Value> {
    s.transfer_action(&id, &action).await?;
    Ok(ok())
}
async fn upload(
    State(s): State<Arc<App>>,
    Query(q): Query<PeerQuery>,
    mut multipart: Multipart,
) -> Api<Transfer> {
    let _permit = s
        .uploads
        .clone()
        .try_acquire_owned()
        .context("another upload is in progress")?;
    s.store.trusted(&q.peer_id).await?;
    let limit = s.settings.read().await.max_file_size;
    let id = Uuid::new_v4().to_string();
    let path = s.root.join("spool").join(format!("{id}.send"));
    let mut guard = UploadGuard {
        path: path.clone(),
        keep: false,
        store: s.store.clone(),
        id: id.clone(),
    };
    s.store.reserve_upload(&id, 0).await?;
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .await?;
    let mut field = multipart
        .next_field()
        .await?
        .context("file field is required")?;
    ensure!(
        field.name() == Some("file"),
        "multipart field must be named file"
    );
    let name = safe_name(field.file_name().unwrap_or("download"));
    let mut size = 0u64;
    let mut reserved = 0u64;
    let mut hash = Sha256::new();
    while let Some(chunk) = field.chunk().await? {
        size = size
            .checked_add(chunk.len() as u64)
            .context("upload length overflow")?;
        ensure!(size <= limit, "file exceeds configured limit");
        if size > reserved {
            let preferred = (size.div_ceil(CHUNK_SIZE as u64) * CHUNK_SIZE as u64).min(limit);
            reserved = s.store.reserve_upload_up_to(&id, size, preferred).await?;
        }
        file.write_all(&chunk).await?;
        hash.update(&chunk);
    }
    drop(field);
    ensure!(
        multipart.next_field().await?.is_none(),
        "upload one file at a time"
    );
    file.sync_all().await?;
    drop(file);
    files::sync_dir(path.parent().unwrap()).await?;
    s.store.reserve_upload(&id, size).await?;
    let t = Transfer {
        id,
        peer_id: q.peer_id,
        direction: Direction::Outgoing,
        name,
        size,
        sha256: hex::encode(hash.finalize()),
        offset: 0,
        status: TransferStatus::AwaitingAccept,
        failure_kind: None,
        error: None,
        created_at: now_ms(),
    };
    let gate = s.gate(&t.peer_id).await;
    let _guard = gate.lock().await;
    s.store.trusted(&t.peer_id).await?;
    s.store.finish_upload(&t).await?;
    guard.keep = true;
    s.outbox.notify_waiters();
    s.changed();
    Ok(Json(t))
}
async fn download(
    State(s): State<Arc<App>>,
    Path(id): Path<String>,
) -> std::result::Result<Response, ApiError> {
    let t = s.store.transfer(&id).await?;
    ensure!(
        t.direction == Direction::Incoming && t.status == TransferStatus::Completed,
        "file is not complete"
    );
    let file = tokio::fs::File::open(files::download_path(&s, &t).await).await?;
    let body = Body::from_stream(tokio_util::io::ReaderStream::new(file));
    Ok(attachment(&t.name, t.size, body))
}

pub fn attachment(name: &str, size: u64, body: Body) -> Response {
    let disposition = format!(
        "attachment; filename=\"download\"; filename*=UTF-8''{}",
        percent_encoding::utf8_percent_encode(&safe_name(name), percent_encoding::NON_ALPHANUMERIC)
    );
    (
        [
            (header::CONTENT_TYPE, "application/octet-stream".to_owned()),
            (header::CONTENT_DISPOSITION, disposition),
            (header::CONTENT_LENGTH, size.to_string()),
        ],
        body,
    )
        .into_response()
}
async fn settings(State(s): State<Arc<App>>) -> Json<Settings> {
    Json(s.settings.read().await.clone())
}
#[derive(Deserialize)]
struct UpdateSettings {
    max_file_size: u64,
}
async fn update_settings(
    State(s): State<Arc<App>>,
    Json(input): Json<UpdateSettings>,
) -> Api<Settings> {
    ensure!(
        (1..=MAX_FILE_SIZE).contains(&input.max_file_size),
        "max_file_size must be between 1 and 2147483648 bytes"
    );
    let mut settings = s.settings.write().await;
    settings.max_file_size = input.max_file_size;
    s.store.put("settings", &*settings).await?;
    s.changed();
    Ok(Json(settings.clone()))
}
async fn events(
    State(s): State<Arc<App>>,
    Extension(updater): Extension<upgrade::Updater>,
    headers: axum::http::HeaderMap,
    ws: WebSocketUpgrade,
) -> std::result::Result<Response, ApiError> {
    ensure!(
        headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) == Some(&s.origin),
        "WebSocket origin rejected"
    );
    let protocols = headers
        .get(header::SEC_WEBSOCKET_PROTOCOL)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let protocols: Vec<_> = protocols.split(',').map(str::trim).collect();
    ensure!(
        protocols.contains(&"lanlink") && protocols.contains(&s.token.as_str()),
        "WebSocket token rejected"
    );
    Ok(ws.protocols(["lanlink"]).on_upgrade(move |mut socket| async move {
        let mut events = s.events.subscribe();
        loop { tokio::select! {
            () = updater.shutdown.cancelled() => break,
            event = events.recv() => { if matches!(event, Err(tokio::sync::broadcast::error::RecvError::Closed)) { break; }
                    if socket.send(WsMessage::Text("{\"type\":\"changed\"}".into())).await.is_err() { break; } }
            incoming = socket.recv() => { if matches!(incoming, None | Some(Err(_)) | Some(Ok(WsMessage::Close(_)))) { break; } }
        } }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::FromRequest;

    #[tokio::test]
    async fn tiny_upload_succeeds_with_less_than_maximum_file_size_free() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let state = Arc::new(crate::state::test_state(root.path()).await?);
        let peer = "a".repeat(64);
        state
            .store
            .reserve(&Transfer {
                id: Uuid::new_v4().to_string(),
                peer_id: peer.clone(),
                direction: Direction::Incoming,
                name: "occupied".into(),
                size: DISK_QUOTA - 3,
                sha256: "a".repeat(64),
                offset: 0,
                status: TransferStatus::Transferring,
                failure_kind: None,
                error: None,
                created_at: now_ms(),
            })
            .await?;
        let request = Request::builder()
            .header(header::CONTENT_TYPE, "multipart/form-data; boundary=lanlink-test")
            .body(Body::from("--lanlink-test\r\nContent-Disposition: form-data; name=\"file\"; filename=\"tiny.txt\"\r\n\r\nabc\r\n--lanlink-test--\r\n"))?;
        let multipart = Multipart::from_request(request, &()).await?;
        let uploaded = upload(
            State(state.clone()),
            Query(PeerQuery { peer_id: peer }),
            multipart,
        )
        .await
        .map_err(|error| error.0)?
        .0;
        assert_eq!(uploaded.size, 3);
        assert_eq!(
            tokio::fs::read(files::spool_path(&state, &uploaded)).await?,
            b"abc"
        );
        assert_eq!(state.store.transfer(&uploaded.id).await?, uploaded);
        assert!(state.store.reserve_upload("extra", 1).await.is_err());
        Ok(())
    }
    #[tokio::test]
    async fn escaped_message_over_wire_limit_is_rejected_before_persistence() -> anyhow::Result<()>
    {
        let root = tempfile::tempdir()?;
        let state = Arc::new(crate::state::test_state(root.path()).await?);
        let peer = "a".repeat(64);
        let result = send_message(
            State(state.clone()),
            Json(SendMessage {
                peer_id: peer.clone(),
                body: "\u{1}".repeat(MAX_MESSAGE_SIZE),
            }),
        )
        .await;
        assert!(result.is_err());
        assert!(state.store.messages(&peer).await?.is_empty());
        let result = send_message(
            State(state.clone()),
            Json(SendMessage {
                peer_id: peer.clone(),
                body: "a".repeat(MAX_MESSAGE_SIZE),
            }),
        )
        .await
        .map_err(|error| error.0)?;
        assert_eq!(result.0.status, MessageStatus::Pending);
        assert_eq!(state.store.messages(&peer).await?.len(), 1);
        Ok(())
    }
}
