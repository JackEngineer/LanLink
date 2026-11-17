# LANLink MVP architecture candidate

LANLink users need authenticated LAN chat and resumable file transfer between macOS and Windows. This candidate keeps one Rust process, one SQLite database, and one local web application. It favors explicit state ownership over an unrestricted collection of services. This is a proposed design, not runtime validation.

## State and module ownership

`domain.rs` defines `DeviceId`, `MessageId`, `TransferId`, `SessionId`, `TrustEpoch`, `PeerState`, `MessageStatus`, and `TransferState`. DeviceId is the full SHA256 of the persistent Noise static public key. UUIDs identify messages and transfers. Names and discovery addresses never establish identity.

`PeerState` is `Discovered`, `Pairing`, `Trusted`, or `Revoked`. Pairing contains a session identifier, transcript hash, verified remote key, local consent, and remote consent. A session has a separate state of `Handshaking`, `Confirming`, or `Authorized`. `TransferState` is `Offered`, `Accepted`, `Receiving`, `Paused`, `Verifying`, `Complete`, `Cancelled`, or `Failed`. Only constructors and transition functions can produce the next state.

`core.rs` runs one command actor. It owns peer trust, session registrations, message dispatch decisions, transfer metadata, and the sole database writer. HTTP handlers and network tasks submit typed commands and await replies. The actor performs short SQLite operations, never file hashing or network writes. `store.rs` contains SQL and migrations without another state cache. `identity.rs` creates and protects the static key. `discovery.rs` emits untrusted observations and advertises the actual bound TCP port. `session.rs` owns each socket and its Noise transport state. `transfer.rs` owns bounded disk workers. `http.rs` validates HTTP boundaries. `main.rs` wires these modules.

Use bounded channels and admission limits. A session task owns both Noise directions through a framed socket loop. Avoid cancelling partially completed `read_exact` operations. Close a connection after a partial write timeout. A disk worker receives one transfer and one generation number. Its completion is an event, never a direct mutation of shared application state.

This needs no actor framework. Tokio channels, enums, and reply channels suffice. Direct stateless helpers remain ordinary functions. The actor earns its place because revoke, reconnect, ACK, and retry otherwise race across services.

## Pairing and authenticated sessions

Use `Noise_XX_25519_ChaChaPoly_SHA256` for both first pairing and reconnect. Bind the protocol version, connection purpose, and ordered handshake roles into a fixed-format prologue. Fresh connections always create fresh Noise state and nonces. Reconnect checks the received static key against the durable trusted key before permitting application messages. A known ID with another key is an error, never a replacement. XX avoids implementing a second handshake path for the MVP.

First pairing accepts only bounded pairing control frames after the handshake. Save the complete handshake hash before converting to transport state. Exchange a random 32-byte nonce commitment, then reveal the nonce only after receiving the peer commitment. A commitment hashes a domain label, handshake hash, role, and nonce. Both peers verify the commitment before displaying a code. Compute the code from a second domain label, handshake hash, ordered static public keys, and both ordered nonces. Display eight decimal digits in two groups. The commit-and-reveal exchange prevents a malicious intermediary from choosing its final contribution after seeing the other contribution. A truncated handshake hash alone does not provide that property.

Local approval must name the current pairing SessionId. Send an encrypted approval bound to the transcript and the code digest. Denial, expiry, disconnect before bilateral approval, or any mismatch cancels the session. Rate-limit pairing attempts and allow one active pairing per peer. A pair becomes trusted only after local approval and a matching remote approval. Persist that transition before sending `PairReady`. Do not emit chat or file data until receiving `PairReady` too.

A disconnect between the two durable trust writes can leave one side trusted. Treat this as a recoverable incomplete pairing, not successful bilateral setup. On reconnect, both sides exchange their trust state in the control-only phase. If either side lacks trust, display a fresh comparison and require bilateral approval again. Never infer missing consent from an existing pin on the other machine. Reject revoked keys unless the local user explicitly initiates re-pairing.

Revoke increments the durable TrustEpoch, marks the key revoked, cancels all its sessions and disk workers, and invalidates queued commands from earlier epochs. Acknowledge the HTTP revoke only after those workers have stopped. The core checks epoch immediately before committing inbound messages or advancing transfers. Network workers also reject stale queued sends. No application work can commit after the revoke response. Already delivered bytes cannot be recalled.

The Noise framework defines the handshake hash and transport transition in its [official specification](https://noiseprotocol.org/noise.html). The proposed SAS exchange is an application protocol that needs adversarial review and tests.

## Persistence and durable message ACK

Use SQLite WAL, `foreign_keys=ON`, `synchronous=FULL`, and schema migrations. Store peers with public key, trust status, and epoch. Store messages with a globally unique ID, peer, direction, body digest, body, status, and timestamps. Store transfers with peer, direction, immutable manifest, state, durable offset, staging identifier, finalization identifier, and failure reason. Add explicit outbox records or an indexed pending-message query.

Sending a message commits its row before queueing network work. Receiving a message validates identity, size, and epoch, then inserts the message in a transaction. Only a successful commit permits `MessageAck`. A duplicate with identical peer and content returns the same ACK. A reused ID with different peer or content is a protocol violation. Sender marks `sent` only after its authenticated peer ACK. Connection loss restores `sending` to `pending`; an explicit permanent rejection becomes `failed`. Restart and reconnect replay the pending outbox. History reads the same durable rows.

## Files and restart recovery

Offer an immutable manifest containing TransferId, byte length, SHA256, and display filename. The receiver must explicitly accept every transfer. Trust alone grants no file acceptance. Reserve quota transactionally before acceptance. Enforce the default 2 GiB per-file cap, total reserved staging quota, concurrent transfer count, and free-space checks. Space checks do not replace handling disk-full errors.

A sender copies selected data into private bounded staging and hashes that snapshot before offering it. This prevents the source changing between hashing and transmission and supports restart without browser access to the original file. Count sender snapshots against its own quota. A cancelled HTTP upload removes its incomplete staging record and file during reconciliation.

Logical chunks default to 1 MiB. Each chunk is split into binary transport fragments of at most 60 KiB of file data. Include transfer ID, absolute offset, and final-fragment flag in each authenticated frame. Avoid JSON byte arrays or base64 for data frames. Accept only the next contiguous offset and cap all frame and buffer lengths before allocation. Snow limits an encrypted message to 65,535 bytes, including its encryption overhead, as documented in [TransportState](https://docs.rs/snow/latest/snow/struct.TransportState.html).

After a full logical chunk, the receiver writes it, calls `sync_data`, commits the new durable offset, and then ACKs that offset. On restart, truncate staging bytes beyond the recorded offset. If the file is shorter than its recorded offset, mark it failed and require a verified restart. The receiver returns its durable offset only for a previously accepted matching manifest from the same trusted peer. Resume starts there on a fresh authenticated connection. It never restores Noise transport nonces. Duplicate older fragments return the current offset. Gaps fail the current transfer session.

After the last chunk, verify full SHA256, sync the file, and finalize without replacement. Use a private UUID directory within the selected destination filesystem, so staging and destination share a filesystem. Prefer an atomic no-clobber hard link followed by staging unlink where supported. A platform-specific no-replace rename is another option. Never implement a separate existence check followed by ordinary rename. Store a `Verifying` or finalizing record first. Startup checks the known staging and final paths and reconciles their hashes before marking complete. Return the final completion ACK only after durable completion. Exact duplicates can then return the same result.

Use generated storage names. Treat the offered filename as display text and sanitize it separately for optional export, including Windows reserved names and trailing dots. Reject path separators, traversal, and symlink escapes. The receiver's approved destination is the only writable root. Cancellation closes handles before removing only the owned staging object and releasing quota. Retain explicit acceptance when an interrupted transfer resumes. New manifests require new acceptance.

## Local API and UI contract

Production Rust serves compiled UI assets and the API on `127.0.0.1:38761`. Bind explicitly, validate the peer socket address, and allow only the exact configured Host. Require the exact local Origin when supplied; reject `null`, wildcard CORS, and other origins. WebSocket upgrades always require the exact local Origin.

Generate a random local API bearer secret for each launch. The native launcher opens the UI with the token in a URL fragment. JavaScript removes that fragment immediately and retains the token in tab memory. Do not expose a token endpoint, print the token, place it in query parameters, or persist it in localStorage. Browser API calls send Authorization. A bearer-authorized POST obtains a short-lived, one-use WebSocket ticket. The WebSocket must authenticate its first frame before any events are emitted. Apply a short authentication timeout. A missing Origin is permitted for a bearer-authenticated CLI client, never as a substitute for authentication.

The API provides snapshot and history GETs, plus POST commands for pair, approve, deny, revoke, send message, upload file, accept transfer, cancel, retry, and settings. Commands use caller-provided UUID idempotency keys where replay could duplicate work. File uploads stream with body limits. WebSocket events contain a monotonic revision and entity IDs. A revision gap triggers refetch. TanStack Query owns server state; Zustand owns navigation, selected peer, and unsent input. Show discovered and trusted states separately. No fictitious device data enters acceptance evidence.

## Design checks

Model the Domain shaped the explicit pairing and transfer enums. Make Operations Idempotent shaped message uniqueness, receiver offsets, outbox replay, and finalization reconciliation. Tests must inject failure between file sync and database commit, between commit and ACK, and between finalization and completed-state persistence. Exercise wrong keys, mismatched codes, one-sided approval, stale approvals, revoked active sessions, DNS rebinding Host values, hostile Origins, duplicate IDs, disk exhaustion, and disconnect during each file stage. macOS-only tests cannot satisfy the brief's Windows and 1 GB cross-machine release gates.
