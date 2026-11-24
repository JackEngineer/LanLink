use crate::{
    files,
    state::{Active, Observation, PairAttempt, PairGrant, State},
};
use anyhow::{Context, Result, bail, ensure};
use lanlink_protocol::*;
use lanlink_transport::{Connection, commitment, device_id, keypair, security_code};
use std::{collections::HashMap, sync::Arc};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::{OwnedSemaphorePermit, mpsc, watch},
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
            if let Err(e) = session(state, socket, false, false, None, None, permit).await {
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
        let permit = state
            .handshakes
            .clone()
            .try_acquire_owned()
            .context("too many connection handshakes")?;
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
                        permit,
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
struct PendingPairing {
    state: Arc<State>,
    id: String,
    peer: String,
    grant: Option<PairGrant>,
    armed: bool,
}
impl PendingPairing {
    async fn clear(&mut self) {
        clear_pairing(&self.state, &self.id, &self.peer, self.grant.as_ref()).await;
        self.armed = false;
    }
}
impl Drop for PendingPairing {
    fn drop(&mut self) {
        if self.armed {
            let state = self.state.clone();
            let id = self.id.clone();
            let peer = self.peer.clone();
            let grant = self.grant.clone();
            tokio::spawn(async move {
                clear_pairing(&state, &id, &peer, grant.as_ref()).await;
            });
        }
    }
}
async fn clear_pairing(state: &State, id: &str, peer: &str, grant: Option<&PairGrant>) {
    let mut allowed = state.allow_pair.lock().await;
    if allowed.get(peer) == grant {
        allowed.remove(peer);
    }
    drop(allowed);
    state.pairings.lock().await.remove(id);
    state.changed();
}
async fn pair(
    state: &Arc<State>,
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
    let gate = state.gate(&device.id).await;
    let (trust_epoch, revoked, grant) = {
        let _guard = gate.lock().await;
        ensure!(
            !state.denied.lock().await.contains(&device.id),
            "device was revoked"
        );
        let trust = state.store.trust(&device.id).await?;
        if let Some(trust) = &trust {
            ensure!(trust.key == key, "pinned identity changed");
        }
        let revoked = trust.as_ref().is_some_and(|t| t.revoked);
        let grant = state.allow_pair.lock().await.get(&device.id).cloned();
        ensure!(
            !revoked
                || grant.as_ref().is_some_and(|g| {
                    g.expires_at > now_ms() && attempt.as_ref().is_none_or(|id| g.attempt == *id)
                }),
            "device is revoked; explicitly initiate pairing locally first"
        );
        (trust.map(|t| t.epoch), revoked, grant)
    };
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
    let remote_nonce = timeout(Duration::from_secs(10), async {
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
        Ok::<_, anyhow::Error>(remote_nonce)
    })
    .await
    .context("pairing commitment exchange timed out")??;
    let code = if initiator {
        security_code(&conn.session, &nonce, &remote_nonce)
    } else {
        security_code(&conn.session, &remote_nonce, &nonce)
    };
    let id = attempt.unwrap_or_else(|| Uuid::new_v4().to_string());
    let (decisions, mut receive) = mpsc::channel(2);
    let mut pending = PendingPairing {
        state: state.clone(),
        id: id.clone(),
        peer: device.id.clone(),
        grant: grant.clone(),
        armed: true,
    };
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
        let _guard = gate.lock().await;
        let current_trust = state.store.trust(&device.id).await?;
        ensure!(
            !state.denied.lock().await.contains(&device.id)
                && current_trust.map(|t| t.epoch) == trust_epoch,
            "device authorization changed during pairing"
        );
        if revoked {
            ensure!(
                state.allow_pair.lock().await.get(&device.id) == grant.as_ref()
                    && grant.as_ref().is_some_and(|g| g.expires_at > now_ms()),
                "re-pair authorization changed during pairing"
            );
        }
        state.store.pin(device.clone(), key).await?; Ok::<_, anyhow::Error>(())
    }).await.context("pairing expired").and_then(|r| r);
    pending.clear().await;
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
    handshake_permit: OwnedSemaphorePermit,
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
        pair(&state, &mut conn, &device, initiator, attempt).await?;
    } else {
        let trust = state.store.trusted(&peer_id).await?;
        ensure!(
            trust.key == hex::encode(&conn.remote_key),
            "pinned key mismatch"
        );
    }
    let _session_permit = state
        .sessions
        .clone()
        .try_acquire_owned()
        .context("too many authenticated sessions")?;
    let session_id = hex::encode(&conn.session);
    conn.control(Control::Ready {
        session: session_id.clone(),
    })
    .await?;
    ensure!(
        matches!(timeout(Duration::from_secs(10), conn.recv_control()).await??, Control::Ready { session } if session == session_id),
        "remote is not ready"
    );
    drop(handshake_permit);
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
    let mut cleaned_cancellations = std::collections::HashSet::new();
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
                            let control = m.control();
                            if Frame::Control(control.clone()).encode().is_err() {
                                m.status = MessageStatus::Failed;
                                state.store.save_message(&m).await?;
                                state.changed();
                                continue;
                            }
                            controls.push(control);
                            m.status = MessageStatus::Sending;
                            state.store.save_message(&m).await?;
                        }
                    }
                    for mut t in state
                        .store
                        .transfers()
                        .await?
                        .into_iter()
                        .filter(|t| t.peer_id == peer)
                    {
                        if t.status == TransferStatus::Cancelled || files::source_failed(&t) {
                            controls.push(Control::Cancel { id: t.id });
                        } else if t.direction == Direction::Outgoing && !t.status.terminal() {
                            if let Err(error) = files::open_source(state, &t).await {
                                files::fail_source(state, &mut t, error).await?;
                                controls.push(Control::Cancel { id: t.id });
                                continue;
                            }
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
                            ) {
                                let settings = state.settings.read().await;
                                files::enforce_incoming_limit(state, &mut t, &settings).await?;
                            }
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
                                valid_id(&sha256) && name.chars().count() <= 120,
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
                                failure_kind: None,
                                error: None,
                                created_at,
                            };
                            let settings = state.settings.read().await;
                            if let Some((mut t, reopened)) = state
                                .store
                                .receive_offer(&offer, retry, settings.max_file_size)
                                .await?
                            {
                                files::enforce_incoming_limit(state, &mut t, &settings).await?;
                                if reopened {
                                    files::remove_partial(state, &t).await?;
                                    incoming_offsets.remove(&t.id);
                                    verifying.remove(&t.id);
                                }
                                response = Some(transfer_state(&t));
                                state.changed();
                            } else {
                                response = Some(Control::TransferState {
                                    id: offer.id,
                                    status: TransferStatus::Rejected,
                                    offset: 0,
                                });
                            }
                        }
                        Frame::Control(Control::TransferState { id, status, offset }) => {
                            let mut t = state.store.transfer(&id).await?;
                            ensure!(
                                t.peer_id == peer
                                    && t.direction == Direction::Outgoing
                                    && offset <= t.size,
                                "unexpected transfer status"
                            );
                            if !t.status.terminal()
                                || (status == TransferStatus::Completed && files::source_failed(&t))
                            {
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
                                        t.failure_kind = None;
                                        t.error = None;
                                        state.store.save_transfer(&t).await?;
                                        files::remove_partial(state, &t).await?;
                                    }
                                    TransferStatus::Rejected
                                    | TransferStatus::Cancelled
                                    | TransferStatus::Failed => {
                                        if status != TransferStatus::Failed {
                                            files::remove_partial(state, &t).await?;
                                        }
                                        t.status = status;
                                        sent_chunks.remove(&t.id);
                                        state.store.save_transfer(&t).await?;
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
                                    state.changed();
                                }
                                if t.status == TransferStatus::Cancelled
                                    && cleaned_cancellations.insert(id)
                                {
                                    files::remove_partial(state, &t).await?;
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
                            let settings = state.settings.read().await;
                            files::enforce_incoming_limit(state, &mut t, &settings).await?;
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
                    let bytes = match files::read_source_chunk(state, &t).await {
                        Ok(bytes) => bytes,
                        Err(error) => {
                            let source_failed = {
                                let gate = state.gate(peer).await;
                                let _guard = gate.lock().await;
                                state.authorized(peer, session).await?;
                                let mut current = state.store.transfer(&t.id).await?;
                                if !current.status.terminal() {
                                    files::fail_source(state, &mut current, error).await?;
                                }
                                files::source_failed(&current)
                            };
                            if source_failed {
                                conn.control(Control::Cancel { id: t.id.clone() }).await?;
                            }
                            sent_chunks.remove(&t.id);
                            continue;
                        }
                    };
                    let until = send_chunk(state, conn, &t, bytes).await?;
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
    let settings = state.settings.read().await;
    files::enforce_incoming_limit(&state, &mut current, &settings).await?;
    if current.status.terminal() {
        return Ok::<(), anyhow::Error>(());
    }
    let result = match hash {
        Ok(hash) if hash == t.sha256 => files::publish(&state, &mut current, &settings).await,
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
async fn send_chunk(
    state: &State,
    conn: &mut Connection,
    t: &Transfer,
    bytes: Vec<u8>,
) -> Result<u64> {
    let len = bytes.len();
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::test_state;
    use tokio::sync::Semaphore;

    async fn connections(state: Arc<State>) -> Result<(Connection, Connection, Device)> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let keys = keypair()?;
        let device = Device {
            id: device_id(&keys.public),
            name: "peer".into(),
            platform: "test".into(),
        };
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await?;
            Connection::connect(socket, &state.private, false, false).await
        });
        let client = Connection::connect(
            TcpStream::connect(address).await?,
            &keys.private,
            true,
            true,
        )
        .await?;
        Ok((server.await??, client, device))
    }

    async fn exchange_pairing_commitments(client: &mut Connection) -> Result<()> {
        assert!(matches!(
            client.recv_control().await?,
            Control::Commit { .. }
        ));
        let nonce = keypair()?.private;
        client
            .control(Control::Commit {
                value: commitment(&client.session, true, &nonce),
            })
            .await?;
        assert!(matches!(client.recv_control().await?, Control::Open { .. }));
        client
            .control(Control::Open {
                nonce: hex::encode(nonce),
            })
            .await?;
        Ok(())
    }

    #[tokio::test]
    async fn cancelling_pending_pairing_releases_confirmation_state() -> Result<()> {
        let root = tempfile::tempdir()?;
        let state = Arc::new(test_state(root.path()).await?);
        let (mut server, mut client, device) = connections(state.clone()).await?;
        state.allow_pair.lock().await.insert(
            device.id.clone(),
            PairGrant {
                attempt: "attempt".into(),
                expires_at: now_ms() + 120_000,
            },
        );
        let child_state = state.clone();
        let child_device = device.clone();
        let task = tokio::spawn(async move {
            pair(
                &child_state,
                &mut server,
                &child_device,
                false,
                Some("attempt".into()),
            )
            .await
        });
        exchange_pairing_commitments(&mut client).await?;
        timeout(Duration::from_secs(2), async {
            while !state.pairings.lock().await.contains_key("attempt") {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await?;
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        timeout(Duration::from_secs(2), async {
            while !state.pairings.lock().await.is_empty()
                || state.allow_pair.lock().await.contains_key(&device.id)
            {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await?;
        assert_eq!(state.pairing_slots.available_permits(), 4);
        assert!(state.store.trust(&device.id).await?.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn stale_pairing_approvals_cannot_use_a_fresh_grant() -> Result<()> {
        for revoke_during_confirmation in [true, false] {
            let root = tempfile::tempdir()?;
            let state = Arc::new(test_state(root.path()).await?);
            let (mut server, mut client, device) = connections(state.clone()).await?;
            state
                .store
                .pin(device.clone(), hex::encode(&server.remote_key))
                .await?;
            if !revoke_during_confirmation {
                state.store.revoke(&device.id).await?;
            }
            let expires_at = now_ms() + 120_000;
            state.allow_pair.lock().await.insert(
                device.id.clone(),
                PairGrant {
                    attempt: "old".into(),
                    expires_at,
                },
            );
            let local_state = state.clone();
            let local_device = device.clone();
            let pairing = tokio::spawn(async move {
                pair(
                    &local_state,
                    &mut server,
                    &local_device,
                    false,
                    Some("old".into()),
                )
                .await
            });
            exchange_pairing_commitments(&mut client).await?;
            let decisions = timeout(Duration::from_secs(2), async {
                loop {
                    if let Some(attempt) = state.pairings.lock().await.get("old") {
                        break attempt.decisions.clone();
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await?;
            let gate = state.gate(&device.id).await;
            let guard = gate.lock().await;
            decisions.send(true).await?;
            let session = hex::encode(&client.session);
            assert!(matches!(
                client.recv_control().await?,
                Control::Approve { .. }
            ));
            client.control(Control::Approve { session }).await?;
            timeout(Duration::from_secs(2), async {
                loop {
                    let confirmed = state
                        .pairings
                        .lock()
                        .await
                        .get("old")
                        .is_some_and(|attempt| {
                            attempt.value.local_confirmed && attempt.value.remote_confirmed
                        });
                    if confirmed {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await?;
            if revoke_during_confirmation {
                state.store.revoke(&device.id).await?;
            }
            let fresh = PairGrant {
                attempt: "fresh".into(),
                expires_at,
            };
            state
                .allow_pair
                .lock()
                .await
                .insert(device.id.clone(), fresh.clone());
            drop(guard);
            assert!(matches!(client.recv_control().await?, Control::Deny));
            assert!(pairing.await?.is_err());
            assert!(state.store.trust(&device.id).await?.unwrap().revoked);
            assert!(state.allow_pair.lock().await.get(&device.id) == Some(&fresh));
        }
        Ok(())
    }

    async fn authenticated_client(
        state: &State,
        address: std::net::SocketAddr,
    ) -> Result<Connection> {
        let keys = keypair()?;
        let device = Device {
            id: device_id(&keys.public),
            name: "peer".into(),
            platform: "test".into(),
        };
        state
            .store
            .pin(device.clone(), hex::encode(&keys.public))
            .await?;
        let mut conn = Connection::connect(
            TcpStream::connect(address).await?,
            &keys.private,
            true,
            false,
        )
        .await?;
        conn.control(Control::Hello { device }).await?;
        assert!(matches!(conn.recv_control().await?, Control::Hello { .. }));
        let session = hex::encode(&conn.session);
        ensure!(
            matches!(conn.recv_control().await?, Control::Ready { session: value } if value == session),
            "missing readiness"
        );
        conn.control(Control::Ready { session }).await?;
        Ok(conn)
    }

    #[tokio::test]
    async fn authenticated_sessions_release_handshake_slots_and_have_separate_limit() -> Result<()>
    {
        let root = tempfile::tempdir()?;
        let mut state = test_state(root.path()).await?;
        state.handshakes = Arc::new(Semaphore::new(2));
        state.sessions = Arc::new(Semaphore::new(5));
        let state = Arc::new(state);
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let task = tokio::spawn(listen(state.clone(), listener));
        let mut clients = Vec::new();
        for _ in 0..5 {
            clients.push(
                timeout(
                    Duration::from_secs(2),
                    authenticated_client(&state, address),
                )
                .await??,
            );
        }
        timeout(Duration::from_secs(2), async {
            while state.active.lock().await.len() != 5 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await?;
        assert_eq!(state.handshakes.available_permits(), 2);
        assert_eq!(state.sessions.available_permits(), 0);
        assert!(
            timeout(
                Duration::from_secs(2),
                authenticated_client(&state, address)
            )
            .await?
            .is_err()
        );
        assert_eq!(state.active.lock().await.len(), 5);
        drop(clients.pop());
        timeout(Duration::from_secs(2), async {
            while state.sessions.available_permits() == 0 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await?;
        clients.push(
            timeout(
                Duration::from_secs(2),
                authenticated_client(&state, address),
            )
            .await??,
        );
        task.abort();
        drop(clients);
        Ok(())
    }

    #[tokio::test]
    async fn invalid_outbox_and_lost_source_do_not_disconnect_healthy_session() -> Result<()> {
        let root = tempfile::tempdir()?;
        let state = Arc::new(test_state(root.path()).await?);
        let (mut server, mut client, device) = connections(state.clone()).await?;
        let session = hex::encode(&server.session);
        state
            .store
            .pin(device.clone(), hex::encode(&server.remote_key))
            .await?;
        let (stop, _) = watch::channel(false);
        let (_, done) = watch::channel(false);
        state.active.lock().await.insert(
            device.id.clone(),
            Active {
                session: session.clone(),
                preferred: true,
                stop,
                done,
            },
        );
        let invalid = Message {
            id: Uuid::new_v4().to_string(),
            peer_id: device.id.clone(),
            direction: Direction::Outgoing,
            body: "\u{1}".repeat(MAX_MESSAGE_SIZE),
            created_at: now_ms(),
            status: MessageStatus::Pending,
        };
        state.store.insert_message(&invalid).await?;
        let good = Message {
            id: Uuid::new_v4().to_string(),
            body: "valid message".into(),
            ..invalid.clone()
        };
        state.store.insert_message(&good).await?;
        let source = Transfer {
            id: Uuid::new_v4().to_string(),
            peer_id: device.id.clone(),
            direction: Direction::Outgoing,
            name: "test.txt".into(),
            size: 3,
            sha256: "a".repeat(64),
            offset: 0,
            status: TransferStatus::AwaitingAccept,
            failure_kind: None,
            error: None,
            created_at: now_ms(),
        };
        state.store.insert_transfer(&source).await?;
        tokio::fs::write(files::spool_path(&state, &source), b"abc").await?;
        let child_state = state.clone();
        let peer = device.id.clone();
        let task =
            tokio::spawn(async move { ready(&child_state, &mut server, &peer, &session).await });
        timeout(Duration::from_secs(5), async {
            loop {
                match client.recv_control().await? {
                    Control::Chat { id, .. } => {
                        assert_eq!(id, good.id);
                        client.control(Control::ChatAck { id }).await?;
                    }
                    Control::Offer { id, .. } => {
                        assert_eq!(id, source.id);
                        break;
                    }
                    Control::Ping => {}
                    other => bail!("unexpected control {other:?}"),
                }
            }
            Ok::<_, anyhow::Error>(())
        })
        .await
        .context("timed out waiting for the valid queued message or file offer")??;
        tokio::fs::write(files::spool_path(&state, &source), b"x").await?;
        client
            .control(Control::TransferState {
                id: source.id.clone(),
                status: TransferStatus::Transferring,
                offset: 0,
            })
            .await?;
        let incoming = Uuid::new_v4().to_string();
        client
            .control(Control::Chat {
                id: incoming.clone(),
                body: "still connected".into(),
                created_at: now_ms(),
            })
            .await?;
        timeout(Duration::from_secs(5), async {
            let mut cancelled = false;
            loop {
                match client.recv_control().await? {
                    Control::Cancel { id } if id == source.id => cancelled = true,
                    Control::ChatAck { id } if id == incoming => {
                        assert!(cancelled);
                        break;
                    }
                    _ => {}
                }
            }
            Ok::<_, anyhow::Error>(())
        })
        .await
        .context("timed out waiting for the lost-source cancel or follow-up chat ACK")??;
        assert_eq!(
            state.store.message(&invalid.id).await?.status,
            MessageStatus::Failed
        );
        assert_eq!(
            state.store.message(&good.id).await?.status,
            MessageStatus::Sent
        );
        assert_eq!(
            state.store.message(&incoming).await?.body,
            "still connected"
        );
        let failed = state.store.transfer(&source.id).await?;
        assert_eq!(failed.status, TransferStatus::Failed);
        assert!(failed.error.unwrap().contains("upload the file again"));
        assert!(!task.is_finished());
        task.abort();
        Ok(())
    }

    async fn paired_states(
        sender_root: &std::path::Path,
        receiver_root: &std::path::Path,
    ) -> Result<(State, State, Device, Device)> {
        let mut sender = test_state(sender_root).await?;
        let mut receiver = test_state(receiver_root).await?;
        let sender_key = keypair()?;
        let receiver_key = keypair()?;
        sender.private = sender_key.private;
        receiver.private = receiver_key.private;
        sender.device.get_mut().id = device_id(&sender_key.public);
        receiver.device.get_mut().id = device_id(&receiver_key.public);
        let sender_device = sender.device.get_mut().clone();
        let receiver_device = receiver.device.get_mut().clone();
        sender
            .store
            .pin(receiver_device.clone(), hex::encode(receiver_key.public))
            .await?;
        receiver
            .store
            .pin(sender_device.clone(), hex::encode(sender_key.public))
            .await?;
        Ok((sender, receiver, sender_device, receiver_device))
    }

    async fn connect_to_test_peer(
        state: Arc<State>,
        address: std::net::SocketAddr,
        peer: String,
    ) -> Result<()> {
        let permit = state.handshakes.clone().acquire_owned().await?;
        let socket = TcpStream::connect(address).await?;
        session(state, socket, true, false, Some(peer), None, permit).await
    }

    #[tokio::test]
    async fn lost_source_cancellation_replays_after_restart() -> Result<()> {
        let sender_root = tempfile::tempdir()?;
        let receiver_root = tempfile::tempdir()?;
        let (sender, receiver, sender_device, receiver_device) =
            paired_states(sender_root.path(), receiver_root.path()).await?;

        let chunk = CHUNK_SIZE as u64;
        let source = Transfer {
            id: Uuid::new_v4().to_string(),
            peer_id: receiver_device.id.clone(),
            direction: Direction::Outgoing,
            name: "lost.txt".into(),
            size: 2 * chunk,
            sha256: "a".repeat(64),
            offset: chunk,
            status: TransferStatus::Paused,
            failure_kind: None,
            error: None,
            created_at: now_ms(),
        };
        sender.store.insert_transfer(&source).await?;
        let partial = Transfer {
            peer_id: sender_device.id.clone(),
            direction: Direction::Incoming,
            ..source.clone()
        };
        receiver.store.reserve(&partial).await?;
        let partial_path = files::spool_path(&receiver, &partial);
        tokio::fs::File::create(&partial_path)
            .await?
            .set_len(chunk)
            .await?;

        let mut lost_receipt = source.clone();
        lost_receipt.id = Uuid::new_v4().to_string();
        sender.store.insert_transfer(&lost_receipt).await?;
        let completed = Transfer {
            id: lost_receipt.id.clone(),
            offset: source.size,
            status: TransferStatus::Completed,
            ..partial.clone()
        };
        receiver.store.insert_transfer(&completed).await?;
        tokio::fs::write(
            files::download_path(&receiver, &completed).await,
            b"published",
        )
        .await?;
        files::recover(&sender).await?;
        assert!(files::source_failed(
            &sender.store.transfer(&source.id).await?
        ));
        assert!(files::source_failed(
            &sender.store.transfer(&lost_receipt.id).await?
        ));

        let mut healthy = source.clone();
        healthy.id = Uuid::new_v4().to_string();
        healthy.name = "healthy.txt".into();
        healthy.size = 7;
        healthy.offset = 0;
        healthy.status = TransferStatus::AwaitingAccept;
        let healthy_path = files::spool_path(&sender, &healthy);
        tokio::fs::write(&healthy_path, b"healthy").await?;
        healthy.sha256 = files::hash_file(&healthy_path).await?;
        sender.store.insert_transfer(&healthy).await?;
        let message = Message {
            id: Uuid::new_v4().to_string(),
            peer_id: receiver_device.id,
            direction: Direction::Outgoing,
            body: "after reconnect".into(),
            created_at: now_ms(),
            status: MessageStatus::Pending,
        };
        sender.store.insert_message(&message).await?;

        let sender = Arc::new(sender);
        let receiver = Arc::new(receiver);
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let listen_task = tokio::spawn(listen(sender.clone(), listener));
        let receiver_task = tokio::spawn(connect_to_test_peer(
            receiver.clone(),
            address,
            sender_device.id,
        ));
        timeout(Duration::from_secs(5), async {
            loop {
                let cancelled =
                    receiver.store.transfer(&partial.id).await?.status == TransferStatus::Cancelled;
                let reconciled = sender.store.transfer(&lost_receipt.id).await?.status
                    == TransferStatus::Completed;
                let delivered =
                    sender.store.message(&message.id).await?.status == MessageStatus::Sent;
                if cancelled
                    && reconciled
                    && delivered
                    && receiver.store.transfer(&healthy.id).await.is_ok()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Ok::<_, anyhow::Error>(())
        })
        .await??;
        assert!(files::source_failed(
            &sender.store.transfer(&source.id).await?
        ));
        assert!(!tokio::fs::try_exists(partial_path).await?);
        assert_eq!(
            receiver.store.message(&message.id).await?.body,
            message.body
        );
        receiver.transfer_action(&healthy.id, "accept").await?;
        timeout(Duration::from_secs(5), async {
            while sender.store.transfer(&healthy.id).await?.status != TransferStatus::Completed {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Ok::<_, anyhow::Error>(())
        })
        .await??;
        let downloaded = receiver.store.transfer(&healthy.id).await?;
        assert_eq!(downloaded.status, TransferStatus::Completed);
        assert_eq!(
            tokio::fs::read(files::download_path(&receiver, &downloaded).await).await?,
            b"healthy"
        );
        receiver_task.abort();
        listen_task.abort();
        Ok(())
    }

    #[tokio::test]
    async fn oversized_offer_rejects_transfer_and_keeps_the_session() -> Result<()> {
        let sender_root = tempfile::tempdir()?;
        let receiver_root = tempfile::tempdir()?;
        let (sender, mut receiver, sender_device, receiver_device) =
            paired_states(sender_root.path(), receiver_root.path()).await?;

        let chunk = CHUNK_SIZE as u64;
        let oversized = Transfer {
            id: Uuid::new_v4().to_string(),
            peer_id: receiver_device.id.clone(),
            direction: Direction::Outgoing,
            name: "oversized.bin".into(),
            size: chunk + 1,
            sha256: "a".repeat(64),
            offset: 0,
            status: TransferStatus::AwaitingAccept,
            failure_kind: None,
            error: None,
            created_at: now_ms(),
        };
        let paused = Transfer {
            id: Uuid::new_v4().to_string(),
            offset: chunk,
            status: TransferStatus::Paused,
            ..oversized.clone()
        };
        let lost_receipt = Transfer {
            id: Uuid::new_v4().to_string(),
            ..oversized.clone()
        };
        for outgoing in [&oversized, &paused, &lost_receipt] {
            sender.store.insert_transfer(outgoing).await?;
            tokio::fs::File::create(files::spool_path(&sender, outgoing))
                .await?
                .set_len(outgoing.size)
                .await?;
        }
        let partial = Transfer {
            peer_id: sender_device.id.clone(),
            direction: Direction::Incoming,
            ..paused.clone()
        };
        let partial_path = files::spool_path(&receiver, &partial);
        receiver.store.reserve(&partial).await?;
        tokio::fs::File::create(&partial_path)
            .await?
            .set_len(chunk)
            .await?;
        let completed = Transfer {
            id: lost_receipt.id.clone(),
            offset: lost_receipt.size,
            status: TransferStatus::Completed,
            ..partial.clone()
        };
        receiver.store.insert_transfer(&completed).await?;
        let destination = files::download_path(&receiver, &completed).await;
        tokio::fs::write(&destination, b"published").await?;
        receiver.settings.get_mut().max_file_size = chunk;

        let sender = Arc::new(sender);
        let receiver = Arc::new(receiver);
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let listen_task = tokio::spawn(listen(sender.clone(), listener));
        let mut receiver_task = tokio::spawn(connect_to_test_peer(
            receiver.clone(),
            address,
            sender_device.id,
        ));
        tokio::select! {
            result = &mut receiver_task => panic!("oversized offer ended the session: {result:?}"),
            result = timeout(Duration::from_secs(5), async {
                while sender.store.transfer(&oversized.id).await?.status != TransferStatus::Rejected
                    || sender.store.transfer(&paused.id).await?.status != TransferStatus::Rejected
                    || sender.store.transfer(&lost_receipt.id).await?.status != TransferStatus::Completed
                {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                assert!(!receiver.store.transfer_exists(&oversized.id).await?);
                assert_eq!(receiver.store.transfer(&partial.id).await?.status, TransferStatus::Rejected);
                assert!(!tokio::fs::try_exists(partial_path).await?);
                assert_eq!(receiver.store.transfer(&completed.id).await?, completed);
                assert_eq!(tokio::fs::read(destination).await?, b"published");
                let message = Message {
                    id: Uuid::new_v4().to_string(),
                    peer_id: receiver_device.id,
                    direction: Direction::Outgoing,
                    body: "chat after the oversized offer".into(),
                    created_at: now_ms(),
                    status: MessageStatus::Pending,
                };
                sender.store.insert_message(&message).await?;
                sender.outbox.notify_waiters();
                while sender.store.message(&message.id).await?.status != MessageStatus::Sent {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                assert_eq!(receiver.store.message(&message.id).await?.body, message.body);
                assert!(!tokio::fs::try_exists(files::spool_path(&sender, &oversized)).await?);
                Ok::<_, anyhow::Error>(())
            }) => result??,
        }
        receiver_task.abort();
        listen_task.abort();
        Ok(())
    }
}
