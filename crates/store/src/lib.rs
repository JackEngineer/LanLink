use anyhow::{Context, Result, bail, ensure};
use lanlink_protocol::*;
use serde::{Serialize, de::DeserializeOwned};
use sqlx::{
    Row, SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
};
use std::path::Path;

#[derive(Clone)]
pub struct Store {
    pub pool: SqlitePool,
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
        Ok(Self { pool })
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
        let rows: Vec<String> = sqlx::query_scalar("SELECT value FROM transfers")
            .fetch_all(&mut *tx)
            .await?;
        let mut used: u64 =
            sqlx::query_scalar::<_, i64>("SELECT COALESCE(SUM(size),0) FROM uploads")
                .fetch_one(&mut *tx)
                .await? as u64;
        let mut active = 0;
        for row in rows {
            let t: Transfer = serde_json::from_str(&row)?;
            if t.id == transfer.id {
                continue;
            }
            if reserves_space(&t) {
                used = used.checked_add(t.size).context("quota overflow")?;
            }
            if !t.status.terminal() {
                active += 1;
            }
        }
        ensure!(active < 32, "too many pending transfers");
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
        let mut tx = self.pool.begin().await?;
        let rows: Vec<String> = sqlx::query_scalar("SELECT value FROM transfers")
            .fetch_all(&mut *tx)
            .await?;
        ensure!(rows.len() < 10_000, "transfer history limit reached");
        let mut active = 0;
        let mut used: u64 =
            sqlx::query_scalar::<_, i64>("SELECT COALESCE(SUM(size),0) FROM uploads")
                .fetch_one(&mut *tx)
                .await? as u64;
        for row in rows {
            let t: Transfer = serde_json::from_str(&row)?;
            if !t.status.terminal() {
                active += 1;
            }
            if reserves_space(&t) {
                used = used.checked_add(t.size).context("quota overflow")?;
            }
        }
        ensure!(active < 32, "too many pending transfers");
        ensure!(
            used.checked_add(size).is_some_and(|n| n <= DISK_QUOTA),
            "storage quota exceeded"
        );
        sqlx::query("INSERT INTO uploads VALUES (?,?)")
            .bind(id)
            .bind(size as i64)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
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
            if m.status == MessageStatus::Sending {
                m.status = MessageStatus::Pending;
                self.save_message(&m).await?;
            }
        }
        Ok(())
    }
    pub async fn receive_offer(&self, offer: &Transfer) -> Result<Transfer> {
        if let Ok(existing) = self.transfer(&offer.id).await {
            ensure!(
                existing.direction == Direction::Incoming
                    && existing.peer_id == offer.peer_id
                    && existing.name == offer.name
                    && existing.size == offer.size
                    && existing.sha256 == offer.sha256,
                "conflicting transfer ID"
            );
            return Ok(existing);
        }
        let transfers = self.transfers().await?;
        ensure!(transfers.len() < 10_000, "transfer history limit reached");
        if transfers.iter().filter(|t| !t.status.terminal()).count() >= 32 {
            bail!("too many pending transfers");
        }
        self.insert_transfer(offer).await?;
        Ok(offer.clone())
    }
}
pub fn reserves_space(t: &Transfer) -> bool {
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
