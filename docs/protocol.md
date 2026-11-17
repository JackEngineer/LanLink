# LANLink protocol v1

This reference describes the v0.1.0 implementation contract. Interoperability remains subject to the tests in `acceptance.md`.

## Discovery and identity

The mDNS service is `_lanlink._tcp.local.`. The TCP listener binds before advertisement and announces its actual port. Discovery metadata is an address hint, never authorization. Device IDs are lowercase hex SHA-256 digests of persistent X25519 public keys. Display names do not identify a peer.

Each process uses a fresh random suffix in its DNS instance and hostname, retaining it for display-name updates. The persistent ID remains in TXT metadata and drives deduplication. This separates restart announcements from cached goodbye records; it does not create a new device identity.

## TCP and encryption

The initiator sends nine bytes. The first eight are ASCII `LANLINK1`. The ninth is zero for authenticated reconnect or one for explicit pairing. These bytes are the Noise prologue.

Each connection uses `Noise_XX_25519_ChaChaPoly_SHA256`. The three handshake messages and all later ciphertext records have a two-byte big-endian length. Handshake input is bounded. A fresh connection always creates fresh Noise session keys. Restoring a file offset never restores a cryptographic nonce.

The encrypted transport plaintext begins with a record-kind byte. Kind zero contains UTF-8 JSON. Kind one contains 16 raw UUID bytes, an eight-byte big-endian absolute offset, a one-byte logical-chunk-end flag and file bytes. Each file fragment contains at most 60 KiB. Logical chunks are 1 MiB. The full plaintext plus its 16-byte authentication tag remains within Noise's 65,535-byte limit.

## Pairing

A Hello exchanges display metadata after the handshake. The claimed device ID must equal the digest of the authenticated remote static key. Dialing also checks the discovered target ID.

Pairing exchanges SHA-256 commitments before either side opens a fresh 32-byte random nonce. The commitment contains the versioned domain, full handshake hash, role byte and nonce. The displayed security code contains the first 40 bits of a separate domain-separated hash over the handshake hash and ordered nonces.

The user compares the complete code on both computers and confirms separately on each. Approve identifies the full handshake hash. The Agent stores the pinned identity only after both confirmations. Each side sends Ready after persistence. Application traffic begins only after both Ready messages. Reconnect requires an active exact-key pin. Rejected, expired, stale and disconnected approvals do not authorize a new connection.

## Control records

The JSON `type` discriminant is snake_case. The shared Rust enum in `crates/protocol/src/lib.rs` is authoritative.

| Type           | Payload                                   | Meaning                                     |
| -------------- | ----------------------------------------- | ------------------------------------------- |
| hello          | device                                    | Identity metadata checked against Noise key |
| commit         | value                                     | Pairing nonce commitment                    |
| open           | nonce                                     | Pairing nonce opening                       |
| approve        | session                                   | Local user confirmation for this handshake  |
| ready          | session                                   | Durable trust permits application traffic   |
| deny           | none                                      | Pairing refused                             |
| chat           | id, body, created_at                      | Text delivery with stable UUID              |
| chat_ack       | id                                        | Receiver persisted text                     |
| offer          | id, name, size, sha256, created_at, retry | File manifest requiring consent             |
| transfer_state | id, status, offset                        | Receiver consent or durable progress        |
| cancel         | id                                        | Transfer cancelled                          |
| ping           | none                                      | Connection heartbeat                        |

## Durable delivery

The sender persists a text message before attempting delivery. The receiver checks duplicate IDs against the peer, body, direction and timestamp. A conflicting duplicate fails. A matching repeat returns the same ACK. The sender marks sent only after receiving the authenticated ACK.

A file offer contains an immutable manifest. The sender retains a snapshot so browser closure and source-file changes cannot alter a resumed transfer. The receiver accepts the manifest before allocating a partial file. Offset ACK follows file synchronization and SQLite offset persistence. Reconnection uses the receiver's durable contiguous offset. Missing or shorter acknowledged data fails visibly.

Completion requires exact length and SHA-256 equality. Publication uses an opaque UUID-prefixed safe filename and a no-replace operation. Completion receipts survive reconnection. Cancellation and revocation persist terminal state and prevent automatic resumption.

## Limits and trust boundary

The default file cap is 2 GiB, logical chunk size 1 MiB and text limit 16 KiB. The storage budget is 8 GiB. The backend bounds pending transfers, handshake admission, queues and requests. File payloads never require loading the entire file into memory.

The local browser API belongs to the logged-in OS user. It checks loopback binding, exact Host, browser Origin and a per-launch token for mutations and WebSocket access. It does not protect against a malicious process running as that same OS user.

## References

- [Noise specification](https://noiseprotocol.org/noise.html).
- [Snow transport size limits](https://docs.rs/snow/0.10.0/snow/struct.TransportState.html).
- [Windows FlushFileBuffers access requirements](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-flushfilebuffers).

Power-loss guarantees for directory metadata differ between platforms. Native process-crash recovery tests cannot establish storage-device power-loss behavior.
