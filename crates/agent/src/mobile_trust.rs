use anyhow::{Result, ensure};
use lanlink_store::Store;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;
use uuid::Uuid;

pub(crate) const LIFETIME_MS: i64 = 90 * 24 * 60 * 60 * 1000;
const MAX_BROWSERS: usize = 32;

pub(crate) struct Registry {
    pub cookie_name: String,
    pub data: Mutex<Records>,
}

pub(crate) struct Records {
    store: Store,
    key: String,
    entries: Vec<Record>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Record {
    #[serde(flatten)]
    browser: RememberedBrowser,
    token_sha256: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct RememberedBrowser {
    pub id: String,
    pub name: String,
    pub created_at: i64,
    pub last_used_at: i64,
    pub expires_at: i64,
}

pub(crate) fn digest(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

impl Registry {
    pub async fn open(store: Store, identity: &str) -> Result<Self> {
        let identity = digest(identity);
        let key = format!("mobile_browsers_{identity}");
        let entries: Vec<Record> = store.get(&key).await?.unwrap_or_default();
        ensure!(
            entries.len() <= MAX_BROWSERS,
            "mobile browser registry exceeds limit"
        );
        ensure!(
            entries.iter().all(|record| {
                Uuid::parse_str(&record.browser.id).is_ok()
                    && record.token_sha256.len() == 64
                    && record
                        .token_sha256
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit())
                    && record.browser.name.len() <= 80
            }),
            "invalid mobile browser registry"
        );
        Ok(Self {
            cookie_name: format!("lanlink_remember_{identity}"),
            data: Mutex::new(Records {
                store,
                key,
                entries,
            }),
        })
    }
}

impl Records {
    pub fn list(&self, now: i64) -> Vec<RememberedBrowser> {
        self.entries
            .iter()
            .filter(|record| record.browser.expires_at > now)
            .map(|record| record.browser.clone())
            .collect()
    }

    pub fn find(&self, token: &str, now: i64) -> Option<RememberedBrowser> {
        if token.len() != 64 || !token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return None;
        }
        let hash = digest(token);
        self.entries
            .iter()
            .find(|record| record.token_sha256 == hash && record.browser.expires_at > now)
            .map(|record| record.browser.clone())
    }

    pub fn contains(&self, id: &str, now: i64) -> bool {
        self.entries
            .iter()
            .any(|record| record.browser.id == id && record.browser.expires_at > now)
    }

    pub async fn register(
        &mut self,
        token: &str,
        name: String,
        now: i64,
    ) -> Result<RememberedBrowser> {
        let mut entries: Vec<_> = self
            .entries
            .iter()
            .filter(|record| record.browser.expires_at > now)
            .cloned()
            .collect();
        ensure!(
            entries.len() < MAX_BROWSERS,
            "remembered browser limit reached"
        );
        let browser = RememberedBrowser {
            id: Uuid::new_v4().to_string(),
            name,
            created_at: now,
            last_used_at: now,
            expires_at: now + LIFETIME_MS,
        };
        entries.push(Record {
            browser: browser.clone(),
            token_sha256: digest(token),
        });
        self.save(entries).await?;
        Ok(browser)
    }

    pub async fn renew(&mut self, id: &str, now: i64) -> Result<RememberedBrowser> {
        let mut entries = self.entries.clone();
        let record = entries
            .iter_mut()
            .find(|record| record.browser.id == id && record.browser.expires_at > now)
            .ok_or_else(|| anyhow::anyhow!("remembered browser expired"))?;
        record.browser.last_used_at = now;
        record.browser.expires_at = now + LIFETIME_MS;
        let browser = record.browser.clone();
        self.save(entries).await?;
        Ok(browser)
    }

    pub async fn revoke(&mut self, id: &str) -> Result<()> {
        let entries = self
            .entries
            .iter()
            .filter(|record| record.browser.id != id)
            .cloned()
            .collect();
        self.save(entries).await
    }

    async fn save(&mut self, entries: Vec<Record>) -> Result<()> {
        self.store.put(&self.key, &entries).await?;
        self.entries = entries;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn registry_bounds_survive_reopen_and_failed_writes_preserve_grants() -> Result<()> {
        let root = tempfile::tempdir()?;
        let store = Store::open(&root.path().join("trust.sqlite3")).await?;
        let registry = Registry::open(store.clone(), "desktop").await?;
        let mut records = registry.data.lock().await;
        for index in 0..MAX_BROWSERS {
            records
                .register(&format!("{index:064x}"), "Browser".into(), 1000)
                .await?;
        }
        assert!(
            records
                .register(&"f".repeat(64), "Browser".into(), 1000)
                .await
                .is_err()
        );
        assert_eq!(records.list(1000).len(), MAX_BROWSERS);
        drop(records);
        let reopened = Registry::open(store.clone(), "desktop").await?;
        assert_eq!(reopened.data.lock().await.list(1000).len(), MAX_BROWSERS);
        assert!(
            Registry::open(store.clone(), "other-desktop")
                .await?
                .data
                .lock()
                .await
                .list(1000)
                .is_empty()
        );
        let mut records = registry.data.lock().await;
        let known = records.find(&format!("{:064x}", 0), 1000).unwrap();
        sqlx::query("DROP TABLE kv").execute(&store.pool).await?;
        assert!(records.revoke(&known.id).await.is_err());
        assert!(records.renew(&known.id, 2000).await.is_err());
        assert_eq!(
            records
                .find(&format!("{:064x}", 0), 1000)
                .unwrap()
                .last_used_at,
            1000
        );
        Ok(())
    }
}
