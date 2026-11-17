use crate::state::State;
use anyhow::{Context, Result, ensure};
use lanlink_protocol::*;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use tokio::{
    fs::{self, File, OpenOptions},
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
};

pub fn source_failed(t: &Transfer) -> bool {
    t.direction == Direction::Outgoing
        && t.status == TransferStatus::Failed
        && t.failure_kind == Some(TransferFailureKind::SourceUnavailable)
}

pub async fn fail_source(state: &State, t: &mut Transfer, error: anyhow::Error) -> Result<()> {
    t.status = TransferStatus::Failed;
    t.failure_kind = Some(TransferFailureKind::SourceUnavailable);
    t.error = Some(format!(
        "Source snapshot is unavailable; upload the file again. {error}"
    ));
    state.store.save_transfer(t).await?;
    state.changed();
    Ok(())
}

pub fn spool_path(state: &State, t: &Transfer) -> PathBuf {
    state.root.join("spool").join(format!(
        "{}.{}",
        t.id,
        if t.direction == Direction::Incoming {
            "part"
        } else {
            "send"
        }
    ))
}
pub async fn download_path(state: &State, t: &Transfer) -> PathBuf {
    Path::new(&state.settings.read().await.download_dir).join(format!(
        "{}-{}",
        t.id,
        safe_name(&t.name)
    ))
}
pub async fn sync_dir(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        let path = path.to_path_buf();
        tokio::task::spawn_blocking(move || std::fs::File::open(path)?.sync_all()).await??;
    }
    #[cfg(windows)]
    {
        let _ = path;
    }
    Ok(())
}
pub async fn create_partial(state: &State, t: &Transfer) -> Result<()> {
    let path = spool_path(state, t);
    let f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .await?;
    f.sync_all().await?;
    sync_dir(path.parent().unwrap()).await
}
pub async fn remove_partial(state: &State, t: &Transfer) -> Result<()> {
    let path = spool_path(state, t);
    match fs::remove_file(&path).await {
        Ok(()) => sync_dir(path.parent().unwrap()).await?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    Ok(())
}
pub async fn hash_file(path: &Path) -> Result<String> {
    let mut file = File::open(path).await?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; CHUNK_SIZE];
    loop {
        let n = file.read(&mut buffer).await?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}
pub async fn open_source(state: &State, t: &Transfer) -> Result<File> {
    let file = File::open(spool_path(state, t))
        .await
        .context("source snapshot is missing")?;
    let metadata = file.metadata().await?;
    ensure!(
        metadata.is_file() && metadata.len() == t.size,
        "source snapshot is incomplete"
    );
    Ok(file)
}
pub async fn read_source_chunk(state: &State, t: &Transfer) -> Result<Vec<u8>> {
    let mut file = open_source(state, t).await?;
    file.seek(std::io::SeekFrom::Start(t.offset)).await?;
    let len = (t
        .size
        .checked_sub(t.offset)
        .context("source offset exceeds file size")? as usize)
        .min(CHUNK_SIZE);
    let mut bytes = vec![0u8; len];
    file.read_exact(&mut bytes)
        .await
        .context("source snapshot could not be read completely")?;
    Ok(bytes)
}
pub async fn receive_fragment(
    state: &State,
    t: &mut Transfer,
    offset: u64,
    end: bool,
    bytes: &[u8],
    volatile: &mut u64,
) -> Result<bool> {
    ensure!(
        t.direction == Direction::Incoming
            && matches!(
                t.status,
                TransferStatus::Transferring | TransferStatus::Paused
            ),
        "transfer was not accepted"
    );
    ensure!(
        offset == *volatile && offset >= t.offset,
        "noncontiguous fragment"
    );
    let next = offset
        .checked_add(bytes.len() as u64)
        .context("offset overflow")?;
    ensure!(
        next <= t.size && next - t.offset <= CHUNK_SIZE as u64,
        "fragment exceeds transfer or chunk size"
    );
    ensure!(!bytes.is_empty() || (end && t.size == 0), "empty fragment");
    ensure!(
        !end || next == t.size || next - t.offset == CHUNK_SIZE as u64,
        "short logical chunk"
    );
    let mut file = OpenOptions::new()
        .write(true)
        .open(spool_path(state, t))
        .await?;
    file.seek(std::io::SeekFrom::Start(offset)).await?;
    file.write_all(bytes).await?;
    *volatile = next;
    if !end {
        return Ok(false);
    }
    file.sync_all().await?;
    t.offset = next;
    t.status = TransferStatus::Transferring;
    state.store.save_transfer(t).await?;
    Ok(true)
}
pub async fn finalize(state: &State, t: &mut Transfer) -> Result<()> {
    let path = spool_path(state, t);
    ensure!(
        fs::metadata(&path).await?.len() == t.size && hash_file(&path).await? == t.sha256,
        "SHA-256 mismatch"
    );
    publish(state, t).await
}
pub async fn publish(state: &State, t: &mut Transfer) -> Result<()> {
    let path = spool_path(state, t);
    let dest = download_path(state, t).await;
    match fs::hard_link(&path, &dest).await {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            ensure!(
                fs::metadata(&dest).await?.len() == t.size && hash_file(&dest).await? == t.sha256,
                "destination collision"
            );
        }
        Err(e) => return Err(e).context("atomic file publication failed"),
    }
    OpenOptions::new()
        .write(true)
        .open(&dest)
        .await?
        .sync_all()
        .await?;
    sync_dir(dest.parent().unwrap()).await?;
    t.status = TransferStatus::Completed;
    t.error = None;
    state.store.save_transfer(t).await?;
    remove_partial(state, t).await?;
    Ok(())
}
pub async fn recover(state: &State) -> Result<()> {
    state.store.recover_messages().await?;
    let transfers = state.store.transfers().await?;
    for mut t in transfers.clone() {
        if t.status == TransferStatus::Completed
            || (t.direction == Direction::Incoming && t.status == TransferStatus::Failed)
        {
            remove_partial(state, &t).await?;
            continue;
        }
        if matches!(
            t.status,
            TransferStatus::Cancelled | TransferStatus::Rejected
        ) {
            remove_partial(state, &t).await?;
            continue;
        }
        if t.direction == Direction::Incoming
            && matches!(
                t.status,
                TransferStatus::Transferring | TransferStatus::Paused
            )
        {
            let path = spool_path(state, &t);
            match OpenOptions::new().write(true).open(&path).await {
                Ok(file) if file.metadata().await?.len() >= t.offset => {
                    file.set_len(t.offset).await?;
                    file.sync_all().await?;
                    if t.offset == t.size {
                        if let Err(e) = finalize(state, &mut t).await {
                            t.status = TransferStatus::Failed;
                            t.error = Some(e.to_string());
                        }
                    } else {
                        t.status = TransferStatus::Paused;
                    }
                }
                _ => {
                    t.status = TransferStatus::Failed;
                    t.error = Some(
                        "acknowledged partial file is missing or shorter than its durable offset"
                            .into(),
                    );
                }
            }
            state.store.save_transfer(&t).await?;
            if t.status == TransferStatus::Failed {
                remove_partial(state, &t).await?;
            }
        }
        if t.direction == Direction::Outgoing && !t.status.terminal() {
            if let Err(error) = open_source(state, &t).await {
                fail_source(state, &mut t, error).await?;
            } else if t.status == TransferStatus::Transferring {
                t.status = TransferStatus::Paused;
                state.store.save_transfer(&t).await?;
            }
        }
    }
    let known: std::collections::HashSet<_> =
        transfers.iter().map(|t| spool_path(state, t)).collect();
    let mut entries = fs::read_dir(state.root.join("spool")).await?;
    while let Some(entry) = entries.next_entry().await? {
        if !known.contains(&entry.path()) && entry.file_type().await?.is_file() {
            fs::remove_file(entry.path()).await?;
        }
    }
    sync_dir(&state.root.join("spool")).await?;
    Ok(())
}
pub struct UploadGuard {
    pub path: PathBuf,
    pub keep: bool,
    pub store: lanlink_store::Store,
    pub id: String,
}
impl Drop for UploadGuard {
    fn drop(&mut self) {
        if !self.keep {
            let store = self.store.clone();
            let id = self.id.clone();
            let path = self.path.clone();
            tokio::spawn(async move {
                for _ in 0..20 {
                    match store.transfer_exists(&id).await {
                        Ok(true) => return,
                        Ok(false) => {
                            let removed = fs::remove_file(&path).await;
                            if (removed.is_ok()
                                || removed.is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound))
                                && store.release_upload(&id).await.is_ok()
                            {
                                return;
                            }
                        }
                        Err(_) => {}
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
                tracing::warn!(upload_id=%id, "upload cleanup failed; reservation retained until restart");
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::test_state;
    use uuid::Uuid;
    fn transfer(size: u64) -> Transfer {
        Transfer {
            id: Uuid::new_v4().to_string(),
            peer_id: "a".repeat(64),
            direction: Direction::Incoming,
            name: "hello.txt".into(),
            size,
            sha256: String::new(),
            offset: 0,
            status: TransferStatus::Transferring,
            failure_kind: None,
            error: None,
            created_at: 1,
        }
    }
    #[tokio::test]
    async fn restart_fails_missing_and_truncated_outgoing_snapshots() -> Result<()> {
        let root = tempfile::tempdir()?;
        let state = test_state(root.path()).await?;
        for status in [
            TransferStatus::AwaitingAccept,
            TransferStatus::Paused,
            TransferStatus::Transferring,
        ] {
            for content in [
                None,
                Some(b"short".as_slice()),
                Some(b"0123456789".as_slice()),
            ] {
                let mut t = transfer(10);
                t.direction = Direction::Outgoing;
                t.status = status;
                state.store.insert_transfer(&t).await?;
                if let Some(bytes) = content {
                    fs::write(spool_path(&state, &t), bytes).await?;
                }
                recover(&state).await?;
                let current = state.store.transfer(&t.id).await?;
                if content.is_some_and(|bytes| bytes.len() == 10) {
                    assert_eq!(
                        current.status,
                        if status == TransferStatus::Transferring {
                            TransferStatus::Paused
                        } else {
                            status
                        }
                    );
                } else {
                    assert_eq!(current.status, TransferStatus::Failed);
                    assert!(current.error.unwrap().contains("upload the file again"));
                }
            }
        }
        Ok(())
    }
    #[tokio::test]
    async fn fragment_checkpoint_recovery_and_no_clobber_publication() -> Result<()> {
        let root = tempfile::tempdir()?;
        let state = test_state(root.path()).await?;
        let bytes = vec![37u8; CHUNK_SIZE + 3];
        let mut t = transfer(bytes.len() as u64);
        t.sha256 = hex::encode(Sha256::digest(&bytes));
        state.store.reserve(&t).await?;
        create_partial(&state, &t).await?;
        let mut offset = 0;
        for part in bytes[..CHUNK_SIZE].chunks(FRAGMENT_SIZE) {
            let end = offset + part.len() as u64 == CHUNK_SIZE as u64;
            let current = offset;
            receive_fragment(&state, &mut t, current, end, part, &mut offset).await?;
        }
        assert_eq!(state.store.transfer(&t.id).await?.offset, CHUNK_SIZE as u64);
        let current = offset;
        receive_fragment(
            &state,
            &mut t,
            current,
            false,
            &bytes[CHUNK_SIZE..CHUNK_SIZE + 1],
            &mut offset,
        )
        .await?;
        recover(&state).await?;
        assert_eq!(
            fs::metadata(spool_path(&state, &t)).await?.len(),
            CHUNK_SIZE as u64
        );
        t = state.store.transfer(&t.id).await?;
        offset = t.offset;
        let current = offset;
        receive_fragment(
            &state,
            &mut t,
            current,
            true,
            &bytes[CHUNK_SIZE..],
            &mut offset,
        )
        .await?;
        let dest = download_path(&state, &t).await;
        fs::write(&dest, b"existing user data").await?;
        assert!(finalize(&state, &mut t).await.is_err());
        assert_eq!(fs::read(&dest).await?, b"existing user data");
        fs::remove_file(&dest).await?;
        finalize(&state, &mut t).await?;
        assert_eq!(t.status, TransferStatus::Completed);
        assert_eq!(fs::read(&dest).await?, bytes);
        assert!(!spool_path(&state, &t).exists());
        Ok(())
    }
    #[tokio::test]
    async fn shorter_acknowledged_file_fails_without_silent_reset() -> Result<()> {
        let root = tempfile::tempdir()?;
        let state = test_state(root.path()).await?;
        let mut t = transfer(10);
        t.offset = 5;
        state.store.reserve(&t).await?;
        create_partial(&state, &t).await?;
        recover(&state).await?;
        let recovered = state.store.transfer(&t.id).await?;
        assert_eq!(recovered.status, TransferStatus::Failed);
        assert_eq!(recovered.offset, 5);
        assert!(!spool_path(&state, &t).exists());
        Ok(())
    }
    #[tokio::test]
    async fn unaccepted_and_out_of_order_fragments_do_not_write() -> Result<()> {
        let root = tempfile::tempdir()?;
        let state = test_state(root.path()).await?;
        let mut t = transfer(10);
        t.status = TransferStatus::Offered;
        let mut offset = 0;
        assert!(
            receive_fragment(&state, &mut t, 0, false, b"x", &mut offset)
                .await
                .is_err()
        );
        t.status = TransferStatus::Transferring;
        create_partial(&state, &t).await?;
        assert!(
            receive_fragment(&state, &mut t, 1, false, b"x", &mut offset)
                .await
                .is_err()
        );
        assert_eq!(fs::metadata(spool_path(&state, &t)).await?.len(), 0);
        Ok(())
    }
}
