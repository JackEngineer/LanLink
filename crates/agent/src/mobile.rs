use crate::{
    files,
    http::attachment,
    mobile_trust::{Registry, RememberedBrowser},
    state::State as App,
};
use axum::{
    Extension, Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, Multipart, Path, Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
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
const MESSAGE_BYTES: usize = 2048;
const MESSAGE_LIMIT: usize = 40;
const MESSAGE_INTERVAL: Duration = Duration::from_millis(750);
const JSON_MESSAGE_LIMIT: usize = 16 * 1024;
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
    id: Uuid,
    host: String,
    origin: String,
    url: String,
    cookie_name: String,
    trust: Arc<Registry>,
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
    messages: Vec<SessionMessage>,
    last_message: [Option<Instant>; 2],
}

#[derive(Clone, Serialize)]
struct SessionMessage {
    id: Uuid,
    body: String,
    direction: FileDirection,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MessageInput {
    id: Uuid,
    body: String,
}

enum Phase {
    Inviting { invite: String },
    Pending(Browser),
    Approved(Browser),
}

#[derive(Clone)]
struct Browser {
    token: String,
    code: String,
    name: String,
    remembered: Option<RememberedCredential>,
}

#[derive(Clone)]
struct RememberedCredential {
    browser: RememberedBrowser,
    token: String,
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
    remembered: bool,
    remembered_browser_id: Option<String>,
    expires_at: i64,
    max_file_size: u64,
    files: Vec<FileInfo>,
    messages: Vec<SessionMessage>,
}

#[derive(Serialize)]
struct DesktopSession {
    id: Uuid,
    url: String,
    #[serde(flatten)]
    view: MobileView,
}

#[derive(Serialize)]
struct DesktopView {
    addresses: Vec<Ipv4Addr>,
    session: Option<DesktopSession>,
    remembered_browsers: Vec<RememberedBrowser>,
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
        let mut data = self.data.lock().await;
        data.files.clear();
        data.messages.clear();
    }

    fn view(&self, data: &SessionData) -> MobileView {
        let (phase, code) = match &data.phase {
            Phase::Inviting { .. } => ("inviting", None),
            Phase::Pending(browser) => ("pending", Some(browser.code.clone())),
            Phase::Approved(browser) => ("approved", Some(browser.code.clone())),
        };
        let remembered = match &data.phase {
            Phase::Approved(browser) => browser.remembered.as_ref(),
            _ => None,
        };
        MobileView {
            remembered: remembered.is_some(),
            remembered_browser_id: remembered.map(|credential| credential.browser.id.clone()),
            phase,
            code,
            expires_at: self.expires_at,
            max_file_size: self.max_file_size,
            files: data.files.iter().map(|file| file.info.clone()).collect(),
            messages: data.messages.clone(),
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
        .route("/api/v1/mobile/browsers/{id}", delete(revoke_browser))
        .route("/api/v1/mobile/files", post(stage))
        .route(
            "/api/v1/mobile/messages/{session_id}",
            post(desktop_message).layer(DefaultBodyLimit::max(JSON_MESSAGE_LIMIT)),
        )
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
                id: session.id,
                url: session.url.clone(),
                view: session.view(&data),
            })
        }
        _ => None,
    };
    let remembered_browsers = app.mobile_trust.data.lock().await.list(now_ms());
    Ok(Json(DesktopView {
        remembered_browsers,
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
        id: Uuid::new_v4(),
        host,
        url: format!("{origin}/mobile/#{invite}"),
        origin,
        cookie_name: format!("lanlink_mobile_{}", Uuid::new_v4().simple()),
        trust: app.mobile_trust.clone(),
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
            messages: Vec::new(),
            last_message: [None; 2],
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
    #[serde(default = "remember_by_default")]
    remember: bool,
}

fn remember_by_default() -> bool {
    true
}

fn trust_error(error: anyhow::Error) -> MobileError {
    tracing::warn!(%error, "mobile browser registry operation failed");
    MobileError(
        StatusCode::INTERNAL_SERVER_ERROR,
        "cannot save remembered browser",
    )
}

async fn approve(
    State(app): State<Arc<App>>,
    Json(input): Json<Approval>,
) -> Result<Json<DesktopView>> {
    let session = current_session(&app).await?;
    let mut trust = session.trust.data.lock().await;
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
    let mut browser = browser.clone();
    if input.remember {
        let token = hex::encode(lanlink_transport::keypair().map_err(trust_error)?.private);
        let remembered = trust
            .register(&token, browser.name.clone(), now_ms())
            .await
            .map_err(trust_error)?;
        browser.remembered = Some(RememberedCredential {
            browser: remembered,
            token,
        });
    }
    session.active()?;
    data.phase = Phase::Approved(browser);
    drop(data);
    drop(trust);
    snapshot(&app).await
}

async fn revoke_browser(
    State(app): State<Arc<App>>,
    Path(id): Path<Uuid>,
) -> Result<Json<DesktopView>> {
    let slot = app.mobile.lock().await;
    let mut trust = app.mobile_trust.data.lock().await;
    let id = id.to_string();
    trust.revoke(&id).await.map_err(trust_error)?;
    let ended = if let Some(session) = slot.as_ref() {
        let data = session.data.lock().await;
        let matches = matches!(&data.phase, Phase::Approved(browser) if browser.remembered.as_ref().is_some_and(|credential| credential.browser.id == id));
        if matches {
            session.stop.cancel();
            Some(session.clone())
        } else {
            None
        }
    } else {
        None
    };
    drop(trust);
    drop(slot);
    if let Some(session) = ended {
        session.end().await;
    }
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
        .route(
            "/mobile/messages",
            post(phone_message).layer(DefaultBodyLimit::max(JSON_MESSAGE_LIMIT)),
        )
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

fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    let mut matches = headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|cookies| cookies.split(';'))
        .filter_map(|cookie| cookie.trim().split_once('='))
        .filter(|(key, _)| *key == name)
        .map(|(_, value)| value.to_owned());
    let value = matches.next()?;
    if matches.next().is_some() {
        return None;
    }
    Some(value)
}

fn browser_name(headers: &HeaderMap) -> String {
    let agent = headers
        .get(header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .filter(|value| value.len() <= 1024)
        .unwrap_or("");
    let device = if agent.contains("iPhone") {
        "iPhone"
    } else if agent.contains("iPad") {
        "iPad"
    } else if agent.contains("Android") {
        "Android"
    } else {
        "Browser"
    };
    let browser = if agent.contains("Edg/") || agent.contains("EdgiOS/") {
        "Edge"
    } else if agent.contains("Firefox/") || agent.contains("FxiOS/") {
        "Firefox"
    } else if agent.contains("Chrome/") || agent.contains("CriOS/") {
        "Chrome"
    } else if agent.contains("Safari/") {
        "Safari"
    } else {
        ""
    };
    format!("{device} {browser}").trim().to_owned()
}

async fn claim(
    State(session): State<Arc<Session>>,
    headers: HeaderMap,
    Json(input): Json<Claim>,
) -> Result<Response> {
    let mut trust = session.trust.data.lock().await;
    let mut data = session.data.lock().await;
    session.active()?;
    let Phase::Inviting { invite } = &data.phase else {
        return Err(FORBIDDEN);
    };
    if invite != &input.invite {
        return Err(FORBIDDEN);
    }

    let token = hex::encode(lanlink_transport::keypair().map_err(trust_error)?.private);
    let code = (Uuid::new_v4().as_u128() & 0xffff_ffff) % 1_000_000;
    let code = format!("{code:06}");
    let now = now_ms();
    let recall = cookie_value(&headers, &session.trust.cookie_name)
        .and_then(|token| trust.find(&token, now).map(|browser| (token, browser)));
    let mut browser = Browser {
        token: token.clone(),
        code,
        name: browser_name(&headers),
        remembered: None,
    };
    if let Some((remember_token, remembered)) = recall {
        let remembered = trust
            .renew(&remembered.id, now)
            .await
            .map_err(trust_error)?;
        browser.remembered = Some(RememberedCredential {
            browser: remembered,
            token: remember_token,
        });
    }
    session.active()?;
    let remaining_seconds = session
        .deadline
        .saturating_duration_since(Instant::now())
        .as_secs()
        .max(1);
    let cookie = format!(
        "{}={token}; Path=/mobile/; HttpOnly; SameSite=Strict; Max-Age={remaining_seconds}",
        session.cookie_name,
    );
    data.phase = if browser.remembered.is_some() {
        Phase::Approved(browser)
    } else {
        Phase::Pending(browser)
    };
    Ok(([(header::SET_COOKIE, cookie)], Json(session.view(&data))).into_response())
}

async fn mobile_view(State(session): State<Arc<Session>>, headers: HeaderMap) -> Result<Response> {
    let trust = session.trust.data.lock().await;
    let data = session.data.lock().await;
    session.active()?;
    session.authorize_browser(&headers, &data.phase)?;
    let mut response = Json(session.view(&data)).into_response();
    if let Phase::Approved(browser) = &data.phase
        && let Some(credential) = &browser.remembered
    {
        let remembered = &credential.browser;
        let token = &credential.token;
        let now = now_ms();
        if !trust.contains(&remembered.id, now) {
            session.stop.cancel();
            return Err(EXPIRED);
        }
        let seconds = ((remembered.expires_at - now) / 1000).max(1);
        let cookie = format!(
            "{}={token}; Path=/mobile/; HttpOnly; SameSite=Strict; Max-Age={seconds}",
            session.trust.cookie_name
        );
        response.headers_mut().insert(
            header::SET_COOKIE,
            HeaderValue::from_str(&cookie).map_err(|_| FORBIDDEN)?,
        );
    }
    Ok(response)
}

async fn desktop_message(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Path(session_id): Path<Uuid>,
    Json(input): Json<MessageInput>,
) -> Result<Json<SessionMessage>> {
    if !exact_header(&headers, "x-lanlink-token", &app.token) {
        return Err(FORBIDDEN);
    }
    let session = current_session(&app).await?;
    if session.id != session_id {
        return Err(EXPIRED);
    }
    send_session_message(&session, None, FileDirection::ToMobile, input).await
}

async fn phone_message(
    State(session): State<Arc<Session>>,
    headers: HeaderMap,
    Json(input): Json<MessageInput>,
) -> Result<Json<SessionMessage>> {
    send_session_message(&session, Some(&headers), FileDirection::ToComputer, input).await
}

async fn send_session_message(
    session: &Session,
    headers: Option<&HeaderMap>,
    direction: FileDirection,
    input: MessageInput,
) -> Result<Json<SessionMessage>> {
    let trust = session.trust.data.lock().await;
    let mut data = session.data.lock().await;
    authorize_transfer(session, &data, headers)?;
    if let Phase::Approved(browser) = &data.phase
        && let Some(credential) = &browser.remembered
        && !trust.contains(&credential.browser.id, now_ms())
    {
        session.stop.cancel();
        return Err(EXPIRED);
    }
    let body = input.body.trim();
    if body.is_empty() || body.len() > MESSAGE_BYTES {
        return Err(MobileError(
            StatusCode::BAD_REQUEST,
            "消息不能为空，且不能超过 2048 字节。",
        ));
    }
    if let Some(existing) = data
        .messages
        .iter()
        .find(|item| item.direction == direction && item.id == input.id)
    {
        return if existing.body == body {
            Ok(Json(existing.clone()))
        } else {
            Err(MobileError(
                StatusCode::CONFLICT,
                "此消息编号已用于其他内容。",
            ))
        };
    }
    if data.messages.len() >= MESSAGE_LIMIT {
        return Err(MobileError(
            StatusCode::CONFLICT,
            "本次连接已达到 40 条文字上限，请开启新连接。",
        ));
    }
    let index = usize::from(direction == FileDirection::ToMobile);
    let now = Instant::now();
    if data.last_message[index].is_some_and(|last| now.duration_since(last) < MESSAGE_INTERVAL) {
        return Err(MobileError(
            StatusCode::TOO_MANY_REQUESTS,
            "发送太快，请稍后重试。",
        ));
    }
    let message = SessionMessage {
        id: input.id,
        body: body.to_owned(),
        direction,
    };
    data.last_message[index] = Some(now);
    data.messages.push(message.clone());
    Ok(Json(message))
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
        FileLocation::Staged(temp) => temp.to_path_buf(),
        FileLocation::Published(path) => path.clone(),
    };
    let info = file.info.clone();
    drop(data);
    let file = fs::File::open(path).await.map_err(MobileError::io)?;
    session.active()?;
    let stream = ReaderStream::new(file).take_until(session.stop.clone().cancelled_owned());
    Ok(attachment(&info.name, info.size, Body::from_stream(stream)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::to_bytes, extract::FromRequest};
    use serde_json::Value;
    use tower::ServiceExt;

    async fn test_session(root: &FsPath) -> Session {
        let store = lanlink_store::Store::open(&root.join("test.sqlite3"))
            .await
            .unwrap();
        Session {
            id: Uuid::new_v4(),
            trust: Arc::new(Registry::open(store, &"a".repeat(64)).await.unwrap()),
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
                messages: Vec::new(),
                last_message: [None; 2],
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
    async fn session_text_uses_existing_approval_and_boundaries_without_listening()
    -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let app = Arc::new(crate::state::test_state(root.path()).await?);
        let session = paired_session(root.path(), app.mobile_trust.clone(), 10008).await;
        *app.mobile.lock().await = Some(session.clone());
        let phone = lan_router(session.clone());
        let desktop = desktop_router(root.path().to_path_buf()).with_state(app.clone());
        let (view, cookie) = claim_browser(session.clone(), HeaderMap::new(), "invite").await?;
        let id = Uuid::new_v4();
        let request = |origin: &str, cookie: &str, id: Uuid, body: &str| {
            Request::builder()
                .method(Method::POST)
                .uri("/mobile/messages")
                .header(header::HOST, &session.host)
                .header(header::ORIGIN, origin)
                .header(header::COOKIE, cookie.split(';').next().unwrap())
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({"id":id, "body":body}).to_string()))
                .unwrap()
        };
        let desktop_request = |session_id: Uuid, token: &str| {
            Request::builder()
                .method(Method::POST)
                .uri(format!("/api/v1/mobile/messages/{session_id}"))
                .header("x-lanlink-token", token)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(json!({"id":id, "body":"电脑回复"}).to_string()))
                .unwrap()
        };
        let oversized_text = "字".repeat(683);
        let oversized_json = "a".repeat(17_000);
        let ask = |id, body| request(&session.origin, &cookie, id, body);
        assert_eq!(
            phone.clone().oneshot(ask(id, "手机文字")).await?.status(),
            StatusCode::CONFLICT
        );
        let approved = approve(
            State(app.clone()),
            Json(Approval {
                code: view["code"].as_str().unwrap().into(),
                remember: true,
            }),
        )
        .await
        .unwrap();
        let remembered_id = Uuid::parse_str(&approved.0.remembered_browsers[0].id)?;
        assert_eq!(
            phone
                .clone()
                .oneshot(request(&session.origin, "not_the_cookie=1", id, "手机文字"))
                .await?
                .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            phone
                .clone()
                .oneshot(request("http://wrong.invalid", &cookie, id, "手机文字"))
                .await?
                .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            desktop
                .clone()
                .oneshot(desktop_request(session.id, "wrong"))
                .await?
                .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            desktop
                .clone()
                .oneshot(desktop_request(Uuid::new_v4(), &app.token))
                .await?
                .status(),
            StatusCode::GONE
        );
        assert_eq!(
            phone.clone().oneshot(ask(id, " ")).await?.status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            phone
                .clone()
                .oneshot(ask(id, &oversized_text))
                .await?
                .status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            phone
                .clone()
                .oneshot(ask(id, &oversized_json))
                .await?
                .status(),
            StatusCode::PAYLOAD_TOO_LARGE
        );
        assert_eq!(
            phone.clone().oneshot(ask(id, "手机文字")).await?.status(),
            StatusCode::OK
        );
        assert_eq!(
            phone.clone().oneshot(ask(id, " 手机文字 ")).await?.status(),
            StatusCode::OK
        );
        assert_eq!(
            phone.clone().oneshot(ask(id, "不同文字")).await?.status(),
            StatusCode::CONFLICT
        );
        assert_eq!(
            phone
                .clone()
                .oneshot(ask(Uuid::new_v4(), "第二条"))
                .await?
                .status(),
            StatusCode::TOO_MANY_REQUESTS
        );
        assert_eq!(
            desktop
                .clone()
                .oneshot(desktop_request(session.id, &app.token))
                .await?
                .status(),
            StatusCode::OK
        );
        for _ in 0..2 {
            let view = body_json(
                mobile_view(State(session.clone()), cookie_header(&cookie))
                    .await
                    .unwrap(),
            )
            .await?;
            assert_eq!(view["messages"].as_array().unwrap().len(), 2);
            assert_eq!(view["messages"][0]["body"], "手机文字");
            assert_eq!(view["messages"][1]["body"], "电脑回复");
        }
        let snap = snapshot(&app).await.unwrap();
        assert_eq!(snap.0.session.unwrap().view.messages.len(), 2);
        {
            let mut data = session.data.lock().await;
            for _ in data.messages.len()..MESSAGE_LIMIT {
                data.messages.push(SessionMessage {
                    id: Uuid::new_v4(),
                    body: "填充".into(),
                    direction: FileDirection::ToComputer,
                });
            }
        }
        assert_eq!(
            phone
                .clone()
                .oneshot(ask(Uuid::new_v4(), "队列已满"))
                .await?
                .status(),
            StatusCode::CONFLICT
        );
        assert_eq!(
            phone.clone().oneshot(ask(id, "手机文字")).await?.status(),
            StatusCode::OK
        );
        assert!(
            revoke_browser(State(app.clone()), Path(remembered_id))
                .await
                .unwrap()
                .0
                .session
                .is_none()
        );
        assert!(session.data.lock().await.messages.is_empty());
        assert_eq!(
            phone.oneshot(ask(id, "手机文字")).await?.status(),
            StatusCode::GONE
        );
        assert_eq!(
            desktop
                .oneshot(desktop_request(session.id, &app.token))
                .await?
                .status(),
            StatusCode::GONE
        );
        let next = paired_session(root.path(), app.mobile_trust.clone(), 10009).await;
        assert!(next.data.lock().await.messages.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn desktop_text_production_router_rejects_mismatched_origin_token_and_large_body()
    -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let app = Arc::new(crate::state::test_state(root.path()).await?);
        let router = crate::http::router(
            app.clone(),
            root.path().into(),
            crate::upgrade::Updater::new(root.path(), root.path(), 0),
        );
        let post = |origin: &str, token: &str, text: String| {
            Request::builder()
                .method(Method::POST)
                .uri(format!("/api/v1/mobile/messages/{}", Uuid::new_v4()))
                .header(header::HOST, &app.host)
                .header(header::ORIGIN, origin)
                .header("x-lanlink-token", token)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"id": Uuid::new_v4(), "body": text}).to_string(),
                ))
                .unwrap()
        };
        for (origin, token) in [
            ("http://wrong.invalid", app.token.as_str()),
            (app.origin.as_str(), "wrong"),
        ] {
            assert_eq!(
                router
                    .clone()
                    .oneshot(post(origin, token, "text".into()))
                    .await?
                    .status(),
                StatusCode::FORBIDDEN
            );
        }
        assert_eq!(
            router
                .clone()
                .oneshot(post(
                    &app.origin,
                    &app.token,
                    "a".repeat(JSON_MESSAGE_LIMIT + 1)
                ))
                .await?
                .status(),
            StatusCode::PAYLOAD_TOO_LARGE
        );
        assert_eq!(
            router
                .oneshot(post(&app.origin, &app.token, "small".into()))
                .await?
                .status(),
            StatusCode::GONE
        );
        Ok(())
    }

    #[tokio::test]
    async fn approval_transfer_and_revocation_are_session_scoped() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let app = Arc::new(crate::state::test_state(root.path()).await?);
        let session = Arc::new(test_session(root.path()).await);
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
            HeaderMap::new(),
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
                HeaderMap::new(),
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
                    code: "wrong".into(),
                    remember: true
                })
            )
            .await
            .is_err()
        );
        let approved = approve(
            State(app.clone()),
            Json(Approval {
                code: code.clone(),
                remember: true,
            }),
        )
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

    async fn paired_session(root: &FsPath, trust: Arc<Registry>, port: u16) -> Arc<Session> {
        let mut session = test_session(root).await;
        session.trust = trust;
        session.host = format!("192.168.1.2:{port}");
        session.origin = format!("http://{}", session.host);
        session.cookie_name = format!("mobile_{port}");
        Arc::new(session)
    }

    fn cookie_header(cookie: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_str(cookie.split(';').next().unwrap()).unwrap(),
        );
        headers
    }

    async fn claim_browser(
        session: Arc<Session>,
        headers: HeaderMap,
        invite: &str,
    ) -> anyhow::Result<(Value, String)> {
        let response = claim(
            State(session),
            headers,
            Json(Claim {
                invite: invite.into(),
            }),
        )
        .await
        .unwrap();
        let cookie = response.headers()[header::SET_COOKIE].to_str()?.to_owned();
        Ok((body_json(response).await?, cookie))
    }

    #[tokio::test]
    async fn remembered_browser_requires_new_invite_and_fresh_session_cookie_after_restart()
    -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let app = Arc::new(crate::state::test_state(root.path()).await?);
        let first = paired_session(root.path(), app.mobile_trust.clone(), 10001).await;
        *app.mobile.lock().await = Some(first.clone());
        let mut headers = HeaderMap::new();
        headers.insert(
            header::USER_AGENT,
            HeaderValue::from_static("Mozilla iPhone Version/18.0 Mobile Safari/604.1"),
        );
        let (view, session_cookie) = claim_browser(first.clone(), headers, "invite").await?;
        assert_eq!(view["phase"], "pending");
        assert_eq!(view["remembered"], false);
        assert!(view["remembered_browser_id"].is_null());
        let input: Approval = serde_json::from_value(json!({"code": view["code"]}))?;
        assert!(input.remember);
        let approved = approve(State(app.clone()), Json(input)).await.unwrap();
        let id = approved
            .0
            .session
            .unwrap()
            .view
            .remembered_browser_id
            .unwrap();
        assert_eq!(approved.0.remembered_browsers[0].name, "iPhone Safari");
        let response = mobile_view(State(first.clone()), cookie_header(&session_cookie))
            .await
            .unwrap();
        let remembered_cookie = response.headers()[header::SET_COOKIE].to_str()?.to_owned();
        assert!(remembered_cookie.contains("Path=/mobile/; HttpOnly; SameSite=Strict; Max-Age="));
        assert!(!remembered_cookie.contains("Domain="));
        let retry = mobile_view(State(first.clone()), cookie_header(&session_cookie))
            .await
            .unwrap();
        assert_eq!(
            retry.headers()[header::SET_COOKIE]
                .to_str()?
                .split(';')
                .next(),
            remembered_cookie.split(';').next()
        );
        let raw_token = remembered_cookie
            .split(';')
            .next()
            .unwrap()
            .split_once('=')
            .unwrap()
            .1;
        let persisted: String =
            sqlx::query_scalar("SELECT value FROM kv WHERE key LIKE 'mobile_browsers_%'")
                .fetch_one(&app.store.pool)
                .await?;
        assert!(!persisted.contains(raw_token));
        assert!(persisted.contains(&crate::mobile_trust::digest(raw_token)));
        first.end().await;

        let reopened = Arc::new(
            Registry::open(
                lanlink_store::Store::open(&root.path().join("test.sqlite3")).await?,
                &"a".repeat(64),
            )
            .await?,
        );
        assert_eq!(reopened.cookie_name, app.mobile_trust.cookie_name);
        let second = paired_session(root.path(), reopened.clone(), 10002).await;
        assert!(
            mobile_view(State(second.clone()), cookie_header(&remembered_cookie))
                .await
                .is_err()
        );
        assert!(
            claim(
                State(second.clone()),
                cookie_header(&remembered_cookie),
                Json(Claim {
                    invite: "old-invite".into()
                })
            )
            .await
            .is_err()
        );
        let (view, fresh_cookie) =
            claim_browser(second.clone(), cookie_header(&remembered_cookie), "invite").await?;
        assert_eq!(view["phase"], "approved");
        assert_eq!(view["remembered"], true);
        assert_eq!(view["remembered_browser_id"], id);
        assert_ne!(fresh_cookie, session_cookie);
        assert!(
            mobile_view(State(second.clone()), cookie_header(&session_cookie))
                .await
                .is_err()
        );
        assert!(
            mobile_view(State(second.clone()), cookie_header(&remembered_cookie))
                .await
                .is_err()
        );
        assert!(
            mobile_view(State(second.clone()), cookie_header(&fresh_cookie))
                .await
                .is_ok()
        );
        let record = reopened.data.lock().await.list(now_ms()).pop().unwrap();
        assert!(record.last_used_at >= record.created_at);
        assert_eq!(
            record.expires_at - record.last_used_at,
            crate::mobile_trust::LIFETIME_MS
        );
        Ok(())
    }

    #[tokio::test]
    async fn unrecognized_expired_and_guest_browsers_require_desktop_approval() -> anyhow::Result<()>
    {
        let root = tempfile::tempdir()?;
        let app = Arc::new(crate::state::test_state(root.path()).await?);
        let expired_token = "a".repeat(64);
        app.mobile_trust
            .data
            .lock()
            .await
            .register(&expired_token, "Browser".into(), 0)
            .await?;
        for headers in [
            HeaderMap::new(),
            cookie_header(&format!(
                "{}={}",
                app.mobile_trust.cookie_name,
                "b".repeat(64)
            )),
            cookie_header(&format!("{}={expired_token}", app.mobile_trust.cookie_name)),
        ] {
            let session = paired_session(root.path(), app.mobile_trust.clone(), 10003).await;
            *app.mobile.lock().await = Some(session.clone());
            let (view, cookie) = claim_browser(session.clone(), headers, "invite").await?;
            assert_eq!(view["phase"], "pending");
            assert!(
                authorize_transfer(
                    &session,
                    &*session.data.lock().await,
                    Some(&cookie_header(&cookie))
                )
                .is_err()
            );
            let _ = approve(
                State(app.clone()),
                Json(Approval {
                    code: view["code"].as_str().unwrap().into(),
                    remember: false,
                }),
            )
            .await
            .unwrap();
            let response = mobile_view(State(session), cookie_header(&cookie))
                .await
                .unwrap();
            assert!(!response.headers().contains_key(header::SET_COOKIE));
            let approved = body_json(response).await?;
            assert_eq!(approved["remembered"], false);
            assert!(approved["remembered_browser_id"].is_null());
            assert!(app.mobile_trust.data.lock().await.list(now_ms()).is_empty());
        }
        Ok(())
    }

    #[tokio::test]
    async fn revocation_stops_active_streams_and_prevents_poll_or_future_recall()
    -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let app = Arc::new(crate::state::test_state(root.path()).await?);
        let session = paired_session(root.path(), app.mobile_trust.clone(), 10004).await;
        *app.mobile.lock().await = Some(session.clone());
        let (view, cookie) = claim_browser(session.clone(), HeaderMap::new(), "invite").await?;
        let approved = approve(
            State(app.clone()),
            Json(Approval {
                code: view["code"].as_str().unwrap().into(),
                remember: true,
            }),
        )
        .await
        .unwrap();
        let id = approved.0.remembered_browsers[0].id.clone();
        let response = mobile_view(State(session.clone()), cookie_header(&cookie))
            .await
            .unwrap();
        let remembered = response.headers()[header::SET_COOKIE].to_str()?.to_owned();
        let sent = upload(
            &session,
            multipart(b"abc").await?,
            FileDirection::ToMobile,
            None,
        )
        .await
        .unwrap();
        let stream = download(
            &session,
            sent.id,
            FileDirection::ToMobile,
            Some(&cookie_header(&cookie)),
        )
        .await
        .unwrap();
        let response = revoke_browser(State(app.clone()), Path(Uuid::parse_str(&id)?))
            .await
            .unwrap();
        assert!(response.0.remembered_browsers.is_empty());
        assert!(response.0.session.is_none());
        assert!(to_bytes(stream.into_body(), 4).await?.is_empty());
        assert!(
            mobile_view(State(session.clone()), cookie_header(&cookie))
                .await
                .is_err()
        );
        assert!(
            approve(
                State(app.clone()),
                Json(Approval {
                    code: view["code"].as_str().unwrap().into(),
                    remember: true
                })
            )
            .await
            .is_err()
        );
        let reopened = Arc::new(Registry::open(app.store.clone(), &"a".repeat(64)).await?);
        let next = paired_session(root.path(), reopened, 10005).await;
        let (view, _) = claim_browser(next, cookie_header(&remembered), "invite").await?;
        assert_eq!(view["phase"], "pending");
        Ok(())
    }

    #[tokio::test]
    async fn failed_persistence_never_approves_or_reports_revocation() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let app = Arc::new(crate::state::test_state(root.path()).await?);
        let session = paired_session(root.path(), app.mobile_trust.clone(), 10006).await;
        *app.mobile.lock().await = Some(session.clone());
        let (view, _) = claim_browser(session.clone(), HeaderMap::new(), "invite").await?;
        sqlx::query("DROP TABLE kv")
            .execute(&app.store.pool)
            .await?;
        assert!(
            approve(
                State(app.clone()),
                Json(Approval {
                    code: view["code"].as_str().unwrap().into(),
                    remember: true
                })
            )
            .await
            .is_err()
        );
        assert!(matches!(session.data.lock().await.phase, Phase::Pending(_)));
        assert!(
            revoke_browser(State(app), Path(Uuid::new_v4()))
                .await
                .is_err()
        );
        Ok(())
    }

    #[tokio::test]
    async fn lan_boundary_and_expiration_return_json() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let session = Arc::new(test_session(root.path()).await);
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
        let mut expired = test_session(root.path()).await;
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
        let response = lan_router(expired.clone())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/mobile/messages")
                    .header(header::HOST, &expired.host)
                    .header(header::ORIGIN, &expired.origin)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        json!({"id": Uuid::new_v4(), "body": "过期"}).to_string(),
                    ))?,
            )
            .await?;
        assert_eq!(response.status(), StatusCode::GONE);
        assert!(
            claim(
                State(expired),
                HeaderMap::new(),
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
