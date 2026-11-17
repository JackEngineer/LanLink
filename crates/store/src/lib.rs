use anyhow::{Context, Result, ensure};
use lanlink_protocol::*;
use serde::{Serialize, de::DeserializeOwned};
use sqlx::{
    Row, SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
};
use std::path::{Path, PathBuf};

const MAX_ACTIVE_TRANSFERS: usize = 32;
const MAX_TRANSFER_HISTORY: usize = 10_000;

#[derive(Clone)]
pub struct Store {
    pub pool: SqlitePool,
    spool_dir: PathBuf,
}
impl Store {
    pub async fn open(path: &Path) -> Result<Self> {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Full)
            .foreign_keys(true)
            .busy_timeout(std::time::Duration::from_secs(10));
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await?;
        sqlx::raw_sql(
            "CREATE TABLE IF NOT EXISTS kv (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS uploads (id TEXT PRIMARY KEY, size INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS trust (id TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS messages (id TEXT PRIMARY KEY, peer TEXT NOT NULL, value TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS messages_peer ON messages(peer);
CREATE TABLE IF NOT EXISTS transfers (id TEXT PRIMARY KEY, peer TEXT NOT NULL, value TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS transfers_peer ON transfers(peer);",
        )
        .execute(&pool)
        .await?;
        let spool_dir = path.parent().unwrap_or(Path::new(".")).join("spool");
        Ok(Self { pool, spool_dir })
    }
    pub async fn get<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        let value: Option<String> = sqlx::query_scalar("SELECT value FROM kv WHERE key=?")
            .bind(key)
            .fetch_optional(&self.pool)
            .await?;
        value
            .map(|v| serde_json::from_str(&v).map_err(Into::into))
            .transpose()
    }
    pub async fn put<T: Serialize>(&self, key: &str, value: &T) -> Result<()> {
        sqlx::query(
            "INSERT INTO kv VALUES (?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        )
        .bind(key)
        .bind(serde_json::to_string(value)?)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
    pub async fn trust(&self, id: &str) -> Result<Option<Trust>> {
        let value: Option<String> = sqlx::query_scalar("SELECT value FROM trust WHERE id=?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        value
            .map(|v| serde_json::from_str(&v).map_err(Into::into))
            .transpose()
    }
    pub async fn trusts(&self) -> Result<Vec<Trust>> {
        self.list("trust").await
    }
    pub async fn pin(&self, device: Device, key: String) -> Result<()> {
        let epoch = self.trust(&device.id).await?.map_or(1, |t| t.epoch + 1);
        let trust = Trust {
            device,
            key,
            epoch,
            revoked: false,
        };
        self.save_trust(&trust).await
    }
    async fn save_trust(&self, trust: &Trust) -> Result<()> {
        sqlx::query(
            "INSERT INTO trust VALUES (?,?) ON CONFLICT(id) DO UPDATE SET value=excluded.value",
        )
        .bind(&trust.device.id)
        .bind(serde_json::to_string(trust)?)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
    pub async fn revoke(&self, id: &str) -> Result<()> {
        let mut trust = self.trust(id).await?.context("device is not paired")?;
        trust.revoked = true;
        trust.epoch += 1;
        let mut tx = self.pool.begin().await?;
        sqlx::query("UPDATE trust SET value=? WHERE id=?")
            .bind(serde_json::to_string(&trust)?)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        for row in sqlx::query("SELECT value FROM messages WHERE peer=?")
            .bind(id)
            .fetch_all(&mut *tx)
            .await?
        {
            let mut m: Message = serde_json::from_str(row.get("value"))?;
            if m.direction == Direction::Outgoing && m.status != MessageStatus::Sent {
                m.status = MessageStatus::Failed;
                sqlx::query("UPDATE messages SET value=? WHERE id=?")
                    .bind(serde_json::to_string(&m)?)
                    .bind(m.id)
                    .execute(&mut *tx)
                    .await?;
            }
        }
        for row in sqlx::query("SELECT value FROM transfers WHERE peer=?")
            .bind(id)
            .fetch_all(&mut *tx)
            .await?
        {
            let mut t: Transfer = serde_json::from_str(row.get("value"))?;
            if !t.status.terminal() {
                t.status = TransferStatus::Cancelled;
                t.error = Some("device revoked".into());
                sqlx::query("UPDATE transfers SET value=? WHERE id=?")
                    .bind(serde_json::to_string(&t)?)
                    .bind(t.id)
                    .execute(&mut *tx)
                    .await?;
            }
        }
        tx.commit().await?;
        Ok(())
    }
    pub async fn trusted(&self, id: &str) -> Result<Trust> {
        let t = self.trust(id).await?.context("device is not paired")?;
        ensure!(!t.revoked, "device is revoked");
        Ok(t)
    }
    async fn list<T: DeserializeOwned>(&self, table: &str) -> Result<Vec<T>> {
        let query = format!("SELECT value FROM {table} ORDER BY rowid");
        sqlx::query_scalar::<_, String>(&query)
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .map(|v| serde_json::from_str(&v).map_err(Into::into))
            .collect()
    }
    pub async fn messages(&self, peer: &str) -> Result<Vec<Message>> {
        sqlx::query_scalar::<_, String>("SELECT value FROM messages WHERE peer=? ORDER BY rowid")
            .bind(peer)
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .map(|v| serde_json::from_str(&v).map_err(Into::into))
            .collect()
    }
    pub async fn message(&self, id: &str) -> Result<Message> {
        let value: String = sqlx::query_scalar("SELECT value FROM messages WHERE id=?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .context("message not found")?;
        Ok(serde_json::from_str(&value)?)
    }
    pub async fn insert_message(&self, message: &Message) -> Result<()> {
        let existing: Option<String> = sqlx::query_scalar("SELECT value FROM messages WHERE id=?")
            .bind(&message.id)
            .fetch_optional(&self.pool)
            .await?;
        if let Some(existing) = existing {
            let existing: Message = serde_json::from_str(&existing)?;
            ensure!(
                existing.peer_id == message.peer_id
                    && existing.direction == message.direction
                    && existing.body == message.body
                    && existing.created_at == message.created_at,
                "conflicting message ID"
            );
            return Ok(());
        }
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages")
            .fetch_one(&self.pool)
            .await?;
        ensure!(count < 100_000, "message history limit reached");
        sqlx::query("INSERT INTO messages VALUES (?,?,?)")
            .bind(&message.id)
            .bind(&message.peer_id)
            .bind(serde_json::to_string(message)?)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
    pub async fn save_message(&self, message: &Message) -> Result<()> {
        sqlx::query("UPDATE messages SET value=? WHERE id=? AND peer=?")
            .bind(serde_json::to_string(message)?)
            .bind(&message.id)
            .bind(&message.peer_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
    pub async fn transfers(&self) -> Result<Vec<Transfer>> {
        self.list("transfers").await
    }
    pub async fn transfer(&self, id: &str) -> Result<Transfer> {
        let value: String = sqlx::query_scalar("SELECT value FROM transfers WHERE id=?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .context("transfer not found")?;
        Ok(serde_json::from_str(&value)?)
    }
    pub async fn transfer_exists(&self, id: &str) -> Result<bool> {
        Ok(
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM transfers WHERE id=?)")
                .bind(id)
                .fetch_one(&self.pool)
                .await?,
        )
    }
    pub async fn insert_transfer(&self, transfer: &Transfer) -> Result<()> {
        sqlx::query("INSERT INTO transfers VALUES (?,?,?)")
            .bind(&transfer.id)
            .bind(&transfer.peer_id)
            .bind(serde_json::to_string(transfer)?)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
    pub async fn save_transfer(&self, transfer: &Transfer) -> Result<()> {
        sqlx::query("UPDATE transfers SET value=? WHERE id=? AND peer=?")
            .bind(serde_json::to_string(transfer)?)
            .bind(&transfer.id)
            .bind(&transfer.peer_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
    pub async fn reserve(&self, transfer: &Transfer) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        let settings = quota_settings(&mut tx).await?;
        let rows: Vec<String> = sqlx::query_scalar("SELECT value FROM transfers")
            .fetch_all(&mut *tx)
            .await?;
        let (uploads, bytes): (i64, i64) =
            sqlx::query_as("SELECT COUNT(*), COALESCE(SUM(size),0) FROM uploads")
                .fetch_one(&mut *tx)
                .await?;
        let mut used = bytes as u64;
        let mut history = uploads as usize;
        let mut active = uploads as usize;
        for row in rows {
            let t: Transfer = serde_json::from_str(&row)?;
            if t.id == transfer.id {
                continue;
            }
            history += 1;
            if reserves_space(&t) {
                used = used
                    .checked_add(self.reserved_bytes(&t, settings.as_ref()).await?)
                    .context("quota overflow")?;
            }
            if !t.status.terminal() {
                active += 1;
            }
        }
        ensure!(
            history < MAX_TRANSFER_HISTORY,
            "transfer history limit reached"
        );
        ensure!(active < MAX_ACTIVE_TRANSFERS, "too many pending transfers");
        ensure!(
            used.checked_add(transfer.size)
                .is_some_and(|n| n <= DISK_QUOTA),
            "8 GiB storage quota exceeded"
        );
        sqlx::query("INSERT INTO transfers VALUES (?,?,?) ON CONFLICT(id) DO UPDATE SET value=excluded.value").bind(&transfer.id).bind(&transfer.peer_id).bind(serde_json::to_string(transfer)?).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn reserve_upload(&self, id: &str, size: u64) -> Result<()> {
        self.reserve_upload_up_to(id, size, size).await?;
        Ok(())
    }
    pub async fn reserve_upload_up_to(
        &self,
        id: &str,
        minimum: u64,
        preferred: u64,
    ) -> Result<u64> {
        ensure!(
            minimum <= preferred && preferred <= DISK_QUOTA,
            "invalid upload reservation"
        );
        let mut tx = self.pool.begin().await?;
        let settings = quota_settings(&mut tx).await?;
        let rows: Vec<String> = sqlx::query_scalar("SELECT value FROM transfers")
            .fetch_all(&mut *tx)
            .await?;
        let (uploads, bytes): (i64, i64) =
            sqlx::query_as("SELECT COUNT(*), COALESCE(SUM(size),0) FROM uploads WHERE id != ?")
                .bind(id)
                .fetch_one(&mut *tx)
                .await?;
        ensure!(
            rows.len() + (uploads as usize) < MAX_TRANSFER_HISTORY,
            "transfer history limit reached"
        );
        let mut active = uploads as usize;
        let mut used = bytes as u64;
        for row in rows {
            let t: Transfer = serde_json::from_str(&row)?;
            if !t.status.terminal() {
                active += 1;
            }
            if reserves_space(&t) {
                used = used
                    .checked_add(self.reserved_bytes(&t, settings.as_ref()).await?)
                    .context("quota overflow")?;
            }
        }
        ensure!(active < MAX_ACTIVE_TRANSFERS, "too many pending transfers");
        ensure!(
            used.checked_add(minimum).is_some_and(|n| n <= DISK_QUOTA),
            "storage quota exceeded"
        );
        let size = if used.checked_add(preferred).is_some_and(|n| n <= DISK_QUOTA) {
            preferred
        } else {
            minimum
        };
        sqlx::query(
            "INSERT INTO uploads VALUES (?,?) ON CONFLICT(id) DO UPDATE SET size=excluded.size",
        )
        .bind(id)
        .bind(size as i64)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(size)
    }
    pub async fn release_upload(&self, id: &str) -> Result<()> {
        sqlx::query("DELETE FROM uploads WHERE id=?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
    pub async fn finish_upload(&self, t: &Transfer) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        let reserved: Option<i64> = sqlx::query_scalar("SELECT size FROM uploads WHERE id=?")
            .bind(&t.id)
            .fetch_optional(&mut *tx)
            .await?;
        ensure!(
            reserved == Some(t.size as i64),
            "upload size does not match its reservation"
        );
        sqlx::query("INSERT INTO transfers VALUES (?,?,?)")
            .bind(&t.id)
            .bind(&t.peer_id)
            .bind(serde_json::to_string(t)?)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM uploads WHERE id=?")
            .bind(&t.id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn recover_messages(&self) -> Result<()> {
        for mut m in self.list::<Message>("messages").await? {
            if m.direction == Direction::Outgoing
                && matches!(m.status, MessageStatus::Sending | MessageStatus::Pending)
            {
                m.status = if Frame::Control(m.control()).encode().is_ok() {
                    MessageStatus::Pending
                } else {
                    MessageStatus::Failed
                };
                self.save_message(&m).await?;
            }
        }
        Ok(())
    }
    pub async fn receive_offer(
        &self,
        offer: &Transfer,
        retry: bool,
        max_file_size: u64,
    ) -> Result<Option<(Transfer, bool)>> {
        let mut tx = self.pool.begin().await?;
        let row: Option<String> = sqlx::query_scalar("SELECT value FROM transfers WHERE id=?")
            .bind(&offer.id)
            .fetch_optional(&mut *tx)
            .await?;
        let mut reopened = None;
        if let Some(row) = row {
            let mut existing: Transfer = serde_json::from_str(&row)?;
            ensure!(
                existing.direction == Direction::Incoming
                    && existing.peer_id == offer.peer_id
                    && existing.name == offer.name
                    && existing.size == offer.size
                    && existing.sha256 == offer.sha256,
                "conflicting transfer ID"
            );
            if offer.size > max_file_size || !retry || existing.status != TransferStatus::Failed {
                return Ok(Some((existing, false)));
            }
            existing.status = TransferStatus::Offered;
            existing.offset = 0;
            existing.error = None;
            reopened = Some(existing);
        }
        if offer.size > max_file_size {
            return Ok(None);
        }
        let rows: Vec<String> = sqlx::query_scalar("SELECT value FROM transfers")
            .fetch_all(&mut *tx)
            .await?;
        let uploads: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM uploads")
            .fetch_one(&mut *tx)
            .await?;
        ensure!(
            reopened.is_some() || rows.len() + (uploads as usize) < MAX_TRANSFER_HISTORY,
            "transfer history limit reached"
        );
        let mut active = uploads as usize;
        for row in rows {
            let t: Transfer = serde_json::from_str(&row)?;
            if !t.status.terminal() {
                active += 1;
            }
        }
        ensure!(active < MAX_ACTIVE_TRANSFERS, "too many pending transfers");
        let retrying = reopened.is_some();
        let t = reopened.unwrap_or_else(|| offer.clone());
        sqlx::query("INSERT INTO transfers VALUES (?,?,?) ON CONFLICT(id) DO UPDATE SET value=excluded.value")
            .bind(&t.id)
            .bind(&t.peer_id)
            .bind(serde_json::to_string(&t)?)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(Some((t, retrying)))
    }

    async fn reserved_bytes(&self, t: &Transfer, settings: Option<&Settings>) -> Result<u64> {
        let path = match (t.direction, t.status, settings) {
            (Direction::Incoming, TransferStatus::Completed, Some(settings)) => {
                Path::new(&settings.download_dir).join(format!("{}-{}", t.id, safe_name(&t.name)))
            }
            (Direction::Outgoing, TransferStatus::Failed, _) => {
                self.spool_dir.join(format!("{}.send", t.id))
            }
            _ => return Ok(t.size),
        };
        match tokio::fs::symlink_metadata(path).await {
            Ok(metadata) if metadata.is_file() => Ok(metadata.len()),
            Ok(_) => Ok(t.size),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(0),
            Err(error) => Err(error).context("cannot reconcile transfer storage"),
        }
    }
}
async fn quota_settings(tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>) -> Result<Option<Settings>> {
    let value: Option<String> = sqlx::query_scalar("SELECT value FROM kv WHERE key='settings'")
        .fetch_optional(&mut **tx)
        .await?;
    value
        .map(|value| serde_json::from_str(&value).map_err(Into::into))
        .transpose()
}
fn reserves_space(t: &Transfer) -> bool {
    match t.direction {
        Direction::Outgoing => !matches!(
            t.status,
            TransferStatus::Rejected | TransferStatus::Cancelled | TransferStatus::Completed
        ),
        Direction::Incoming => matches!(
            t.status,
            TransferStatus::Transferring | TransferStatus::Paused | TransferStatus::Completed
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn transfer(id: &str, direction: Direction, size: u64, status: TransferStatus) -> Transfer {
        Transfer {
            id: id.into(),
            peer_id: "peer".into(),
            direction,
            name: "a.txt".into(),
            size,
            sha256: "a".repeat(64),
            offset: 0,
            status,
            failure_kind: None,
            error: None,
            created_at: 0,
        }
    }
    #[tokio::test]
    async fn restart_fails_legacy_unencodable_outbox_without_losing_valid_messages() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Store::open(&dir.path().join("test.sqlite3")).await?;
        for (id, body) in [
            ("bad", "\u{1}".repeat(MAX_MESSAGE_SIZE)),
            ("good", "hello".into()),
        ] {
            store
                .insert_message(&Message {
                    id: id.into(),
                    peer_id: "peer".into(),
                    direction: Direction::Outgoing,
                    body,
                    created_at: 0,
                    status: MessageStatus::Pending,
                })
                .await?;
        }
        store.recover_messages().await?;
        assert_eq!(store.message("bad").await?.status, MessageStatus::Failed);
        assert_eq!(store.message("good").await?.status, MessageStatus::Pending);
        Ok(())
    }
    #[tokio::test]
    async fn completed_download_quota_tracks_files_without_rewriting_history() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Store::open(&dir.path().join("test.sqlite3")).await?;
        store
            .put(
                "settings",
                &Settings {
                    download_dir: dir.path().to_string_lossy().into(),
                    max_file_size: MAX_FILE_SIZE,
                },
            )
            .await?;
        let completed = transfer(
            "completed",
            Direction::Incoming,
            DISK_QUOTA,
            TransferStatus::Completed,
        );
        let path = dir.path().join("completed-a.txt");
        let file = tokio::fs::File::create(&path).await?;
        file.set_len(DISK_QUOTA).await?;
        store.insert_transfer(&completed).await?;
        assert!(store.reserve_upload("next", 1).await.is_err());
        file.set_len(1).await?;
        store.reserve_upload("next", DISK_QUOTA - 1).await?;
        store.release_upload("next").await?;
        drop(file);
        tokio::fs::rename(&path, dir.path().join("moved.txt")).await?;
        store.reserve_upload("next", DISK_QUOTA).await?;
        store.release_upload("next").await?;
        let next = transfer(
            "incoming",
            Direction::Incoming,
            DISK_QUOTA,
            TransferStatus::Transferring,
        );
        store.reserve(&next).await?;
        assert_eq!(store.transfer("completed").await?, completed);
        Ok(())
    }
    #[tokio::test]
    async fn failed_outgoing_quota_tracks_snapshot_bytes() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Store::open(&dir.path().join("test.sqlite3")).await?;
        let spool = dir.path().join("spool");
        tokio::fs::create_dir(&spool).await?;
        let failed = transfer(
            "failed",
            Direction::Outgoing,
            DISK_QUOTA,
            TransferStatus::Failed,
        );
        let path = spool.join("failed.send");
        let file = tokio::fs::File::create(&path).await?;
        file.set_len(DISK_QUOTA).await?;
        store.insert_transfer(&failed).await?;
        assert!(store.reserve_upload("blocked", 1).await.is_err());
        file.set_len(1).await?;
        let incoming = transfer(
            "incoming",
            Direction::Incoming,
            DISK_QUOTA - 1,
            TransferStatus::Transferring,
        );
        store.reserve(&incoming).await?;
        assert!(store.reserve_upload("blocked", 1).await.is_err());
        drop(file);
        tokio::fs::remove_file(&path).await?;
        store.reserve_upload("next", 1).await?;
        assert_eq!(store.transfer(&failed.id).await?, failed);
        Ok(())
    }
    #[tokio::test]
    async fn upload_owns_its_transfer_slot_until_conversion() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Store::open(&dir.path().join("test.sqlite3")).await?;
        for n in 0..31 {
            store
                .insert_transfer(&transfer(
                    &n.to_string(),
                    Direction::Incoming,
                    1,
                    TransferStatus::Offered,
                ))
                .await?;
        }
        store.reserve_upload("upload", 32).await?;
        let mut offer = transfer("offer", Direction::Incoming, 1, TransferStatus::Offered);
        assert!(
            store
                .receive_offer(&offer, false, MAX_FILE_SIZE)
                .await
                .is_err()
        );
        let mut existing = store.transfer("0").await?;
        assert_eq!(
            store.receive_offer(&existing, false, MAX_FILE_SIZE).await?,
            Some((existing.clone(), false))
        );
        existing.status = TransferStatus::Transferring;
        store.reserve(&existing).await?;
        offer.status = TransferStatus::Transferring;
        assert!(store.reserve(&offer).await.is_err());
        store.reserve_upload("upload", 1).await?;
        let upload = transfer(
            "upload",
            Direction::Outgoing,
            1,
            TransferStatus::AwaitingAccept,
        );
        store.finish_upload(&upload).await?;
        assert_eq!(store.transfer("upload").await?, upload);
        Ok(())
    }

    #[tokio::test]
    async fn oversized_offer_still_checks_identity_before_rejection() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Store::open(&dir.path().join("test.sqlite3")).await?;
        let existing = transfer("offer", Direction::Incoming, 4, TransferStatus::Paused);
        store.insert_transfer(&existing).await?;
        let mut conflicting = existing.clone();
        conflicting.peer_id = "different peer".into();
        assert!(store.receive_offer(&conflicting, false, 1).await.is_err());
        conflicting.peer_id = existing.peer_id.clone();
        conflicting.sha256 = "b".repeat(64);
        assert!(store.receive_offer(&conflicting, false, 1).await.is_err());
        assert_eq!(store.transfer(&existing.id).await?, existing);
        Ok(())
    }
    #[tokio::test]
    async fn upload_reservations_grow_atomically_to_actual_size() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Store::open(&dir.path().join("test.sqlite3")).await?;
        store.reserve_upload("occupied", DISK_QUOTA - 3).await?;
        store.reserve_upload("tiny", 0).await?;
        store.reserve_upload("tiny", 2).await?;
        store.reserve_upload("tiny", 3).await?;
        assert!(store.reserve_upload("tiny", 4).await.is_err());
        let mut uploaded = transfer(
            "tiny",
            Direction::Outgoing,
            2,
            TransferStatus::AwaitingAccept,
        );
        assert!(store.finish_upload(&uploaded).await.is_err());
        uploaded.size = 3;
        store.finish_upload(&uploaded).await?;
        assert!(store.reserve_upload("extra", 1).await.is_err());
        assert_eq!(store.transfer("tiny").await?.size, 3);
        Ok(())
    }
    #[tokio::test]
    async fn concurrent_upload_reservations_cannot_exceed_quota() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Store::open(&dir.path().join("test.sqlite3")).await?;
        let (a, b) = tokio::join!(
            store.reserve_upload("a", DISK_QUOTA / 2 + 1),
            store.reserve_upload("b", DISK_QUOTA / 2 + 1)
        );
        assert_ne!(a.is_ok(), b.is_ok());
        Ok(())
    }
    #[tokio::test]
    async fn durable_dedup_outbox_and_revoke() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("test.sqlite3");
        let store = Store::open(&path).await?;
        let device = Device {
            id: "a".repeat(64),
            name: "peer".into(),
            platform: "test".into(),
        };
        store.pin(device.clone(), "key".into()).await?;
        let incoming = Message {
            id: "message-1".into(),
            peer_id: device.id.clone(),
            direction: Direction::Incoming,
            body: "hello".into(),
            created_at: 1,
            status: MessageStatus::Sent,
        };
        store.insert_message(&incoming).await?;
        store.insert_message(&incoming).await?;
        let mut conflict = incoming.clone();
        conflict.body = "different".into();
        assert!(store.insert_message(&conflict).await.is_err());
        let outgoing = Message {
            id: "message-2".into(),
            peer_id: device.id.clone(),
            direction: Direction::Outgoing,
            body: "outbox".into(),
            created_at: 2,
            status: MessageStatus::Sending,
        };
        store.insert_message(&outgoing).await?;
        store.pool.close().await;
        let store = Store::open(&path).await?;
        store.recover_messages().await?;
        assert_eq!(store.messages(&device.id).await?.len(), 2);
        assert_eq!(
            store.message(&outgoing.id).await?.status,
            MessageStatus::Pending
        );
        let epoch = store.trusted(&device.id).await?.epoch;
        store.revoke(&device.id).await?;
        assert!(store.trusted(&device.id).await.is_err());
        assert!(store.trust(&device.id).await?.unwrap().epoch > epoch);
        assert_eq!(
            store.message(&outgoing.id).await?.status,
            MessageStatus::Failed
        );
        assert_eq!(
            store.message(&incoming.id).await?.status,
            MessageStatus::Sent
        );
        Ok(())
    }
    #[tokio::test]
    async fn quota_includes_upload_reservations() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Store::open(&dir.path().join("test.sqlite3")).await?;
        store.reserve_upload("upload", DISK_QUOTA).await?;
        let t = Transfer {
            id: "transfer".into(),
            peer_id: "peer".into(),
            direction: Direction::Incoming,
            name: "a.txt".into(),
            size: 1,
            sha256: "a".repeat(64),
            offset: 0,
            status: TransferStatus::Transferring,
            failure_kind: None,
            error: None,
            created_at: 0,
        };
        assert!(store.reserve(&t).await.is_err());
        store.release_upload("upload").await?;
        store.reserve(&t).await?;
        assert_eq!(store.transfer("transfer").await?.size, 1);
        Ok(())
    }
}
