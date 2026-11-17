# Architecture decision
LANLink keeps durable state in SQLite and ephemeral cryptographic state inside each TCP connection. We choose the direct services design from sol, with per-peer serialization for trust-sensitive commits. A global actor would serialize unrelated disk work. Transport tasks never hold an application lock during network I/O.

Device identity is SHA-256 of a persistent Noise static public key. Typed messages and transfers have explicit states. A fresh Noise XX handshake authenticates every connection. First pairing adds nonce commit/open, then requires two explicit human approvals and two durable Ready responses. Reconnect requires exact pinned keys and bilateral Ready. A revoked peer cannot silently re-pair. These are contract requirements, not verified implementation claims.

File uploads become private immutable snapshots. Use opaque UUID paths, sequential 1MiB logical chunks fragmented below the Noise frame limit, sync-before-offset-commit, resume by receiver's durable offset, final SHA-256 and atomic no-clobber publication. Cross-platform filesystem finalization needs actual native tests. Completed files must be accessible to the user in a downloads directory.

Local HTTP binds loopback. Validate Host and Origin, require per-launch token on mutations and WebSocket. Bootstrap token only through same-origin fetch with Fetch Metadata checks; a local OS process can access its own Agent. No permissive CORS or CDN assets. WebSocket invalidates React queries. UI state stays in Zustand.

## Candidate review
Candidates are archived in docs/design-candidates. sol and astra independently require nonce commitments for the pairing code. terra and luna omit commitments and will not be used for SAS. sol's database-only publication is insufficient for the explicit destination-write requirement; use astra's no-clobber publication and recovery. terra's isolated Noise ownership and explicit receiver quota handling are retained. One XX handshake path is preferred over a separate IK path in MVP.

## Phase gates
P1 requires real local HTTP and restart identity/database persistence. P2 requires observed mDNS advertisements and resolved peers. P3 requires rejection of untrusted traffic, bilateral approval, restart pins and revoke. P4 requires bidirectional durable ACK and duplicate suppression. P5 requires accepted streaming transfers, interruption/recovery and matching hashes. P6 requires browser interaction against actual Agent data. P7 requires native macOS and Windows installation plus physical LAN tests. A local two-Agent test does not satisfy P7.
