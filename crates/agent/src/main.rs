mod discovery;
mod files;
mod http;
mod mobile;
mod mobile_trust;
mod network;
mod state;

use anyhow::{Context, Result, ensure};
use clap::Parser;
use lanlink_protocol::*;
use lanlink_store::Store;
use std::{collections::HashMap, path::PathBuf, sync::Arc};
use tokio::{
    net::TcpListener,
    sync::{Mutex, RwLock, Semaphore, broadcast},
};

#[derive(Parser)]
#[command(version, about = "Private offline LAN messaging and file transfer")]
struct Args {
    #[arg(long)]
    data_dir: Option<PathBuf>,
    #[arg(long, default_value_t = 38761)]
    http_port: u16,
    #[arg(long, default_value_t = 38762)]
    tcp_port: u16,
    #[arg(long)]
    name: Option<String>,
    #[arg(long)]
    web_dir: Option<PathBuf>,
    #[arg(long)]
    no_mdns: bool,
    #[arg(long)]
    peer: Vec<String>,
}
#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let root = args.data_dir.unwrap_or_else(|| {
        directories::ProjectDirs::from("dev", "LANLink", "LANLink")
            .expect("home directory unavailable")
            .data_local_dir()
            .into()
    });
    tokio::fs::create_dir_all(&root).await?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).await?;
    }
    let root = tokio::fs::canonicalize(root).await?;
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.join("agent.lock"))?;
    fs2::FileExt::try_lock_exclusive(&lock)
        .context("another LANLink agent uses this data directory")?;
    for name in ["spool", "downloads", "logs"] {
        tokio::fs::create_dir_all(root.join(name)).await?;
    }
    let logs = tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix("agent")
        .max_log_files(7)
        .build(root.join("logs"))?;
    let (writer, _log_guard) = tracing_appender::non_blocking(logs);
    tracing_subscriber::fmt()
        .with_ansi(false)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "lanlink_agent=info".into()),
        )
        .with_writer(writer)
        .init();
    let store = Store::open(&root.join("lanlink.sqlite3")).await?;
    let keys: (String, String) = match store.get("identity").await? {
        Some(keys) => keys,
        None => {
            let pair = lanlink_transport::keypair()?;
            let keys = (hex::encode(pair.private), hex::encode(pair.public));
            store.put("identity", &keys).await?;
            keys
        }
    };
    let private = hex::decode(&keys.0)?;
    let public = hex::decode(&keys.1)?;
    ensure!(
        private.len() == 32 && public.len() == 32,
        "invalid persisted identity"
    );
    let id = lanlink_transport::device_id(&public);
    let mut device: Device = store.get("device").await?.unwrap_or(Device {
        id: id.clone(),
        name: format!("{} {}", std::env::consts::OS, &id[..8]),
        platform: std::env::consts::OS.into(),
    });
    ensure!(device.id == id, "persisted identity mismatch");
    if let Some(name) = args.name {
        ensure!(
            !name.trim().is_empty()
                && name.chars().count() <= 80
                && !name.chars().any(char::is_control),
            "name must contain 1 to 80 characters"
        );
        device.name = name.trim().into();
    }
    store.put("device", &device).await?;
    let settings: Settings = store.get("settings").await?.unwrap_or(Settings {
        download_dir: root.join("downloads").to_string_lossy().into_owned(),
        max_file_size: MAX_FILE_SIZE,
    });
    store.put("settings", &settings).await?;
    files::sync_dir(&root).await?;
    if let Some(parent) = root.parent() {
        files::sync_dir(parent).await?;
    }
    let tcp = match TcpListener::bind((std::net::Ipv4Addr::UNSPECIFIED, args.tcp_port)).await {
        Ok(listener) => listener,
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            TcpListener::bind((std::net::Ipv4Addr::UNSPECIFIED, 0)).await?
        }
        Err(e) => return Err(e).context("LAN TCP bind failed"),
    };
    let tcp_port = tcp.local_addr()?.port();
    let http = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, args.http_port))
        .await
        .context("local HTTP port is unavailable")?;
    let http_port = http.local_addr()?.port();
    let host = format!("127.0.0.1:{http_port}");
    let origin = format!("http://{host}");
    let (events, _) = broadcast::channel(64);
    let mobile_trust = Arc::new(mobile_trust::Registry::open(store.clone(), &device.id).await?);
    let state = Arc::new(state::State {
        store,
        mobile_trust,
        device: RwLock::new(device),
        private,
        token: hex::encode(lanlink_transport::keypair()?.private),
        root,
        settings: RwLock::new(settings),
        origin: origin.clone(),
        host,
        observations: RwLock::new(HashMap::new()),
        pairings: Mutex::new(HashMap::new()),
        active: Mutex::new(HashMap::new()),
        gates: Mutex::new(HashMap::new()),
        dialing: Mutex::new(HashMap::new()),
        allow_pair: Mutex::new(HashMap::new()),
        denied: Mutex::new(std::collections::HashSet::new()),
        pairing_slots: Arc::new(Semaphore::new(4)),
        events,
        handshakes: Arc::new(Semaphore::new(16)),
        sessions: Arc::new(Semaphore::new(64)),
        uploads: Arc::new(Semaphore::new(1)),
        tcp_port,
        mobile: Mutex::new(None),
        outbox: tokio::sync::Notify::new(),
    });
    sqlx::query("DELETE FROM uploads")
        .execute(&state.store.pool)
        .await?;
    files::recover(&state).await?;
    mobile::recover(&state).await;
    for peer in args.peer {
        network::add_manual(&state, &peer).await?;
    }
    let mdns = if args.no_mdns {
        None
    } else {
        Some(
            discovery::start(state.clone())
                .await
                .context("mDNS startup failed; --no-mdns permits explicitly configured peers")?,
        )
    };
    network::start_reconnect(state.clone());
    let listener_state = state.clone();
    tokio::spawn(async move {
        if let Err(e) = network::listen(listener_state, tcp).await {
            tracing::error!(error=%e, "LAN listener failed");
        }
    });
    let web_dir = args.web_dir.unwrap_or_else(|| {
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|p| p.join("web")))
            .filter(|p| p.exists())
            .unwrap_or_else(|| PathBuf::from("web/dist"))
    });
    println!("LANLink {VERSION} ready at {origin} (TCP {tcp_port})");
    tracing::info!(%origin, tcp_port, "agent started");
    axum::serve(http, http::router(state.clone(), web_dir))
        .with_graceful_shutdown(async move {
            if let Err(error) = shutdown_signal().await {
                tracing::warn!(%error, "shutdown signal listener failed");
            }
            mobile::shutdown(&state).await;
            if let Some(mdns) = mdns
                && let Ok(stopped) = mdns.shutdown()
                && !matches!(
                    tokio::time::timeout(std::time::Duration::from_secs(3), stopped.recv_async())
                        .await,
                    Ok(Ok(_))
                )
            {
                tracing::warn!("mDNS shutdown did not complete within three seconds");
            }
        })
        .await?;
    Ok(())
}

async fn shutdown_signal() -> std::io::Result<()> {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! {
            result = tokio::signal::ctrl_c() => result,
            _ = terminate.recv() => Ok(()),
        }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c().await
}
