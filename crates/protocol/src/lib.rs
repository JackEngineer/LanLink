use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const VERSION: &str = "0.1.0";
pub const MAX_FILE_SIZE: u64 = 2 * 1024 * 1024 * 1024;
pub const DISK_QUOTA: u64 = 8 * 1024 * 1024 * 1024;
pub const CHUNK_SIZE: usize = 1024 * 1024;
pub const FRAGMENT_SIZE: usize = 60 * 1024;
pub const MAX_MESSAGE_SIZE: usize = 16 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub platform: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Peer {
    #[serde(flatten)]
    pub device: Device,
    pub addresses: Vec<String>,
    pub port: u16,
    pub online: bool,
    pub paired: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pairing {
    pub id: String,
    pub peer_id: String,
    pub peer_name: String,
    pub code: String,
    pub local_confirmed: bool,
    pub remote_confirmed: bool,
    pub expires_at: i64,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Incoming,
    Outgoing,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MessageStatus {
    Sending,
    Sent,
    Pending,
    Failed,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Message {
    pub id: String,
    pub peer_id: String,
    pub direction: Direction,
    pub body: String,
    pub created_at: i64,
    pub status: MessageStatus,
}
impl Message {
    pub fn control(&self) -> Control {
        Control::Chat {
            id: self.id.clone(),
            body: self.body.clone(),
            created_at: self.created_at,
        }
    }
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TransferStatus {
    Offered,
    AwaitingAccept,
    Transferring,
    Paused,
    Completed,
    Rejected,
    Cancelled,
    Failed,
}
impl TransferStatus {
    pub fn terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Rejected | Self::Cancelled | Self::Failed
        )
    }
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TransferFailureKind {
    SourceUnavailable,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Transfer {
    pub id: String,
    pub peer_id: String,
    pub direction: Direction,
    pub name: String,
    pub size: u64,
    pub sha256: String,
    pub offset: u64,
    pub status: TransferStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_kind: Option<TransferFailureKind>,
    pub error: Option<String>,
    pub created_at: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub download_dir: String,
    pub max_file_size: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Trust {
    pub device: Device,
    pub key: String,
    pub epoch: u64,
    pub revoked: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Control {
    Hello {
        device: Device,
    },
    Commit {
        value: String,
    },
    Open {
        nonce: String,
    },
    Approve {
        session: String,
    },
    Ready {
        session: String,
    },
    Deny,
    Chat {
        id: String,
        body: String,
        created_at: i64,
    },
    ChatAck {
        id: String,
    },
    Offer {
        id: String,
        name: String,
        size: u64,
        sha256: String,
        created_at: i64,
        retry: bool,
    },
    TransferState {
        id: String,
        status: TransferStatus,
        offset: u64,
    },
    Cancel {
        id: String,
    },
    Ping,
}
#[derive(Debug)]
pub enum Frame {
    Control(Control),
    Data {
        id: Uuid,
        offset: u64,
        end: bool,
        bytes: Vec<u8>,
    },
}
impl Frame {
    pub fn encode(&self) -> Result<Vec<u8>, String> {
        let mut out = Vec::new();
        match self {
            Self::Control(control) => {
                control.validate_timestamp()?;
                out.push(0);
                out.extend(serde_json::to_vec(control).map_err(|e| e.to_string())?);
            }
            Self::Data {
                id,
                offset,
                end,
                bytes,
            } => {
                if bytes.len() > FRAGMENT_SIZE {
                    return Err("fragment too large".into());
                }
                out.push(1);
                out.extend(id.as_bytes());
                out.extend(offset.to_be_bytes());
                out.push(u8::from(*end));
                out.extend(bytes);
            }
        }
        if out.len() > 65519 {
            return Err("record exceeds Noise maximum".into());
        }
        Ok(out)
    }
    pub fn decode(input: &[u8]) -> Result<Self, String> {
        match input.first() {
            Some(0) => {
                let control: Control =
                    serde_json::from_slice(&input[1..]).map_err(|e| e.to_string())?;
                control.validate_timestamp()?;
                Ok(Self::Control(control))
            }
            Some(1) if input.len() >= 26 && input.len() <= 26 + FRAGMENT_SIZE && input[25] <= 1 => {
                Ok(Self::Data {
                    id: Uuid::from_slice(&input[1..17]).map_err(|e| e.to_string())?,
                    offset: u64::from_be_bytes(input[17..25].try_into().unwrap()),
                    end: input[25] == 1,
                    bytes: input[26..].to_vec(),
                })
            }
            _ => Err("invalid record".into()),
        }
    }
}
impl Control {
    fn validate_timestamp(&self) -> Result<(), String> {
        if let Self::Chat { created_at, .. } | Self::Offer { created_at, .. } = self
            && !(-8_640_000_000_000_000..=8_640_000_000_000_000).contains(created_at)
        {
            return Err("timestamp is outside the supported date range".into());
        }
        Ok(())
    }
}
pub fn valid_id(id: &str) -> bool {
    id.len() == 64
        && id
            .bytes()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}
pub fn safe_name(name: &str) -> String {
    let mut clean: String = name
        .chars()
        .filter(|c| !c.is_control() && !"/\\<>:\"|?*".contains(*c))
        .take(120)
        .collect();
    while clean.len() > 180 {
        clean.pop();
    }
    let clean = clean.trim().trim_matches('.').trim();
    let stem = clean.split('.').next().unwrap_or("").to_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && matches!(stem.as_bytes()[3], b'1'..=b'9'));
    if clean.is_empty() || reserved {
        "download".into()
    } else {
        clean.into()
    }
}
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn network_timestamps_must_be_representable_by_clients() {
        for timestamp in [
            i64::MIN,
            -8_640_000_000_000_001,
            8_640_000_000_000_001,
            i64::MAX,
        ] {
            for control in [
                Control::Chat {
                    id: Uuid::new_v4().to_string(),
                    body: "hello".into(),
                    created_at: timestamp,
                },
                Control::Offer {
                    id: Uuid::new_v4().to_string(),
                    name: "a.txt".into(),
                    size: 1,
                    sha256: "a".repeat(64),
                    created_at: timestamp,
                    retry: false,
                },
            ] {
                let mut wire = vec![0];
                wire.extend(serde_json::to_vec(&control).unwrap());
                assert!(Frame::decode(&wire).unwrap_err().contains("timestamp"));
                assert!(Frame::Control(control).encode().is_err());
            }
        }
        for timestamp in [-8_640_000_000_000_000, 0, now_ms(), 8_640_000_000_000_000] {
            let encoded = Frame::Control(Control::Chat {
                id: Uuid::new_v4().to_string(),
                body: "hello".into(),
                created_at: timestamp,
            })
            .encode()
            .unwrap();
            assert!(Frame::decode(&encoded).is_ok());
        }
    }
    #[test]
    fn fragments_fit_noise_and_roundtrip() {
        let frame = Frame::Data {
            id: Uuid::new_v4(),
            offset: 987,
            end: true,
            bytes: vec![8; FRAGMENT_SIZE],
        };
        let encoded = frame.encode().unwrap();
        assert!(encoded.len() + 16 <= 65535);
        assert!(matches!(
            Frame::decode(&encoded).unwrap(),
            Frame::Data {
                offset: 987,
                end: true,
                ..
            }
        ));
        assert!(Frame::decode(&[1, 0]).is_err());
    }
    #[test]
    fn names_are_safe_on_both_platforms() {
        for name in ["../../", "CON.txt", "NUL", "LPT1.foo", "...", ""] {
            assert_eq!(safe_name(name), "download");
        }
        assert_eq!(safe_name("照片.png"), "照片.png");
        assert_eq!(safe_name("a\\b/c.txt"), "abc.txt");
    }
}
