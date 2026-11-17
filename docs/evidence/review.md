# Independent review record

Two read-only backend reviews examined `4c6aeb7efc66c38dc853e08f24ee998103ad626c`. A separate reviewer examined the delivery scripts and browser changes. Review findings were kept separate from implementation. Native Windows execution was unavailable to the reviewers.

## Backend findings

- JSON escaping could expand accepted message content beyond the Noise frame limit and poison the outbox.
- Unbounded wire timestamps could make the browser fail when formatting historical records.
- An outer pairing timeout could cancel the owner before it removed a pending confirmation.
- Removed discovery records could consume the device capacity permanently.
- A valid Unicode display name could exceed one mDNS TXT property and prevent restart.
- Long-lived authenticated connections held handshake permits.
- Completed incoming records consumed storage quota after their files were moved or removed.
- Uploads required room for the configured maximum instead of their actual size.
- Missing or truncated outgoing snapshots caused repeated connection failure without a durable error.

The reviewer found message ACK ordering and duplicate comparison consistent with durable delivery. File publication used no-replace semantics. These observations did not establish crash or power-loss guarantees on Windows.

## Delivery findings and validation

Review corrected mixed-batch upload navigation so an oversized file error remains visible. This was then verified in the real browser with a normal file and a sparse 3 GiB file. Only the normal file was uploaded.

The macOS stop script now propagates process-enumeration failures. A sandbox-denied enumeration returned exit 126 instead of reporting success. Windows launch scripts now retain standard output/error logs and show interactive startup failure; their text is ASCII for Windows PowerShell 5.1 compatibility. Both install and uninstall wait for the exact installed Agent process to stop before replacing or removing files. Windows scripts received static review only.

Shell syntax, Python syntax, YAML parsing, Web lint and 13 frontend tests passed in the delivery review. The reviewer found no new code comments that concealed workarounds or suppressed checks.

## Remaining gate

The final independent code review passed for the crates diff against `4c6aeb7`, SHA-256 `bd73f8dd9e7d6ed3af17530b58794e9f38b33b500994575f6aeb8cb0ad893e4b`. It covered the nine initial fixes, upload admission ownership, typed source failure with remote cancellation, stale pairing approval invalidation, and per-process discovery names. No remaining P1/P2 was found. The author and the root separately ran the final 27 Rust tests successfully; strict Clippy and formatting passed. The packaged binary then passed the complete same-host integration suite, including bilateral 1 GiB, verified resume offset, rejection, corruption and revocation. Physical Mac/Windows acceptance remains open regardless of review results.

## Physical discovery correction and integration tests

Physical Mac/Windows validation found that calling mDNS `verify` every 20 seconds with a 10-second deadline prematurely removed a healthy endpoint after missed multicast replies. The failing regression was committed before the one-line fix removing that forced deadline. Independent review of `fad384c` found no remaining P1/P2 in the bounded fix or test changes. The test retains its 25-second goodbye deadline and permits up to 135 seconds for Windows hard termination without goodbye. See [physical final evidence](../physical-final.md).

The additional controlled IP migration test in `a128770` received independent review and real execution against the installed Mac binary. Review moved socket binding before advertisement; the final run also verified all owned processes exited. Only test scripts changed after the three-platform CI candidate `fad384c`; application and packaging source remained identical.
