# LANLink MVP architecture candidate

## Decision

Use explicit domain services backed by SQLite, with one `PeerSession` actor per connected peer and one `TransferWorker` per active transfer. Avoid a global actor owning all application state. Durable truth belongs in SQLite; connection actors own only ephemeral Noise state, bounded queues, and cancellation. This keeps unrelated peers concurrent while giving each ordered transport a single owner.

The Rust agent owns identity, discovery, pairing, transport, persistence, and file I/O. The React UI talks only to the loopback Axum API. `AppService` validates commands and coordinates repositories and workers; domain rules remain in `pairing`, `messaging`, and `transfer` modules.

## Data shapes

```rust
type DeviceId = [u8; 32];       // SHA-256 of Noise static public key
type MessageId = Uuid;
type TransferId = Uuid;
type PairingId = Uuid;

struct Device { id: DeviceId, name: String, os: Os, tcp_port: u16, trust: Trust }
enum Trust { Pending(PairingId), Paired, Revoked }
enum MessageStatus { Pending, Sending, Sent, Failed(String) }
struct Message { id: MessageId, peer: DeviceId, body: String, created_at: i64, status: MessageStatus }
struct Pairing { id: PairingId, peer_key: [u8; 32], handshake_hash: [u8; 32], local_approved: bool, remote_approved: bool, expires_at: i64 }
enum TransferState { AwaitingAcceptance, Receiving, Sending, Paused, Complete, Cancelled, Failed(String) }
struct Transfer { id: TransferId, peer: DeviceId, name: SafeName, size: u64, sha256: [u8; 32], chunk_size: u32, state: TransferState, durable_offset: u64 }
```

Parse and validate discovery records, API JSON, and wire frames at their boundaries. Enforce size, enum, path, and identity constraints before constructing domain values. Never accept a peer ID independently of the public key that proves it.

## Modules and ownership

- `identity`: create/load the persistent Noise static key; derive `DeviceId`; protect the private key with the platform credential store where practical, otherwise an owner-only data directory.
- `discovery`: publish ID, display name, OS, and actual bound port through mDNS; treat records as untrusted hints only.
- `transport`: listener, per-connection Noise handshake, encrypted record framing, and `PeerSession` actor. One actor serializes all reads/writes for its connection. Bound each peer's outbound queue.
- `pairing`: pairing state machine, SAS generation, bilateral approval, expiry, and trust pin persistence.
- `messaging`: outbox/inbox repositories and delivery semantics.
- `transfer`: acceptance, bounded chunk streaming, durable progress, resume, hash verification, safe finalization, and cancellation.
- `store`: SQLite migrations and transactions. Enable foreign keys, WAL, and a busy timeout; keep one pool rather than sharing mutable in-memory copies of durable state.
- `api`: Axum REST and WebSocket adapter. `AppService` is the only command boundary from the UI; websocket events are notifications, with SQLite as the source of truth.

The service can spawn a peer actor after Noise authentication and stop it on disconnect or revocation. Actors receive typed commands and return typed events. Do not put database policy inside actor loops; repositories make commits and return committed records before the actor sends acknowledgements.

## Pairing and authenticated reconnection

Use Noise `XX` with X25519, ChaChaPoly, and BLAKE2s for first contact. Both sides exchange their static keys and identity metadata inside the authenticated handshake. Derive the displayed short authentication string from the final Noise handshake hash, both ordered device IDs, and a fixed protocol label. For example, compare the first 20 bits as five decimal digits. Both clients display the same value; it is a human comparison aid, not a substitute for Noise authentication.

After the handshake, permit only pairing-control frames. Each user locally approves that exact peer and SAS. Send `PairApprove { pairing_id, handshake_hash }` only after local approval. Mark the peer paired and persist its static key only when local approval is true and a matching remote approval arrives, with the transaction keyed by pairing ID and handshake hash. A new handshake gets a new pairing ID and hash; old approvals cannot authorize it. Expire incomplete pairings and discard their transient state. No messages or files are accepted before the bilateral commit.

On subsequent connections, use Noise `IK` with the local static key and the stored peer static key. Require the authenticated remote key to map to a `Paired` device. The Noise transport keys are fresh for each connection; “resume” means reconnecting and reconciling durable message/transfer state, not reusing old session keys. A revoked peer is rejected before application frames are handled, and its active actors are cancelled immediately.

## Wire framing and local protocol

After Noise completes, use a length-prefixed encrypted record protocol. Each encrypted payload stays below snow's 65,535-byte message limit; cap plaintext records around 60 KiB to leave framing and tag room. A logical 1 MiB file chunk is split into numbered records with `transfer_id`, `chunk_index`, byte offset, and bounded payload. Reassemble or stream into the destination range without allocating an attacker-selected buffer.

Typed frames include `PairApprove`, `MessageSubmit`, `MessageAck`, `TransferOffer`, `TransferAccept`, `ChunkData`, `ChunkAck`, `TransferQuery`, `TransferCancel`, and `ProtocolError`. Include protocol version and request IDs. Reject unknown critical variants and malformed lengths. Apply per-peer limits to in-flight requests and bytes.

## SQLite durability and ACK rules

`messages_outbox` uses `(peer_id, message_id)` as a primary key. `messages_inbox` has the same unique key and stores the committed body and timestamp. On receipt, insert the inbox row in a transaction with `INSERT ... ON CONFLICT DO NOTHING`, commit, then emit `MessageAck(message_id)`. This makes retries idempotent and means an ACK always represents durable receiver state. Sender changes Pending/Sending to Sent only on a matching ACK; after reconnect it resends all unacked IDs. The receiver's unique key turns those retries into the same durable message, not a duplicate.

An outgoing UI submit first commits to the outbox as Pending, then a worker marks it Sending and attempts transport. Failure returns it to Pending or Failed with retryable reason. History queries are indexed by `(peer_id, created_at, message_id)` and read from SQLite.

## Transfer acceptance and restart resume

The sender commits an offer and source metadata before sending `TransferOffer`. The receiver records `AwaitingAcceptance`; no file bytes are accepted until explicit per-transfer user acceptance. On acceptance, create a random `.part` file inside the configured receive directory using exclusive creation, then persist its opaque staging name, expected size/hash, and state. Never derive a path from a peer-supplied filename.

Use a configurable chunk size defaulting to 1 MiB and a hard configurable maximum file size defaulting to 2 GiB. Stream each logical chunk as small Noise records. The receiver validates transfer ID, index, offset, and exact expected chunk length. Write at the expected offset and sync the staged file before committing that chunk's durable progress in SQLite; only then send `ChunkAck`. If the process crashes after file sync but before DB commit, a retry rewrites the same range safely. If DB says a chunk is durable but the staged file is missing or shorter, mark the transfer failed and do not claim progress.

On reconnect, exchange transfer IDs, manifest hash, and committed chunk bitmap (or compact ranges). Resume only when ID, size, chunk size, and SHA-256 manifest match. Sender rereads the original source and retransmits missing chunks; if the source is unavailable or no longer matches, fail visibly and retain the receiver's partial file for an explicit retry decision. Never trust a sender-provided offset without comparing it to receiver committed state.

When all chunks are committed, stream-hash the staged file and compare SHA-256. Finalize with an atomic no-replace rename into a sanitized, collision-free destination name. Sync the containing directory where supported, then commit Complete. Enforce a total staging quota and per-peer concurrent-transfer limit. Cancellation stops workers promptly and persists Cancelled; cleanup follows a bounded retention policy rather than deleting arbitrary paths.

## Loopback API and browser boundary

Bind the API to `127.0.0.1:38761` only. Validate `Host` against the exact loopback authority and `Origin` against the packaged UI origin (plus an explicit development origin only in dev builds). Reject absent or unexpected origins on browser mutation requests, reject remote bind addresses, and disable permissive CORS. Serve the packaged UI from the same origin when possible.

Require a random per-install or per-launch bearer credential for REST and WebSocket APIs. Bootstrap it only to the local UI after the same Host/Origin checks, keep it out of URLs and logs, and require it on every API and websocket connection. Apply an additional origin check to websocket upgrades. A local port is not an authorization boundary by itself. Use constant-time token comparison and rotate/revoke sessions on logout or agent restart.

REST commands: `GET /api/devices`, `POST /api/pairings`, `POST /api/pairings/{id}/approve`, `POST /api/pairings/{id}/reject`, `POST /api/devices/{id}/revoke`, `GET|POST /api/devices/{id}/messages`, `POST /api/transfers`, `POST /api/transfers/{id}/accept`, `POST /api/transfers/{id}/cancel`, and `GET /api/transfers/{id}`. `GET /api/events` is an authenticated websocket for device, message, pairing, and transfer updates. API responses acknowledge accepted commands; delivery and transfer completion are separate persisted events.

## Why this is the MVP shape

SQLite provides one recoverable source of truth for history, delivery, and transfer progress. Per-peer actors serialize a cryptographic stream and isolate backpressure without serializing every peer behind one actor. New Noise handshakes restore authenticated connections after restart while pairing state and unacknowledged work remain durable. Chunk-level durable ACKs make resume predictable and idempotent. The separation leaves the UI thin and keeps security checks at the network and serialization boundaries.
