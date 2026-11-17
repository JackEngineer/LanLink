use anyhow::{Context, Result, bail, ensure};
use lanlink_protocol::{Control, Frame};
use sha2::{Digest, Sha256};
use snow::{Builder, TransportState};
use std::sync::Arc;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpStream, tcp::OwnedWriteHalf},
    sync::{Mutex, mpsc},
    task::JoinHandle,
    time::{Duration, timeout},
};

pub const NOISE: &str = "Noise_XX_25519_ChaChaPoly_SHA256";
pub fn keypair() -> Result<snow::Keypair> {
    Ok(Builder::new(NOISE.parse()?).generate_keypair()?)
}
pub fn device_id(public: &[u8]) -> String {
    hex::encode(Sha256::digest(public))
}
pub fn commitment(session: &[u8], initiator: bool, nonce: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(b"LANLink-v1-commit");
    h.update(session);
    h.update([u8::from(initiator)]);
    h.update(nonce);
    hex::encode(h.finalize())
}
pub fn security_code(session: &[u8], initiator_nonce: &[u8], responder_nonce: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(b"LANLink-v1-SAS");
    h.update(session);
    h.update(initiator_nonce);
    h.update(responder_nonce);
    let value = hex::encode(h.finalize());
    format!("{} {}", &value[..5], &value[5..10])
}
pub struct Connection {
    pub remote_key: Vec<u8>,
    pub session: Vec<u8>,
    pub pairing: bool,
    noise: Arc<Mutex<TransportState>>,
    writer: OwnedWriteHalf,
    incoming: mpsc::Receiver<Result<Frame>>,
    reader: JoinHandle<()>,
}
impl Drop for Connection {
    fn drop(&mut self) {
        self.reader.abort();
    }
}
impl Connection {
    pub async fn connect(
        socket: TcpStream,
        private: &[u8],
        initiator: bool,
        pairing: bool,
    ) -> Result<Self> {
        timeout(
            Duration::from_secs(10),
            Self::handshake(socket, private, initiator, pairing),
        )
        .await
        .context("handshake timed out")?
    }
    async fn handshake(
        mut socket: TcpStream,
        private: &[u8],
        initiator: bool,
        mut pairing: bool,
    ) -> Result<Self> {
        socket.set_nodelay(true)?;
        let mut preface = *b"LANLINK1\x00";
        if initiator {
            preface[8] = u8::from(pairing);
            socket.write_all(&preface).await?;
        } else {
            socket.read_exact(&mut preface).await?;
            ensure!(
                &preface[..8] == b"LANLINK1" && preface[8] <= 1,
                "unknown protocol"
            );
            pairing = preface[8] == 1;
        }
        let builder = Builder::new(NOISE.parse()?)
            .local_private_key(private)?
            .prologue(&preface)?;
        let mut handshake = if initiator {
            builder.build_initiator()?
        } else {
            builder.build_responder()?
        };
        let mut buffer = vec![0u8; 65535];
        let mut plain = vec![0u8; 65535];
        for turn in 0..3 {
            if (turn % 2 == 0) == initiator {
                let n = handshake.write_message(&[], &mut buffer)?;
                socket.write_u16(n as u16).await?;
                socket.write_all(&buffer[..n]).await?;
            } else {
                let n = socket.read_u16().await? as usize;
                ensure!((32..=4096).contains(&n), "invalid handshake size");
                socket.read_exact(&mut buffer[..n]).await?;
                handshake.read_message(&buffer[..n], &mut plain)?;
            }
        }
        let remote_key = handshake
            .get_remote_static()
            .context("missing remote key")?
            .to_vec();
        let session = handshake.get_handshake_hash().to_vec();
        let noise = Arc::new(Mutex::new(handshake.into_transport_mode()?));
        let (mut reader_socket, writer) = socket.into_split();
        let (send, incoming) = mpsc::channel(32);
        let read_noise = noise.clone();
        let reader = tokio::spawn(async move {
            let mut encrypted = vec![0u8; 65535];
            let mut decrypted = vec![0u8; 65535];
            loop {
                let result: Result<Frame> = async {
                    let n = reader_socket.read_u16().await? as usize;
                    ensure!(n >= 17, "invalid transport size");
                    reader_socket.read_exact(&mut encrypted[..n]).await?;
                    let size = read_noise
                        .lock()
                        .await
                        .read_message(&encrypted[..n], &mut decrypted)?;
                    Frame::decode(&decrypted[..size]).map_err(anyhow::Error::msg)
                }
                .await;
                let failed = result.is_err();
                if send.send(result).await.is_err() || failed {
                    break;
                }
            }
        });
        Ok(Self {
            remote_key,
            session,
            pairing,
            noise,
            writer,
            incoming,
            reader,
        })
    }
    pub async fn send(&mut self, frame: Frame) -> Result<()> {
        let plain = frame.encode().map_err(anyhow::Error::msg)?;
        let mut buffer = vec![0; plain.len() + 16];
        let n = self.noise.lock().await.write_message(&plain, &mut buffer)?;
        timeout(Duration::from_secs(20), async {
            self.writer.write_u16(n as u16).await?;
            self.writer.write_all(&buffer[..n]).await
        })
        .await
        .context("socket write timed out")??;
        Ok(())
    }
    pub async fn control(&mut self, value: Control) -> Result<()> {
        self.send(Frame::Control(value)).await
    }
    pub async fn recv(&mut self) -> Result<Frame> {
        self.incoming.recv().await.context("connection closed")?
    }
    pub async fn recv_control(&mut self) -> Result<Control> {
        match self.recv().await? {
            Frame::Control(c) => Ok(c),
            _ => bail!("application data before authorization"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn commitment_binds_session_role_nonce() {
        assert_ne!(
            commitment(&[1; 32], true, &[2; 32]),
            commitment(&[1; 32], false, &[2; 32])
        );
        assert_ne!(
            commitment(&[1; 32], true, &[2; 32]),
            commitment(&[3; 32], true, &[2; 32])
        );
        assert_eq!(security_code(&[0; 32], &[1; 32], &[2; 32]).len(), 11);
    }
    #[tokio::test]
    async fn encrypted_roundtrip() -> Result<()> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let a = keypair()?;
        let b = keypair()?;
        let expected = a.public.clone();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await?;
            let mut conn = Connection::connect(socket, &b.private, false, false).await?;
            ensure!(conn.remote_key == expected, "wrong key");
            ensure!(conn.pairing, "wrong mode");
            let frame = conn.recv().await?;
            conn.send(frame).await
        });
        let socket = TcpStream::connect(address).await?;
        let mut client = Connection::connect(socket, &a.private, true, true).await?;
        client.control(Control::Ping).await?;
        assert!(matches!(client.recv_control().await?, Control::Ping));
        server.await??;
        Ok(())
    }
}
