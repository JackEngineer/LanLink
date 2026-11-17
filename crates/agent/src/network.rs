use crate::{
    files,
    state::{Active, Observation, PairAttempt, State},
};
use anyhow::{Context, Result, bail, ensure};
use lanlink_protocol::*;
use lanlink_transport::{Connection, commitment, device_id, keypair, security_code};
use std::{collections::HashMap, sync::Arc};
use tokio::{
    io::{AsyncReadExt, AsyncSeekExt},
    net::{TcpListener, TcpStream},
    sync::{mpsc, watch},
    time::{Duration, Instant, interval, timeout},
};
use uuid::Uuid;

pub async fn listen(state: Arc<State>, listener: TcpListener) -> Result<()> {
    loop {
        let (socket, address) = listener.accept().await?;
        if !local_ip(address.ip()) {
            continue;
        }
        let Ok(permit) = state.handshakes.clone().try_acquire_owned() else {
            continue;
        };
        let state = state.clone();
        tokio::spawn(async move {
            let _permit = permit;
            if let Err(e) = session(state, socket, false, false, None, None).await {
                tracing::debug!(error=%e, "incoming connection ended");
            }
        });
    }
}
pub async fn dial(
    state: Arc<State>,
    peer: String,
    pairing: bool,
    attempt: Option<String>,
) -> Result<()> {
    {
        let mut dialing = state.dialing.lock().await;
        ensure!(
            !dialing.contains_key(&peer),
            "connection attempt already in progress"
        );
        dialing.insert(peer.clone(), true);
    }
    let result = async {
        let observation = state
            .observations
            .read()
            .await
            .get(&peer)
            .cloned()
            .context("device address is unavailable")?;
        let mut last = None;
        for address in &observation.peer.addresses {
            let address: std::net::IpAddr = address.parse()?;
            ensure!(
                local_ip(address),
                "only local network addresses are allowed"
            );
            match timeout(
                Duration::from_secs(3),
                TcpStream::connect((address, observation.peer.port)),
            )
            .await
            {
                Ok(Ok(socket)) => {
                    return session(
                        state.clone(),
                        socket,
                        true,
                        pairing,
                        Some(peer.clone()),
                        attempt,
                    )
                    .await;
                }
                Ok(Err(error)) => last = Some(error.to_string()),
                Err(_) => last = Some("connection timed out".into()),
            }
        }
        bail!(
            "{}",
            last.unwrap_or_else(|| "device has no addresses".into())
        )
    }
    .await;
    state.dialing.lock().await.remove(&peer);
    result
}
pub fn start_reconnect(state: Arc<State>) {
    tokio::spawn(async move {
        let mut tick = interval(Duration::from_secs(3));
        loop {
            tick.tick().await;
            let Ok(trusts) = state.store.trusts().await else {
                continue;
            };
            for trust in trusts {
                let id = trust.device.id;
                if state.denied.lock().await.contains(&id)
                    || trust.revoked
                    || state.active.lock().await.contains_key(&id)
                    || state.dialing.lock().await.contains_key(&id)
                {
                    continue;
                }
                if !state.observations.read().await.contains_key(&id) {
                    continue;
                }
                let cloned = state.clone();
                tokio::spawn(async move {
                    if let Err(e) = dial(cloned, id, false, None).await {
                        tracing::debug!(error=%e, "reconnect ended");
                    }
                });
            }
        }
    });
}
async fn pair(
    state: &State,
    conn: &mut Connection,
    device: &Device,
    initiator: bool,
    attempt: Option<String>,
) -> Result<()> {
    let _pairing_permit = state
        .pairing_slots
        .clone()
        .try_acquire_owned()
        .context("too many pairing attempts")?;
    let key = hex::encode(&conn.remote_key);
    let session = hex::encode(&conn.session);
    if let Some(trust) = state.store.trust(&device.id).await? {
        ensure!(trust.key == key, "pinned identity changed");
        if trust.revoked {
            ensure!(
                state
                    .allow_pair
                    .lock()
                    .await
                    .get(&device.id)
                    .is_some_and(|until| *until > now_ms()),
                "device is revoked; explicitly initiate pairing locally first"
            );
        }
    }
    ensure!(
        state.pairings.lock().await.len() < 4,
        "too many pairing attempts"
    );
    ensure!(
        !state
            .pairings
            .lock()
            .await
            .values()
            .any(|p| p.value.peer_id == device.id),
        "pairing already pending"
    );
    let nonce = keypair()?.private;
    conn.control(Control::Commit {
        value: commitment(&conn.session, initiator, &nonce),
    })
    .await?;
    let remote_commit = match conn.recv_control().await? {
        Control::Commit { value } if value.len() == 64 => value,
        _ => bail!("expected commitment"),
    };
    conn.control(Control::Open {
        nonce: hex::encode(&nonce),
    })
    .await?;
    let remote_nonce = match conn.recv_control().await? {
        Control::Open { nonce } => hex::decode(nonce)?,
        _ => bail!("expected nonce opening"),
    };
    ensure!(
        remote_nonce.len() == 32
            && commitment(&conn.session, !initiator, &remote_nonce) == remote_commit,
        "invalid nonce commitment"
    );
    let code = if initiator {
        security_code(&conn.session, &nonce, &remote_nonce)
    } else {
        security_code(&conn.session, &remote_nonce, &nonce)
    };
    let id = attempt.unwrap_or_else(|| Uuid::new_v4().to_string());
    let (decisions, mut receive) = mpsc::channel(2);
    state.pairings.lock().await.insert(
        id.clone(),
        PairAttempt {
            value: Pairing {
                id: id.clone(),
                peer_id: device.id.clone(),
                peer_name: device.name.clone(),
                code,
                local_confirmed: false,
                remote_confirmed: false,
                expires_at: now_ms() + 120_000,
            },
            decisions,
        },
    );
    state.changed();
    let result = timeout(Duration::from_secs(120), async {
        let mut local = false; let mut remote = false;
        while !local || !remote {
            tokio::select! {
                decision = receive.recv() => { ensure!(decision == Some(true), "pairing declined"); if !local { local = true; conn.control(Control::Approve { session: session.clone() }).await?; } }
                frame = conn.recv_control() => { match frame? { Control::Approve { session: approved } if approved == session => remote = true, Control::Deny => bail!("pairing declined"), _ => bail!("invalid pairing message") } }
            }
            if let Some(p) = state.pairings.lock().await.get_mut(&id) { p.value.local_confirmed = local; p.value.remote_confirmed = remote; } state.changed();
        }
        let gate = state.gate(&device.id).await; let _guard = gate.lock().await;
        if let Some(t) = state.store.trust(&device.id).await? { ensure!(!t.revoked || state.allow_pair.lock().await.get(&device.id).is_some_and(|until| *until > now_ms()), "device revoked during pairing"); }
        state.store.pin(device.clone(), key).await?; Ok::<_, anyhow::Error>(())
    }).await.context("pairing expired").and_then(|r| r);
    state.pairings.lock().await.remove(&id);
    state.allow_pair.lock().await.remove(&device.id);
    state.changed();
    if result.is_err() {
        let _ = conn.control(Control::Deny).await;
    }
    result
}
async fn session(
    state: Arc<State>,
    socket: TcpStream,
    initiator: bool,
    pairing: bool,
    expected: Option<String>,
    attempt: Option<String>,
) -> Result<()> {
    let mut conn = Connection::connect(socket, &state.private, initiator, pairing).await?;
    let peer_id = device_id(&conn.remote_key);
    ensure!(
        !state.denied.lock().await.contains(&peer_id),
        "device was revoked"
    );
    let local = state.device.read().await.clone();
    ensure!(peer_id != local.id, "self connection");
    if let Some(expected) = expected {
        ensure!(
            peer_id == expected,
            "discovered identity does not match Noise key"
        );
    }
    conn.control(Control::Hello {
        device: local.clone(),
    })
    .await?;
    let device = match timeout(Duration::from_secs(5), conn.recv_control()).await?? {
        Control::Hello { device } => device,
        _ => bail!("missing device metadata"),
    };
    ensure!(
        device.id == peer_id
            && !device.name.trim().is_empty()
            && device.name.chars().count() <= 80
            && device.platform.len() <= 32,
        "invalid device metadata"
    );
    if conn.pairing {
        timeout(
            Duration::from_secs(130),
            pair(&state, &mut conn, &device, initiator, attempt),
        )
        .await??;
    } else {
        let trust = state.store.trusted(&peer_id).await?;
        ensure!(
            trust.key == hex::encode(&conn.remote_key),
            "pinned key mismatch"
        );
    }
    let session_id = hex::encode(&conn.session);
    conn.control(Control::Ready {
        session: session_id.clone(),
    })
    .await?;
    ensure!(
        matches!(timeout(Duration::from_secs(10), conn.recv_control()).await??, Control::Ready { session } if session == session_id),
        "remote is not ready"
    );
    let (stop, mut cancelled) = watch::channel(false);
    let (done_tx, done_rx) = watch::channel(false);
    let preferred = initiator == (local.id < peer_id);
    {
        let gate = state.gate(&peer_id).await;
        let _guard = gate.lock().await;
        state.store.trusted(&peer_id).await?;
        let mut active = state.active.lock().await;
        if let Some(existing) = active.get(&peer_id) {
            ensure!(
                (preferred, &session_id) > (existing.preferred, &existing.session),
                "duplicate session"
            );
            let _ = existing.stop.send(true);
        }
        active.insert(
            peer_id.clone(),
            Active {
                session: session_id.clone(),
                preferred,
                stop,
                done: done_rx,
            },
        );
    }
    if let Some(obs) = state.observations.write().await.get_mut(&peer_id) {
        obs.peer.device = device;
        obs.seen = now_ms();
    }
    state.changed();
    let result = tokio::select! { _ = cancelled.changed() => Ok(()), result = ready(&state, &mut conn, &peer_id, &session_id) => result };
    drop(conn);
    {
        let gate = state.gate(&peer_id).await;
        let _guard = gate.lock().await;
        let mut active = state.active.lock().await;
        if active
            .get(&peer_id)
            .is_some_and(|a| a.session == session_id)
        {
            active.remove(&peer_id);
            drop(active);
            for mut message in state.store.messages(&peer_id).await? {
                if message.status == MessageStatus::Sending {
                    message.status = MessageStatus::Pending;
                    state.store.save_message(&message).await?;
                }
            }
            for mut t in state
                .store
                .transfers()
                .await?
                .into_iter()
                .filter(|t| t.peer_id == peer_id && t.status == TransferStatus::Transferring)
            {
                t.status = TransferStatus::Paused;
                state.store.save_transfer(&t).await?;
                if t.direction == Direction::Incoming {
                    let file = tokio::fs::OpenOptions::new()
                        .write(true)
                        .open(files::spool_path(&state, &t))
                        .await?;
                    file.set_len(t.offset).await?;
                    file.sync_all().await?;
                }
            }
        }
    }
    let _ = done_tx.send(true);
    state.changed();
    result
}
async fn ready(state: &Arc<State>, conn: &mut Connection, peer: &str, session: &str) -> Result<()> {
    let mut tick = interval(Duration::from_secs(2));
    let mut incoming_offsets = HashMap::new();
    let mut sent_chunks: HashMap<String, (u64, Instant)> = HashMap::new();
    let mut last_read = Instant::now();
    let mut verifications = tokio::task::JoinSet::new();
    let mut verifying = std::collections::HashSet::new();
    enum Wake {
        Verification(Option<std::result::Result<Result<()>, tokio::task::JoinError>>),
        Tick,
        Frame(Result<Frame>),
    }
    loop {
        let wake = tokio::select! {
            result = verifications.join_next(), if !verifications.is_empty() => Wake::Verification(result),
            _ = tick.tick() => Wake::Tick,
            _ = state.outbox.notified() => Wake::Tick,
            frame = conn.recv() => Wake::Frame(frame),
        };
        match wake {
            Wake::Verification(result) => {
                if let Some(result) = result {
                    result??;
                }
            }
            Wake::Tick => {
                ensure!(
                    last_read.elapsed() < Duration::from_secs(20),
                    "peer heartbeat timed out"
                );
                let controls = {
                    let gate = state.gate(peer).await;
                    let _guard = gate.lock().await;
                    state.authorized(peer, session).await?;
                    let mut controls = vec![Control::Ping];
                    for mut m in state.store.messages(peer).await? {
                        if matches!(m.status, MessageStatus::Pending | MessageStatus::Sending)
                            && m.direction == Direction::Outgoing
                        {
                            controls.push(Control::Chat {
                                id: m.id.clone(),
                                body: m.body.clone(),
                                created_at: m.created_at,
                            });
                            m.status = MessageStatus::Sending;
                            state.store.save_message(&m).await?;
                        }
                    }
                    for t in state
                        .store
                        .transfers()
                        .await?
                        .into_iter()
                        .filter(|t| t.peer_id == peer)
                    {
                        if t.status == TransferStatus::Cancelled {
                            controls.push(Control::Cancel { id: t.id });
                        } else if t.direction == Direction::Outgoing && !t.status.terminal() {
                            controls.push(Control::Offer {
                                id: t.id,
                                name: t.name,
                                size: t.size,
                                sha256: t.sha256,
                                created_at: t.created_at,
                                retry: t.status == TransferStatus::AwaitingAccept,
                            });
                        } else if t.direction == Direction::Incoming
                            && t.status != TransferStatus::Offered
                        {
                            if matches!(
                                t.status,
                                TransferStatus::Transferring | TransferStatus::Paused
                            ) && t.offset == t.size
                                && verifying.insert(t.id.clone())
                            {
                                verifications.spawn(verify_transfer(
                                    state.clone(),
                                    peer.to_owned(),
                                    session.to_owned(),
                                    t.clone(),
                                ));
                            }
                            controls.push(transfer_state(&t));
                        }
                    }
                    controls
                };
                for control in controls {
                    conn.control(control).await?;
                }
            }
            Wake::Frame(frame) => {
                last_read = Instant::now();
                let mut response = None;
                let mut send = None;
                let mut verify = None;
                {
                    let gate = state.gate(peer).await;
                    let _guard = gate.lock().await;
                    state.authorized(peer, session).await?;
                    match frame? {
                        Frame::Control(Control::Ping) => {}
                        Frame::Control(Control::Chat {
                            id,
                            body,
                            created_at,
                        }) => {
                            ensure!(Uuid::parse_str(&id)?.to_string() == id, "noncanonical UUID");
                            ensure!(
                                !body.trim().is_empty() && body.len() <= MAX_MESSAGE_SIZE,
                                "invalid message"
                            );
                            state
                                .store
                                .insert_message(&Message {
                                    id: id.clone(),
                                    peer_id: peer.into(),
                                    direction: Direction::Incoming,
                                    body,
                                    created_at,
                                    status: MessageStatus::Sent,
                                })
                                .await?;
                            response = Some(Control::ChatAck { id });
                            state.changed();
                        }
                        Frame::Control(Control::ChatAck { id }) => {
                            let mut m = state.store.message(&id).await?;
                            ensure!(
                                m.peer_id == peer && m.direction == Direction::Outgoing,
                                "unexpected message ACK"
                            );
                            m.status = MessageStatus::Sent;
                            state.store.save_message(&m).await?;
                            state.changed();
                        }
                        Frame::Control(Control::Offer {
                            id,
                            name,
                            size,
                            sha256,
                            created_at,
                            retry,
                        }) => {
                            ensure!(Uuid::parse_str(&id)?.to_string() == id, "noncanonical UUID");
                            ensure!(
                                size <= state.settings.read().await.max_file_size
                                    && valid_id(&sha256)
                                    && name.chars().count() <= 120,
                                "invalid file offer"
                            );
                            let offer = Transfer {
                                id,
                                peer_id: peer.into(),
                                direction: Direction::Incoming,
                                name: safe_name(&name),
                                size,
                                sha256,
                                offset: 0,
                                status: TransferStatus::Offered,
                                error: None,
                                created_at,
                            };
                            let mut t = state.store.receive_offer(&offer).await?;
                            if retry && t.status == TransferStatus::Failed {
                                files::remove_partial(state, &t).await?;
                                t.status = TransferStatus::Offered;
                                t.offset = 0;
                                t.error = None;
                                state.store.save_transfer(&t).await?;
                                incoming_offsets.remove(&t.id);
                                verifying.remove(&t.id);
                            }
                            response = Some(transfer_state(&t));
                            state.changed();
                        }
                        Frame::Control(Control::TransferState { id, status, offset }) => {
                            let mut t = state.store.transfer(&id).await?;
                            ensure!(
                                t.peer_id == peer
                                    && t.direction == Direction::Outgoing
                                    && offset <= t.size,
                                "unexpected transfer status"
                            );
                            if !t.status.terminal() {
                                match status {
                                    TransferStatus::Transferring | TransferStatus::Paused => {
                                        ensure!(
                                            offset == t.size || offset % CHUNK_SIZE as u64 == 0,
                                            "invalid resume offset"
                                        );
                                        t.offset = offset;
                                        t.status = TransferStatus::Transferring;
                                        state.store.save_transfer(&t).await?;
                                        if !sent_chunks.get(&id).is_some_and(|(until, when)| {
                                            *until > offset
                                                && when.elapsed() < Duration::from_secs(15)
                                        }) && (offset < t.size || t.size == 0)
                                        {
                                            send = Some(t);
                                        }
                                    }
                                    TransferStatus::Completed => {
                                        ensure!(offset == t.size, "incomplete completion receipt");
                                        t.offset = offset;
                                        t.status = status;
                                        state.store.save_transfer(&t).await?;
                                        files::remove_partial(state, &t).await?;
                                    }
                                    TransferStatus::Rejected
                                    | TransferStatus::Cancelled
                                    | TransferStatus::Failed => {
                                        t.status = status;
                                        sent_chunks.remove(&t.id);
                                        state.store.save_transfer(&t).await?;
                                        if status != TransferStatus::Failed {
                                            files::remove_partial(state, &t).await?;
                                        }
                                    }
                                    TransferStatus::Offered | TransferStatus::AwaitingAccept => {}
                                }
                                state.changed();
                            }
                        }
                        Frame::Control(Control::Cancel { id }) => {
                            if let Ok(mut t) = state.store.transfer(&id).await {
                                ensure!(t.peer_id == peer, "wrong cancellation peer");
                                if !t.status.terminal() {
                                    t.status = TransferStatus::Cancelled;
                                    state.store.save_transfer(&t).await?;
                                    files::remove_partial(state, &t).await?;
                                    state.changed();
                                }
                            }
                        }
                        Frame::Data {
                            id,
                            offset,
                            end,
                            bytes,
                        } => {
                            let mut t = state.store.transfer(&id.to_string()).await?;
                            ensure!(t.peer_id == peer, "wrong transfer peer");
                            ensure!(
                                t.direction == Direction::Incoming,
                                "file data targeted an outgoing transfer"
                            );
                            if t.status.terminal()
                                || offset < t.offset
                                || (t.offset == t.size && verifying.contains(&t.id))
                            {
                                response = Some(transfer_state(&t));
                            } else {
                                let volatile =
                                    incoming_offsets.entry(t.id.clone()).or_insert(t.offset);
                                match files::receive_fragment(
                                    state, &mut t, offset, end, &bytes, volatile,
                                )
                                .await
                                {
                                    Ok(true) => {
                                        if t.offset == t.size {
                                            verify = Some(t);
                                        } else {
                                            response = Some(transfer_state(&t));
                                        }
                                        state.changed();
                                    }
                                    Ok(false) => {}
                                    Err(e) => {
                                        t.status = TransferStatus::Failed;
                                        t.error = Some(e.to_string());
                                        state.store.save_transfer(&t).await?;
                                        files::remove_partial(state, &t).await?;
                                        response = Some(transfer_state(&t));
                                        state.changed();
                                    }
                                }
                            }
                        }
                        _ => bail!("invalid application frame"),
                    }
                }
                if let Some(t) = verify
                    && verifying.insert(t.id.clone())
                {
                    verifications.spawn(verify_transfer(
                        state.clone(),
                        peer.to_owned(),
                        session.to_owned(),
                        t,
                    ));
                }
                if let Some(response) = response {
                    conn.control(response).await?;
                }
                if let Some(t) = send {
                    if !sent_chunks.contains_key(&t.id) {
                        tracing::info!(transfer_id=%t.id, offset=t.offset, "transfer stream starting");
                    }
                    let until = send_chunk(state, conn, &t).await?;
                    sent_chunks.insert(t.id, (until, Instant::now()));
                }
            }
        }
    }
}
async fn verify_transfer(
    state: Arc<State>,
    peer: String,
    session: String,
    t: Transfer,
) -> Result<()> {
    let hash = files::hash_file(&files::spool_path(&state, &t)).await;
    let gate = state.gate(&peer).await;
    let _guard = gate.lock().await;
    state.authorized(&peer, &session).await?;
    let mut current = state.store.transfer(&t.id).await?;
    if current.status.terminal() {
        return Ok::<(), anyhow::Error>(());
    }
    let result = match hash {
        Ok(hash) if hash == t.sha256 => files::publish(&state, &mut current).await,
        Ok(_) => Err(anyhow::anyhow!("SHA-256 mismatch")),
        Err(error) => Err(error),
    };
    if let Err(error) = result {
        current.status = TransferStatus::Failed;
        current.error = Some(error.to_string());
        state.store.save_transfer(&current).await?;
        files::remove_partial(&state, &current).await?;
    }
    state.outbox.notify_waiters();
    state.changed();
    Ok(())
}
fn transfer_state(t: &Transfer) -> Control {
    Control::TransferState {
        id: t.id.clone(),
        status: t.status,
        offset: t.offset,
    }
}
async fn send_chunk(state: &State, conn: &mut Connection, t: &Transfer) -> Result<u64> {
    let mut file = tokio::fs::File::open(files::spool_path(state, t)).await?;
    file.seek(std::io::SeekFrom::Start(t.offset)).await?;
    let len = ((t.size - t.offset) as usize).min(CHUNK_SIZE);
    let mut bytes = vec![0u8; len];
    file.read_exact(&mut bytes).await?;
    let id = Uuid::parse_str(&t.id)?;
    if len == 0 {
        conn.send(Frame::Data {
            id,
            offset: t.offset,
            end: true,
            bytes,
        })
        .await?;
    } else {
        for (i, part) in bytes.chunks(FRAGMENT_SIZE).enumerate() {
            if state.store.transfer(&t.id).await?.status.terminal() {
                break;
            }
            let offset = t.offset + (i * FRAGMENT_SIZE) as u64;
            conn.send(Frame::Data {
                id,
                offset,
                end: offset + part.len() as u64 == t.offset + len as u64,
                bytes: part.to_vec(),
            })
            .await?;
        }
    }
    Ok(t.offset + len as u64)
}

pub async fn add_manual(state: &State, value: &str) -> Result<()> {
    let (id, address) = value
        .split_once('@')
        .context("--peer requires id@ip:port")?;
    ensure!(valid_id(id), "invalid peer ID");
    let address: std::net::SocketAddr = address.parse()?;
    state.observations.write().await.insert(
        id.into(),
        Observation {
            peer: Peer {
                device: Device {
                    id: id.into(),
                    name: format!("Device {}", &id[..8]),
                    platform: "unknown".into(),
                },
                addresses: vec![address.ip().to_string()],
                port: address.port(),
                online: true,
                paired: false,
            },
            seen: now_ms(),
        },
    );
    Ok(())
}

pub fn local_ip(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(ip) => ip.is_private() || ip.is_loopback() || ip.is_link_local(),
        std::net::IpAddr::V6(ip) => {
            ip.is_loopback() || ip.is_unique_local() || ip.is_unicast_link_local()
        }
    }
}
