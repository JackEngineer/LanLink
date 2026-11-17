use anyhow::{Result, ensure};
use lanlink_protocol::*;
use lanlink_store::Store;
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
};
use tokio::sync::{Mutex, RwLock, Semaphore, broadcast, mpsc, watch};

pub struct PairAttempt {
    pub value: Pairing,
    pub decisions: mpsc::Sender<bool>,
}
#[derive(Clone, PartialEq, Eq)]
pub struct PairGrant {
    pub attempt: String,
    pub expires_at: i64,
}
#[derive(Clone)]
pub struct Active {
    pub session: String,
    pub preferred: bool,
    pub stop: watch::Sender<bool>,
    pub done: watch::Receiver<bool>,
}
#[derive(Clone)]
pub struct Observation {
    pub peer: Peer,
    pub seen: i64,
}
pub struct State {
    pub store: Store,
    pub device: RwLock<Device>,
    pub private: Vec<u8>,
    pub token: String,
    pub root: PathBuf,
    pub settings: RwLock<Settings>,
    pub origin: String,
    pub host: String,
    pub observations: RwLock<HashMap<String, Observation>>,
    pub pairings: Mutex<HashMap<String, PairAttempt>>,
    pub active: Mutex<HashMap<String, Active>>,
    pub gates: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    pub dialing: Mutex<HashMap<String, bool>>,
    pub allow_pair: Mutex<HashMap<String, PairGrant>>,
    pub denied: Mutex<HashSet<String>>,
    pub pairing_slots: Arc<Semaphore>,
    pub events: broadcast::Sender<()>,
    pub handshakes: Arc<Semaphore>,
    pub sessions: Arc<Semaphore>,
    pub uploads: Arc<Semaphore>,
    pub tcp_port: u16,
    pub outbox: tokio::sync::Notify,
}
impl State {
    pub fn changed(&self) {
        let _ = self.events.send(());
    }
    pub async fn gate(&self, id: &str) -> Arc<Mutex<()>> {
        self.gates
            .lock()
            .await
            .entry(id.to_owned())
            .or_default()
            .clone()
    }
    pub async fn authorized(&self, peer: &str, session: &str) -> Result<Trust> {
        ensure!(
            !self.denied.lock().await.contains(peer),
            "device was revoked"
        );
        let trust = self.store.trusted(peer).await?;
        ensure!(
            self.active
                .lock()
                .await
                .get(peer)
                .is_some_and(|a| a.session == session && !*a.stop.borrow()),
            "stale connection"
        );
        Ok(trust)
    }
    pub async fn peers(&self) -> Result<Vec<Peer>> {
        let active = self.active.lock().await.clone();
        let mut peers: HashMap<String, Peer> = self
            .observations
            .read()
            .await
            .iter()
            .map(|(id, obs)| {
                let mut p = obs.peer.clone();
                p.online = obs.peer.online || active.contains_key(id);
                (id.clone(), p)
            })
            .collect();
        for t in self.store.trusts().await? {
            let p = peers.entry(t.device.id.clone()).or_insert(Peer {
                device: t.device,
                addresses: Vec::new(),
                port: 0,
                online: false,
                paired: false,
            });
            p.paired = !t.revoked;
            p.online |= active.contains_key(&p.device.id);
        }
        let mut peers: Vec<_> = peers.into_values().collect();
        peers.sort_by(|a, b| a.device.name.cmp(&b.device.name));
        Ok(peers)
    }
    pub async fn revoke(&self, id: &str) -> Result<()> {
        let gate = self.gate(id).await;
        let mut done = {
            let _guard = gate.lock().await;
            self.denied.lock().await.insert(id.to_owned());
            self.allow_pair.lock().await.remove(id);
            let current = self.active.lock().await.remove(id).map(|a| {
                let _ = a.stop.send(true);
                a.done
            });
            self.store.revoke(id).await?;
            current
        };
        if let Some(done) = done.as_mut() {
            while !*done.borrow() {
                if done.changed().await.is_err() {
                    break;
                }
            }
        }
        let decisions: Vec<_> = self
            .pairings
            .lock()
            .await
            .values()
            .filter(|p| p.value.peer_id == id)
            .map(|p| p.decisions.clone())
            .collect();
        for decision in decisions {
            let _ = decision.try_send(false);
        }
        for t in self
            .store
            .transfers()
            .await?
            .into_iter()
            .filter(|t| t.peer_id == id && t.status == TransferStatus::Cancelled)
        {
            crate::files::remove_partial(self, &t).await?;
        }
        self.changed();
        Ok(())
    }
    pub async fn transfer_action(&self, id: &str, action: &str) -> Result<()> {
        let initial = self.store.transfer(id).await?;
        let gate = self.gate(&initial.peer_id).await;
        let _guard = gate.lock().await;
        let mut t = self.store.transfer(id).await?;
        let discard_failed = action == "cancel"
            && t.direction == Direction::Outgoing
            && t.status == TransferStatus::Failed;
        if !discard_failed {
            self.store.trusted(&t.peer_id).await?;
        }
        match action {
            "accept" => {
                ensure!(
                    t.direction == Direction::Incoming && t.status == TransferStatus::Offered,
                    "only incoming offers can be accepted"
                );
                ensure!(
                    t.size <= self.settings.read().await.max_file_size,
                    "file exceeds configured limit"
                );
                t.status = TransferStatus::Transferring;
                self.store.reserve(&t).await?;
                if let Err(error) = crate::files::create_partial(self, &t).await {
                    t.status = TransferStatus::Failed;
                    t.error = Some(error.to_string());
                    self.store.save_transfer(&t).await?;
                    return Err(error);
                }
            }
            "reject" => {
                ensure!(
                    t.direction == Direction::Incoming && t.status == TransferStatus::Offered,
                    "only incoming offers can be rejected"
                );
                t.status = TransferStatus::Rejected;
                self.store.save_transfer(&t).await?;
            }
            "cancel" => {
                ensure!(
                    discard_failed || !t.status.terminal(),
                    "transfer is already finished"
                );
                crate::files::remove_partial(self, &t).await?;
                if discard_failed {
                    t.failure_kind = None;
                    t.error = None;
                }
                t.status = TransferStatus::Cancelled;
                self.store.save_transfer(&t).await?;
            }
            "retry" => {
                ensure!(
                    t.direction == Direction::Outgoing
                        && matches!(t.status, TransferStatus::Paused | TransferStatus::Failed),
                    "only paused or failed outgoing transfers can be retried"
                );
                ensure!(
                    !crate::files::source_failed(&t),
                    "source snapshot is unavailable; upload the file again"
                );
                crate::files::open_source(self, &t).await?;
                t.status = TransferStatus::AwaitingAccept;
                t.error = None;
                self.store.reserve(&t).await?;
            }
            _ => anyhow::bail!("unknown action"),
        }
        self.outbox.notify_waiters();
        self.changed();
        Ok(())
    }
}

#[cfg(test)]
pub async fn test_state(root: &std::path::Path) -> Result<State> {
    tokio::fs::create_dir_all(root.join("spool")).await?;
    tokio::fs::create_dir_all(root.join("downloads")).await?;
    let store = Store::open(&root.join("test.sqlite3")).await?;
    let device = Device {
        id: "a".repeat(64),
        name: "test".into(),
        platform: "test".into(),
    };
    store.pin(device.clone(), "test-key".into()).await?;
    let (events, _) = broadcast::channel(8);
    Ok(State {
        store,
        device: RwLock::new(device),
        private: vec![0; 32],
        token: "test-token".into(),
        root: root.into(),
        settings: RwLock::new(Settings {
            download_dir: root.join("downloads").to_string_lossy().into(),
            max_file_size: MAX_FILE_SIZE,
        }),
        origin: "http://127.0.0.1:1".into(),
        host: "127.0.0.1:1".into(),
        observations: RwLock::new(HashMap::new()),
        pairings: Mutex::new(HashMap::new()),
        active: Mutex::new(HashMap::new()),
        gates: Mutex::new(HashMap::new()),
        dialing: Mutex::new(HashMap::new()),
        allow_pair: Mutex::new(HashMap::new()),
        denied: Mutex::new(HashSet::new()),
        pairing_slots: Arc::new(Semaphore::new(4)),
        events,
        handshakes: Arc::new(Semaphore::new(4)),
        sessions: Arc::new(Semaphore::new(64)),
        uploads: Arc::new(Semaphore::new(1)),
        tcp_port: 0,
        outbox: tokio::sync::Notify::new(),
    })
}
