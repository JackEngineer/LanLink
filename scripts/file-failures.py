#!/usr/bin/env python3
"""Check file failure handling with two real LANLink Agents on one host."""

import argparse
from contextlib import ExitStack
import os
from pathlib import Path
import signal
import tempfile
import traceback
from uuid import UUID

from smoke import (
    Agent, SEND_REJECTION, deny_message, discover, download_dir, expect_offer,
    file_digest, make_file, pair, ports, require, send_and_ack, wait_for, wait_for_file,
)


def run(args, run_dir, log):
    a_http, a_tcp, b_http, b_tcp = ports(4)
    a = Agent("A", args, run_dir, a_http, a_tcp)
    b = Agent("B", args, run_dir, b_http, b_tcp)
    timeout = args.timeout
    with ExitStack() as cleanup:
        cleanup.callback(a.stop)
        cleanup.callback(b.stop)
        a.start()
        b.start()
        discover(a, b)
        discover(b, a)
        pair(a, b)
        log("PASS two isolated Agents discovered each other through mDNS and paired with matching codes")

        large = run_dir / "large.bin"
        large_sha = make_file(large, 4096)
        downloads = download_dir(b, run_dir)
        completed, completed_dest = expect_offer(a, b, large, large_sha, timeout, downloads)
        b.change("POST", f"/transfers/{completed}/accept")
        wait_for_file(a, b, completed, completed_dest, 4096, large_sha, timeout)

        offered, rejected_dest = expect_offer(a, b, large, large_sha, timeout, downloads)
        settings = b.request("PATCH", "/settings", payload={"max_file_size": 1024})
        require(settings["max_file_size"] == 1024, "Receiver limit did not change")
        new = a.upload(b, large, timeout)["id"]
        wait_for("new and existing oversized transfers become rejected",
                 lambda: (a.transfer(new), a.transfer(offered), b.transfer(offered)),
                 lambda rows: all(row and row["status"] == "rejected" for row in rows), timeout)
        for transfer_id in (new, offered):
            snapshot = a.data_dir / "spool" / f"{UUID(transfer_id)}.send"
            require(not snapshot.exists(), f"Rejected sender snapshot remained in spool: {snapshot}")
        require(b.transfer(new) is None, "Receiver persisted an over-limit offer")
        require(not rejected_dest.exists() and not (downloads / f"{UUID(new)}-{large.name}").exists(),
                "Receiver published an over-limit file")
        wait_for_file(a, b, completed, completed_dest, 4096, large_sha, timeout)
        require(b.download(completed, timeout) == (4096, large_sha),
                "Already completed download changed after lowering the limit")
        send_and_ack(a, b, "chat after a receiver size limit rejection")
        send_and_ack(b, a, "receiver chat after a size limit rejection")
        log("PASS new and existing over-limit offers rejected; completed SHA-256 and bidirectional chat ACK verified")

        small = run_dir / "small.bin"
        small_sha = make_file(small, 512)
        healthy, healthy_dest = expect_offer(a, b, small, small_sha, timeout, downloads)
        b.change("POST", f"/transfers/{healthy}/accept")
        wait_for_file(a, b, healthy, healthy_dest, 512, small_sha, timeout)
        log("PASS below-limit file published with matching SHA-256")

        damaged, damaged_dest = expect_offer(a, b, small, small_sha, timeout, downloads)
        snapshot = a.data_dir / "spool" / f"{UUID(damaged)}.send"
        require(snapshot.resolve(strict=True) == run_dir / a.label / "spool" / snapshot.name
                and snapshot.is_file(), f"Source snapshot is outside this run or is not a file: {snapshot}")
        with snapshot.open("r+b") as file:
            file.truncate(9)
            file.flush()
            os.fsync(file.fileno())
        wait_for("damaged source failure and receiver cancellation",
                 lambda: (a.transfer(damaged), b.transfer(damaged)),
                 lambda rows: all(rows) and rows[0]["status"] == "failed"
                 and rows[0]["failure_kind"] == "source_unavailable"
                 and rows[1]["status"] == "cancelled", timeout)
        require(snapshot.stat().st_size == 9 and not damaged_dest.exists(), "Damaged transfer fixture changed")
        a.request("POST", f"/transfers/{damaged}/retry", denied=SEND_REJECTION)
        a.change("DELETE", f"/devices/{b.id}/pair")
        require(not a.peer(b)["paired"], "Peer trust remained after revoke")
        require(snapshot.stat().st_size == 9, "Revocation discarded the snapshot before explicit cancellation")
        a.change("POST", f"/transfers/{damaged}/cancel")
        require(a.transfer(damaged)["status"] == "cancelled" and not snapshot.exists(),
                "Explicit cancel failed to discard the owned snapshot")
        deny_message(a, b, "denied after revoke")
        log("PASS damaged source failed and receiver cancelled; retry and revoked chat denied; explicit cancel removed snapshot")


def interrupted(_signum, _frame):
    raise KeyboardInterrupt("test interrupted")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--agent", required=True, type=Path, help="Built lanlink-agent executable")
    parser.add_argument("--artifacts", type=Path, help="Directory in which to keep each run")
    parser.add_argument("--timeout", type=float, default=30, help="Seconds to wait for discovery or an API state")
    parser.set_defaults(web_dir=None)
    args = parser.parse_args()
    args.agent = args.agent.resolve()
    if not args.agent.is_file():
        parser.error(f"Agent binary does not exist: {args.agent}")
    if not 0 < args.timeout <= 3600:
        parser.error("Timeout must be greater than zero and at most 3600 seconds")
    if args.artifacts:
        args.artifacts = args.artifacts.resolve()
        args.artifacts.mkdir(parents=True, exist_ok=True)

    signal.signal(signal.SIGTERM, interrupted)
    run_dir = Path(tempfile.mkdtemp(prefix="lanlink-file-failures-", dir=args.artifacts)).resolve()
    with (run_dir / "summary.log").open("w", encoding="utf-8") as summary:
        def log(line):
            print(line, flush=True)
            print(line, file=summary, flush=True)

        log(f"Artifacts: {run_dir}")
        log(f"Agent: {args.agent}")
        try:
            log(f"Agent SHA-256: {file_digest(args.agent)}")
            run(args, run_dir, log)
        except (Exception, KeyboardInterrupt) as exc:
            log(f"FAIL {exc}")
            traceback.print_exc(file=summary)
            return 1
        log("PASS all file failure checks against two real Agents")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
