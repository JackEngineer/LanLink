use crate::{files, http::attachment, state::State as App};
use axum::{
    Extension, Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, Multipart, Path, Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use futures_util::StreamExt;
use lanlink_protocol::{MAX_FILE_SIZE, now_ms, safe_name};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    io,
    net::Ipv4Addr,
    path::{Path as FsPath, PathBuf},
    sync::Arc,
    time::Duration,
};
use tempfile::TempPath;
use tokio::{fs, io::AsyncWriteExt, net::TcpListener, sync::Mutex, time::Instant};
use tokio_util::{io::ReaderStream, sync::CancellationToken};
use uuid::Uuid;

const SESSION_LIFETIME: Duration = Duration::from_secs(600);
const MAX_FILES: usize = 16;
const MAX_SESSION_BYTES: u64 = MAX_FILE_SIZE;
const MULTIPART_OVERHEAD: u64 = 1024 * 1024;

type Result<T> = std::result::Result<T, MobileError>;

#[derive(Debug)]
struct MobileError(StatusCode, &'static str);

impl MobileError {
    fn io(error: io::Error) -> Self {
        tracing::warn!(%error, "mobile file transfer I/O failed");
        Self(
            StatusCode::INTERNAL_SERVER_ERROR,
            "mobile file transfer I/O failed",
        )
    }
}

impl IntoResponse for MobileError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"error": self.1}))).into_response()
    }
}

const FORBIDDEN: MobileError = MobileError(StatusCode::FORBIDDEN, "mobile request rejected");
const EXPIRED: MobileError = MobileError(StatusCode::GONE, "mobile session ended");
const NOT_APPROVED: MobileError = MobileError(StatusCode::CONFLICT, "desktop approval is required");
const NOT_FOUND: MobileError = MobileError(StatusCode::NOT_FOUND, "mobile file not found");
const SIZE_LIMIT: MobileError = MobileError(
    StatusCode::PAYLOAD_TOO_LARGE,
    "mobile file or session size limit exceeded",
);
const BAD_MULTIPART: MobileError =
    MobileError(StatusCode::BAD_REQUEST, "upload one file at a time");

pub struct Session {
    host: String,
    origin: String,
    url: String,
    cookie_name: String,
    expires_at: i64,
    deadline: Instant,
    stop: CancellationToken,
    listener_closed: CancellationToken,
    max_file_size: u64,
    outbox: PathBuf,
    download_dir: PathBuf,
    partial_prefix: String,
    upload: Mutex<()>,
    data: Mutex<SessionData>,
    assets: Assets,
}

struct MobileListener {
    listener: Option<TcpListener>,
    closed: CancellationToken,
}

impl axum::serve::Listener for MobileListener {
    type Io = tokio::net::TcpStream;
    type Addr = std::net::SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        axum::serve::Listener::accept(self.listener.as_mut().expect("live mobile listener")).await
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        self.listener
            .as_ref()
            .expect("live mobile listener")
            .local_addr()
    }
}

impl Drop for MobileListener {
    fn drop(&mut self) {
        drop(self.listener.take());
        self.closed.cancel();
    }
}

fn multipart_error(error: axum::extract::multipart::MultipartError) -> MobileError {
    if error.status() == StatusCode::PAYLOAD_TOO_LARGE {
        SIZE_LIMIT
    } else {
        BAD_MULTIPART
    }
}

struct SessionData {
    phase: Phase,
    files: Vec<StoredFile>,
}

enum Phase {
    Inviting { invite: String },
    Pending(Browser),
    Approved(Browser),
}

struct Browser {
    token: String,
    code: String,
}

struct Assets {
    html: Vec<u8>,
    script: Vec<u8>,
    css: Vec<u8>,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum FileDirection {
    ToComputer,
    ToMobile,
}

#[derive(Clone, Serialize)]
struct FileInfo {
    id: Uuid,
    name: String,
    size: u64,
    direction: FileDirection,
}

enum FileLocation {
    Staged(TempPath),
    Published(PathBuf),
}

struct StoredFile {
    info: FileInfo,
    location: FileLocation,
}

#[derive(Serialize)]
struct MobileView {
    phase: &'static str,
    code: Option<String>,
    expires_at: i64,
    max_file_size: u64,
    files: Vec<FileInfo>,
}

#[derive(Serialize)]
struct DesktopSession {
    url: String,
    #[serde(flatten)]
    view: MobileView,
}

#[derive(Serialize)]
struct DesktopView {
    addresses: Vec<Ipv4Addr>,
    session: Option<DesktopSession>,
}

fn addresses() -> io::Result<Vec<Ipv4Addr>> {
    let mut addresses: Vec<_> = if_addrs::get_if_addrs()?
        .into_iter()
        .filter_map(|interface| match interface.ip() {
            std::net::IpAddr::V4(ip) if ip.is_private() => Some(ip),
            _ => None,
        })
        .collect();
    addresses.sort_unstable();
    addresses.dedup();
    Ok(addresses)
}

fn partial_prefix(device_id: &str) -> String {
    format!(".lanlink-mobile-{device_id}-")
}

pub async fn recover(app: &App) {
    let path = PathBuf::from(&app.settings.read().await.download_dir);
    let prefix = partial_prefix(&app.device.read().await.id);
    if let Err(error) = remove_partials(&path, &prefix).await {
        tracing::warn!(%error, "mobile partial file recovery failed");
    }
}

async fn remove_partials(path: &FsPath, prefix: &str) -> io::Result<()> {
    let mut entries = match fs::read_dir(path).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    while let Some(entry) = entries.next_entry().await? {
        let name = entry.file_name();
        let partial = name
            .to_str()
            .and_then(|name| name.strip_prefix(prefix))
            .and_then(|name| name.strip_suffix(".part"))
            .is_some_and(|id| Uuid::parse_str(id).is_ok());
        if partial && entry.file_type().await?.is_file() {
            fs::remove_file(entry.path()).await?;
        }
    }
    Ok(())
}

impl Session {
    fn active(&self) -> Result<()> {
        if self.stop.is_cancelled() || Instant::now() >= self.deadline {
            return Err(EXPIRED);
        }
        Ok(())
    }

    async fn end(&self) {
        self.stop.cancel();
        let _upload = self.upload.lock().await;
        self.data.lock().await.files.clear();
    }

    fn view(&self, data: &SessionData) -> MobileView {
        let (phase, code) = match &data.phase {
            Phase::Inviting { .. } => ("inviting", None),
            Phase::Pending(browser) => ("pending", Some(browser.code.clone())),
            Phase::Approved(browser) => ("approved", Some(browser.code.clone())),
        };
        MobileView {
            phase,
            code,
            expires_at: self.expires_at,
            max_file_size: self.max_file_size,
            files: data.files.iter().map(|file| file.info.clone()).collect(),
        }
    }

    fn authorize_browser(&self, headers: &HeaderMap, phase: &Phase) -> Result<()> {
        let (Phase::Pending(browser) | Phase::Approved(browser)) = phase else {
            return Err(FORBIDDEN);
        };
        let authorized = headers.get_all(header::COOKIE).iter().any(|header| {
            header.to_str().is_ok_and(|cookies| {
                cookies.split(';').any(|cookie| {
                    cookie.trim().split_once('=').is_some_and(|(name, value)| {
                        name == self.cookie_name && value == browser.token
                    })
                })
            })
        });
        if !authorized {
            return Err(FORBIDDEN);
        }
        Ok(())
    }
}

impl Assets {
    async fn load(web_dir: &FsPath) -> Result<Self> {
        let root = web_dir.join("mobile");
        let (html, script, css) = tokio::try_join!(
            fs::read(root.join("index.html")),
            fs::read(root.join("main.js")),
            fs::read(root.join("style.css")),
        )
        .map_err(|error| {
            tracing::warn!(%error, "mobile page assets are unavailable");
            MobileError(
                StatusCode::SERVICE_UNAVAILABLE,
                "mobile page is unavailable",
            )
        })?;
        Ok(Self { html, script, css })
    }
}

pub fn desktop_router(web_dir: PathBuf) -> Router<Arc<App>> {
    Router::new()
        .route("/api/v1/mobile", get(desktop_view).post(open).delete(close))
        .route("/api/v1/mobile/approve", post(approve))
        .route("/api/v1/mobile/files", post(stage))
        .route("/api/v1/mobile/files/{id}", get(receipt))
        .layer(Extension(web_dir))
}

async fn desktop_view(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
) -> Result<Json<DesktopView>> {
    if !exact_header(&headers, "x-lanlink-token", &app.token) {
        return Err(FORBIDDEN);
    }
    snapshot(&app).await
}

async fn snapshot(app: &App) -> Result<Json<DesktopView>> {
    let session = match app.mobile.lock().await.as_ref() {
        Some(session) if session.active().is_ok() => {
            let data = session.data.lock().await;
            Some(DesktopSession {
                url: session.url.clone(),
                view: session.view(&data),
            })
        }
        _ => None,
    };
    Ok(Json(DesktopView {
        addresses: addresses().unwrap_or_default(),
        session,
    }))
}

#[derive(Deserialize)]
struct OpenRequest {
    address: Ipv4Addr,
}

async fn open(
    State(app): State<Arc<App>>,
    Extension(web_dir): Extension<PathBuf>,
    Json(input): Json<OpenRequest>,
) -> Result<Json<DesktopView>> {
    let mut slot = app.mobile.lock().await;
    if let Some(session) = slot.as_ref() {
        if session.active().is_ok() {
            return Err(MobileError(
                StatusCode::CONFLICT,
                "a mobile session is already open",
            ));
        }
        session.end().await;
        session.listener_closed.cancelled().await;
    }
    if !addresses()
        .map_err(MobileError::io)?
        .contains(&input.address)
    {
        return Err(MobileError(
            StatusCode::BAD_REQUEST,
            "select an available private IPv4 address",
        ));
    }

    let assets = Assets::load(&web_dir).await?;
    let listener = TcpListener::bind((input.address, 0))
        .await
        .map_err(MobileError::io)?;
    let host = listener.local_addr().map_err(MobileError::io)?.to_string();
    let origin = format!("http://{host}");
    let invite = Uuid::new_v4().simple().to_string();
    let settings = app.settings.read().await;
    fs::create_dir_all(&settings.download_dir)
        .await
        .map_err(MobileError::io)?;
    let session = Arc::new(Session {
        host,
        url: format!("{origin}/mobile/#{invite}"),
        origin,
        cookie_name: format!("lanlink_mobile_{}", Uuid::new_v4().simple()),
        expires_at: now_ms() + SESSION_LIFETIME.as_millis() as i64,
        deadline: Instant::now() + SESSION_LIFETIME,
        stop: CancellationToken::new(),
        listener_closed: CancellationToken::new(),
        max_file_size: settings.max_file_size.min(MAX_FILE_SIZE),
        outbox: app.root.join("spool"),
        download_dir: PathBuf::from(&settings.download_dir),
        partial_prefix: partial_prefix(&app.device.read().await.id),
        upload: Mutex::new(()),
        data: Mutex::new(SessionData {
            phase: Phase::Inviting { invite },
            files: Vec::new(),
        }),
        assets,
    });
    drop(settings);

    let listener = MobileListener {
        listener: Some(listener),
        closed: session.listener_closed.clone(),
    };
    *slot = Some(session.clone());
    drop(slot);
    let router = lan_router(session.clone());
    let for_shutdown = session.clone();
    tokio::spawn(async move {
        let shutdown = async move {
            tokio::select! {
                _ = for_shutdown.stop.cancelled() => {},
                _ = tokio::time::sleep_until(for_shutdown.deadline) => for_shutdown.end().await,
            }
        };
        let result = axum::serve(listener, router)
            .with_graceful_shutdown(shutdown)
            .await;
        session.end().await;
        if let Err(error) = result {
            tracing::warn!(%error, "mobile listener closed");
        }
    });
    snapshot(&app).await
}

async fn close(State(app): State<Arc<App>>) -> Result<Json<DesktopView>> {
    shutdown(&app).await;
    snapshot(&app).await
}

pub async fn shutdown(app: &App) {
    let mut slot = app.mobile.lock().await;
    if let Some(session) = slot.take() {
        session.end().await;
        session.listener_closed.cancelled().await;
    }
}

async fn current_session(app: &App) -> Result<Arc<Session>> {
    let session = app.mobile.lock().await.as_ref().cloned().ok_or(EXPIRED)?;
    session.active()?;
    Ok(session)
}

#[derive(Deserialize)]
struct Approval {
    code: String,
}

async fn approve(
    State(app): State<Arc<App>>,
    Json(input): Json<Approval>,
) -> Result<Json<DesktopView>> {
    let session = current_session(&app).await?;
    let mut data = session.data.lock().await;
    session.active()?;
    let Phase::Pending(browser) = &data.phase else {
        return Err(MobileError(
            StatusCode::CONFLICT,
            "no browser is awaiting approval",
        ));
    };
    if browser.code != input.code {
        return Err(MobileError(
            StatusCode::BAD_REQUEST,
            "comparison code does not match",
        ));
    }
    data.phase = Phase::Approved(Browser {
        token: browser.token.clone(),
        code: browser.code.clone(),
    });
    drop(data);
    snapshot(&app).await
}

async fn stage(State(app): State<Arc<App>>, multipart: Multipart) -> Result<Json<FileInfo>> {
    let session = current_session(&app).await?;
    upload(&session, multipart, FileDirection::ToMobile, None)
        .await
        .map(Json)
}

async fn receipt(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    let same_origin = if headers.contains_key("sec-fetch-site") {
        exact_header(&headers, "sec-fetch-site", "same-origin")
    } else {
        headers
            .get(header::REFERER)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|referer| referer.starts_with(&format!("{}/", app.origin)))
    };
    if !same_origin {
        return Err(FORBIDDEN);
    }
    let session = current_session(&app).await?;
    download(&session, id, FileDirection::ToComputer, None).await
}

fn lan_router(session: Arc<Session>) -> Router {
    Router::new()
        .route("/mobile/", get(page))
        .route("/mobile/main.js", get(script))
        .route("/mobile/style.css", get(stylesheet))
        .route(
            "/mobile/claim",
            post(claim).layer(DefaultBodyLimit::max(1024)),
        )
        .route("/mobile/state", get(mobile_view))
        .route(
            "/mobile/files",
            post(receive).layer(DefaultBodyLimit::max(
                (session.max_file_size + MULTIPART_OVERHEAD) as usize,
            )),
        )
        .route("/mobile/files/{id}", get(deliver))
        .layer(middleware::from_fn_with_state(session.clone(), protect))
        .with_state(session)
}

fn exact_header(headers: &HeaderMap, name: &str, expected: &str) -> bool {
    let mut values = headers.get_all(name).iter();
    values.next().is_some_and(|value| value == expected) && values.next().is_none()
}

async fn protect(State(session): State<Arc<Session>>, request: Request, next: Next) -> Response {
    let headers = request.headers();
    let read = matches!(*request.method(), Method::GET | Method::HEAD);
    let needs_origin = !read || headers.contains_key(header::ORIGIN);
    let mut response = if !exact_header(headers, "host", &session.host)
        || (needs_origin && !exact_header(headers, "origin", &session.origin))
    {
        FORBIDDEN.into_response()
    } else if let Err(error) = session.active() {
        error.into_response()
    } else {
        tokio::select! {
            biased;
            _ = session.stop.cancelled() => EXPIRED.into_response(),
            response = next.run(request) => response,
        }
    };
    let status = response.status();
    if (status.is_client_error() || status.is_server_error())
        && !exact_header(response.headers(), "content-type", "application/json")
    {
        response = if status == StatusCode::PAYLOAD_TOO_LARGE {
            SIZE_LIMIT
        } else {
            MobileError(status, "mobile request rejected")
        }
        .into_response();
    }
    let headers = response.headers_mut();
    for (name, value) in [
        (
            "content-security-policy",
            "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self'; form-action 'self'; frame-ancestors 'none'; base-uri 'none'",
        ),
        ("x-frame-options", "DENY"),
        ("x-content-type-options", "nosniff"),
        ("referrer-policy", "no-referrer"),
        ("cache-control", "no-store"),
    ] {
        headers.insert(name, HeaderValue::from_static(value));
    }
    response
}

async fn page(State(session): State<Arc<Session>>) -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        session.assets.html.clone(),
    )
}

async fn script(State(session): State<Arc<Session>>) -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        session.assets.script.clone(),
    )
}

async fn stylesheet(State(session): State<Arc<Session>>) -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        session.assets.css.clone(),
    )
}

#[derive(Deserialize)]
struct Claim {
    invite: String,
}

async fn claim(State(session): State<Arc<Session>>, Json(input): Json<Claim>) -> Result<Response> {
    let mut data = session.data.lock().await;
    session.active()?;
    let Phase::Inviting { invite } = &data.phase else {
        return Err(FORBIDDEN);
    };
    if invite != &input.invite {
        return Err(FORBIDDEN);
    }

    let token = Uuid::new_v4().simple().to_string();
    let code = (Uuid::new_v4().as_u128() & 0xffff_ffff) % 1_000_000;
    let code = format!("{code:06}");
    let remaining_seconds = session
        .deadline
        .saturating_duration_since(Instant::now())
        .as_secs()
        .max(1);
    let cookie = format!(
        "{}={token}; Path=/mobile/; HttpOnly; SameSite=Strict; Max-Age={remaining_seconds}",
        session.cookie_name,
    );
    data.phase = Phase::Pending(Browser { token, code });
    Ok(([(header::SET_COOKIE, cookie)], Json(session.view(&data))).into_response())
}

async fn mobile_view(
    State(session): State<Arc<Session>>,
    headers: HeaderMap,
) -> Result<Json<MobileView>> {
    let data = session.data.lock().await;
    session.active()?;
    session.authorize_browser(&headers, &data.phase)?;
    Ok(Json(session.view(&data)))
}

async fn receive(
    State(session): State<Arc<Session>>,
    headers: HeaderMap,
    multipart: Multipart,
) -> Result<Json<FileInfo>> {
    upload(
        &session,
        multipart,
        FileDirection::ToComputer,
        Some(&headers),
    )
    .await
    .map(Json)
}

async fn deliver(
    State(session): State<Arc<Session>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    download(&session, id, FileDirection::ToMobile, Some(&headers)).await
}

fn authorize_transfer(
    session: &Session,
    data: &SessionData,
    headers: Option<&HeaderMap>,
) -> Result<()> {
    session.active()?;
    if let Some(headers) = headers {
        session.authorize_browser(headers, &data.phase)?;
    }
    if !matches!(data.phase, Phase::Approved(_)) {
        return Err(NOT_APPROVED);
    }
    Ok(())
}

struct Draft {
    info: FileInfo,
    temp: TempPath,
}

async fn upload(
    session: &Session,
    multipart: Multipart,
    direction: FileDirection,
    headers: Option<&HeaderMap>,
) -> Result<FileInfo> {
    let _upload = session
        .upload
        .try_lock()
        .map_err(|_| MobileError(StatusCode::CONFLICT, "another mobile upload is in progress"))?;

    let limit = {
        let data = session.data.lock().await;
        authorize_transfer(session, &data, headers)?;
        if data.files.len() >= MAX_FILES {
            return Err(MobileError(
                StatusCode::PAYLOAD_TOO_LARGE,
                "mobile file count limit exceeded",
            ));
        }
        let used = data.files.iter().map(|file| file.info.size).sum::<u64>();
        session.max_file_size.min(MAX_SESSION_BYTES - used)
    };

    let draft = tokio::select! {
        biased;
        _ = session.stop.cancelled() => return Err(EXPIRED),
        result = write_draft(session, multipart, direction, limit) => result?,
    };

    let Draft { info, temp } = draft;
    let mut data = session.data.lock().await;
    authorize_transfer(session, &data, headers)?;
    let location = match direction {
        FileDirection::ToMobile => FileLocation::Staged(temp),
        FileDirection::ToComputer => {
            let destination = session
                .download_dir
                .join(format!("{}-{}", info.id, info.name));
            temp.persist_noclobber(&destination)
                .map_err(|error| MobileError::io(error.error))?;
            FileLocation::Published(destination)
        }
    };
    data.files.push(StoredFile {
        info: info.clone(),
        location,
    });
    drop(data);
    if direction == FileDirection::ToComputer {
        files::sync_dir(&session.download_dir)
            .await
            .map_err(|error| MobileError::io(io::Error::other(error)))?;
    }
    Ok(info)
}

async fn write_draft(
    session: &Session,
    mut multipart: Multipart,
    direction: FileDirection,
    limit: u64,
) -> Result<Draft> {
    let mut field = multipart
        .next_field()
        .await
        .map_err(multipart_error)?
        .ok_or(BAD_MULTIPART)?;
    if field.name() != Some("file") {
        return Err(BAD_MULTIPART);
    }
    let name = safe_name(field.file_name().unwrap_or("download"));
    let id = Uuid::new_v4();
    let directory = match direction {
        FileDirection::ToComputer => session.download_dir.clone(),
        FileDirection::ToMobile => session.outbox.clone(),
    };
    let prefix = format!("{}{id}", session.partial_prefix);
    let (file, temp) = tokio::task::spawn_blocking(move || {
        tempfile::Builder::new()
            .prefix(&prefix)
            .suffix(".part")
            .rand_bytes(0)
            .tempfile_in(directory)
            .map(|file| file.into_parts())
    })
    .await
    .map_err(|error| MobileError::io(io::Error::other(error)))?
    .map_err(MobileError::io)?;
    let mut file = fs::File::from_std(file);
    let mut size = 0u64;
    while let Some(chunk) = field.chunk().await.map_err(multipart_error)? {
        size = size.checked_add(chunk.len() as u64).ok_or(SIZE_LIMIT)?;
        if size > limit {
            return Err(SIZE_LIMIT);
        }
        file.write_all(&chunk).await.map_err(MobileError::io)?;
    }
    drop(field);
    if multipart
        .next_field()
        .await
        .map_err(multipart_error)?
        .is_some()
    {
        return Err(BAD_MULTIPART);
    }
    file.sync_all().await.map_err(MobileError::io)?;
    Ok(Draft {
        info: FileInfo {
            id,
            name,
            size,
            direction,
        },
        temp,
    })
}

async fn download(
    session: &Session,
    id: Uuid,
    direction: FileDirection,
    headers: Option<&HeaderMap>,
) -> Result<Response> {
    let data = session.data.lock().await;
    authorize_transfer(session, &data, headers)?;
    let file = data
        .files
        .iter()
        .find(|file| file.info.id == id && file.info.direction == direction)
        .ok_or(NOT_FOUND)?;
    let path = match &file.location {
        FileLocation::Staged(temp) => temp.as_ref(),
        FileLocation::Published(path) => path.as_path(),
    };
    let info = file.info.clone();
    let file = fs::File::open(path).await.map_err(MobileError::io)?;
    drop(data);
    let stream = ReaderStream::new(file).take_until(session.stop.clone().cancelled_owned());
    Ok(attachment(&info.name, info.size, Body::from_stream(stream)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::to_bytes, extract::FromRequest};
    use serde_json::Value;
    use tower::ServiceExt;

    fn test_session(root: &FsPath) -> Session {
        Session {
            host: "192.168.1.2:12345".into(),
            origin: "http://192.168.1.2:12345".into(),
            url: "http://192.168.1.2:12345/mobile/#invite".into(),
            cookie_name: "mobile_test".into(),
            expires_at: now_ms() + 60_000,
            deadline: Instant::now() + Duration::from_secs(60),
            stop: CancellationToken::new(),
            listener_closed: CancellationToken::new(),
            max_file_size: 4,
            outbox: root.join("spool"),
            download_dir: root.join("downloads"),
            partial_prefix: "test-mobile-".into(),
            upload: Mutex::new(()),
            data: Mutex::new(SessionData {
                phase: Phase::Inviting {
                    invite: "invite".into(),
                },
                files: Vec::new(),
            }),
            assets: Assets {
                html: b"mobile".to_vec(),
                script: Vec::new(),
                css: Vec::new(),
            },
        }
    }

    async fn body_json(response: Response) -> anyhow::Result<Value> {
        let body = to_bytes(response.into_body(), 8192).await?;
        Ok(serde_json::from_slice(&body)?)
    }

    async fn multipart(content: &[u8]) -> anyhow::Result<Multipart> {
        let mut body = b"--test\r\nContent-Disposition: form-data; name=\"file\"; filename=\"../../a.txt\"\r\n\r\n".to_vec();
        body.extend_from_slice(content);
        body.extend_from_slice(b"\r\n--test--\r\n");
        let request = Request::builder()
            .header(header::CONTENT_TYPE, "multipart/form-data; boundary=test")
            .body(Body::from(body))?;
        Ok(Multipart::from_request(request, &()).await?)
    }

    #[tokio::test]
    async fn approval_transfer_and_revocation_are_session_scoped() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let app = Arc::new(crate::state::test_state(root.path()).await?);
        let session = Arc::new(test_session(root.path()));
        *app.mobile.lock().await = Some(session.clone());

        let mut desktop_headers = HeaderMap::new();
        assert!(
            desktop_view(State(app.clone()), desktop_headers.clone())
                .await
                .is_err()
        );
        desktop_headers.insert("x-lanlink-token", HeaderValue::from_static("test-token"));
        assert!(
            desktop_view(State(app.clone()), desktop_headers)
                .await
                .is_ok()
        );

        let response = claim(
            State(session.clone()),
            Json(Claim {
                invite: "invite".into(),
            }),
        )
        .await
        .unwrap();
        let cookie = response.headers()[header::SET_COOKIE].to_str()?.to_owned();
        assert!(cookie.contains("HttpOnly; SameSite=Strict"));
        let view = body_json(response).await?;
        assert_eq!(view["phase"], "pending");
        let code = view["code"].as_str().unwrap().to_owned();
        assert_eq!(code.len(), 6);
        let mut browser = HeaderMap::new();
        browser.insert(
            header::COOKIE,
            HeaderValue::from_str(cookie.split(';').next().unwrap())?,
        );
        assert!(
            claim(
                State(session.clone()),
                Json(Claim {
                    invite: "invite".into()
                })
            )
            .await
            .is_err()
        );
        assert!(
            upload(
                &session,
                multipart(b"abc").await?,
                FileDirection::ToComputer,
                Some(&browser)
            )
            .await
            .is_err()
        );
        assert!(
            approve(
                State(app.clone()),
                Json(Approval {
                    code: "wrong".into()
                })
            )
            .await
            .is_err()
        );
        let approved = approve(State(app.clone()), Json(Approval { code: code.clone() }))
            .await
            .unwrap();
        assert_eq!(approved.0.session.unwrap().view.code, Some(code));

        let sent = upload(
            &session,
            multipart(b"abc").await?,
            FileDirection::ToMobile,
            None,
        )
        .await
        .unwrap();
        assert_eq!(sent.name, "a.txt");
        let response = download(&session, sent.id, FileDirection::ToMobile, Some(&browser))
            .await
            .unwrap();
        assert_eq!(response.headers()[header::CONTENT_LENGTH], "3");
        assert_eq!(to_bytes(response.into_body(), 4).await?.as_ref(), b"abc");
        assert!(
            download(&session, sent.id, FileDirection::ToComputer, None)
                .await
                .is_err()
        );
        assert!(
            download(
                &session,
                sent.id,
                FileDirection::ToMobile,
                Some(&HeaderMap::new())
            )
            .await
            .is_err()
        );

        let received = upload(
            &session,
            multipart(b"xyz").await?,
            FileDirection::ToComputer,
            Some(&browser),
        )
        .await
        .unwrap();
        let destination = session
            .download_dir
            .join(format!("{}-{}", received.id, received.name));
        assert_eq!(fs::read(&destination).await?, b"xyz");
        let response = download(&session, received.id, FileDirection::ToComputer, None)
            .await
            .unwrap();
        assert_eq!(to_bytes(response.into_body(), 4).await?.as_ref(), b"xyz");
        for (referer, allowed) in [
            (format!("{}/", app.origin), true),
            (format!("{}.attacker.example/", app.origin), false),
            ("http://attacker.example/".to_owned(), false),
            (String::new(), false),
        ] {
            let mut headers = HeaderMap::new();
            headers.insert(header::REFERER, HeaderValue::from_str(&referer)?);
            assert_eq!(
                receipt(State(app.clone()), headers, Path(received.id))
                    .await
                    .is_ok(),
                allowed
            );
        }
        assert!(
            upload(
                &session,
                multipart(b"large").await?,
                FileDirection::ToMobile,
                None
            )
            .await
            .is_err()
        );

        let streamed = download(&session, sent.id, FileDirection::ToMobile, Some(&browser))
            .await
            .unwrap();
        session.end().await;
        assert!(mobile_view(State(session.clone()), browser).await.is_err());
        assert!(to_bytes(streamed.into_body(), 4).await?.is_empty());
        assert!(
            fs::read_dir(&session.outbox)
                .await?
                .next_entry()
                .await?
                .is_none()
        );
        assert!(
            fs::read_dir(&session.download_dir)
                .await?
                .next_entry()
                .await?
                .is_some()
        );
        assert_eq!(fs::read(destination).await?, b"xyz");
        Ok(())
    }

    #[tokio::test]
    async fn lan_boundary_and_expiration_return_json() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let session = Arc::new(test_session(root.path()));
        let router = lan_router(session.clone());
        for (method, path, host, origin, expected) in [
            (
                "GET",
                "/api/v1/session",
                session.host.as_str(),
                None,
                StatusCode::NOT_FOUND,
            ),
            (
                "GET",
                "/mobile/",
                "attacker.example",
                None,
                StatusCode::FORBIDDEN,
            ),
            (
                "POST",
                "/mobile/claim",
                session.host.as_str(),
                None,
                StatusCode::FORBIDDEN,
            ),
            (
                "POST",
                "/mobile/claim",
                session.host.as_str(),
                Some("http://attacker.example"),
                StatusCode::FORBIDDEN,
            ),
        ] {
            let mut request = Request::builder()
                .method(method)
                .uri(path)
                .header(header::HOST, host);
            if let Some(origin) = origin {
                request = request.header(header::ORIGIN, origin);
            }
            let response = router.clone().oneshot(request.body(Body::empty())?).await?;
            assert_eq!(response.status(), expected);
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
            assert!(body_json(response).await?["error"].is_string());
        }
        let mut expired = test_session(root.path());
        expired.deadline = Instant::now() - Duration::from_secs(1);
        let expired = Arc::new(expired);
        let response = lan_router(expired.clone())
            .oneshot(
                Request::builder()
                    .uri("/mobile/")
                    .header(header::HOST, &expired.host)
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(response.status(), StatusCode::GONE);
        assert_eq!(body_json(response).await?["error"], "mobile session ended");
        assert!(
            claim(
                State(expired),
                Json(Claim {
                    invite: "invite".into()
                })
            )
            .await
            .is_err()
        );
        Ok(())
    }

    #[tokio::test]
    async fn restart_removes_only_owned_incomplete_files() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let app = crate::state::test_state(root.path()).await?;
        let directory = PathBuf::from(&app.settings.read().await.download_dir);
        let owned = directory.join(format!(
            "{}{}.part",
            partial_prefix(&app.device.read().await.id),
            Uuid::new_v4()
        ));
        let unrelated = directory.join("other.part");
        fs::write(&owned, b"partial").await?;
        fs::write(&unrelated, b"keep").await?;
        recover(&app).await;
        assert!(!owned.exists());
        assert_eq!(fs::read(unrelated).await?, b"keep");
        Ok(())
    }
}
