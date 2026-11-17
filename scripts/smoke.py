#!/usr/bin/env python3
"""Exercise two real LANLink Agents on their advertised mDNS endpoints."""

import argparse
import hashlib
import http.client
import json
import os
from pathlib import Path
import platform
import re
import signal
import socket
import subprocess
import tempfile
import time
import traceback
from urllib.parse import urlencode
import uuid

API = "/api/v1"
MIB = 1024 * 1024
HTTP_TIMEOUT = 4
MDNS_HOST_TTL = 120
MDNS_GOODBYE_TIMEOUT = 25
READ_LIMIT = 8 * MIB
TERMINAL_FAILURES = {"failed", "rejected", "cancelled"}
REVOKE_OBSERVATION = 5
SEND_REJECTION = {400, 401, 403, 409}


class CheckFailed(RuntimeError):
    pass


def require(condition, reason):
    if not condition:
        raise CheckFailed(reason)


def wait_for(description, observe, ready, timeout, interval=0.15):
    deadline = time.monotonic() + timeout
    while True:
        last = observe()
        if ready(last):
            return last
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise CheckFailed(f"Timed out waiting for {description}. Last value: {last!r}")
        time.sleep(min(interval, remaining))


class Agent:

    def __init__(self, label, args, run_dir, http_port, tcp_port):
        self.label = label
        self.name = f"QA {label} {run_dir.name[-8:]}"
        self.data_dir = run_dir / label
        self.log_path = run_dir / f"{label}.log"
        self.http_port = http_port
        self.tcp_port = tcp_port
        self.host = f"127.0.0.1:{http_port}"
        self.origin = f"http://{self.host}"
        self.timeout = args.timeout
        self.process = None
        self.token = None
        self.device = None
        self.argv = [
            str(args.agent), "--data-dir", str(self.data_dir),
            "--http-port", str(http_port), "--tcp-port", str(tcp_port),
            "--name", self.name,
        ]
        if args.web_dir:
            self.argv += ["--web-dir", str(args.web_dir)]

    def start(self):
        require(self.process is None, f"{self.label} is already started")
        options = (
            {"creationflags": subprocess.CREATE_NEW_PROCESS_GROUP}
            if os.name == "nt" else {"start_new_session": True}
        )
        with self.log_path.open("ab") as log:
            log.write(f"\nStarting {self.argv!r}\n".encode())
            log.flush()
            self.process = subprocess.Popen(
                self.argv, stdin=subprocess.DEVNULL, stdout=log, stderr=subprocess.STDOUT,
                env={**os.environ, "RUST_LOG": os.environ.get("LANLINK_TEST_LOG", "lanlink_agent=info")}, **options,
            )

        def healthy():
            try:
                return self.get("/health")
            except (ConnectionRefusedError, ConnectionResetError):
                return None

        health = wait_for(f"{self.label} health", healthy, bool, self.timeout)
        require(health.get("status") == "ok" and health.get("version") == "0.1.0",
                f"{self.label} returned unexpected health: {health!r}")
        session = self.request("GET", "/session", headers={"Sec-Fetch-Site": "same-origin"})
        self.token = session["token"]
        require(isinstance(self.token, str) and self.token, f"{self.label} omitted the session token")
        self.device = self.get("/device")
        require(self.device["name"] == self.name and self.device["id"] and self.device["platform"],
                f"{self.label} returned unexpected device identity: {self.device!r}")

    def stop(self, crash=False):
        process = self.process
        if process is None:
            return
        self.process = None
        self.token = None
        if process.poll() is not None:
            return
        try:
            if os.name == "nt":
                process.kill() if crash else process.terminate()
            else:
                os.killpg(process.pid, signal.SIGKILL if crash else signal.SIGTERM)
        except ProcessLookupError:
            pass
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            if os.name == "nt":
                process.kill()
            else:
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
            process.wait(timeout=5)

    @property
    def id(self):
        return self.device["id"]

    def request(self, method, path, *, payload=None, body=None, headers=None,
                timeout=HTTP_TIMEOUT, denied=None):
        require(self.process is not None, f"{self.label} has not started")
        require(self.process.poll() is None,
                f"{self.label} exited with {self.process.returncode}. See {self.log_path}")
        request_headers = {"Host": self.host}
        if method != "GET":
            request_headers["Origin"] = self.origin
            if self.token:
                request_headers["X-LANLink-Token"] = self.token
        if payload is not None:
            body = json.dumps(payload).encode()
            request_headers["Content-Type"] = "application/json"
        if headers:
            request_headers.update(headers)
        connection = http.client.HTTPConnection("127.0.0.1", self.http_port, timeout=timeout)
        try:
            connection.request(method, API + path, body=body, headers=request_headers)
            with connection.getresponse() as response:
                status = response.status
                raw = response.read(READ_LIMIT + 1)
        finally:
            connection.close()
        context = f"{self.label} {method} {path} returned HTTP {status}"
        require(len(raw) <= READ_LIMIT, f"{context} with an oversized response")
        if denied is not None:
            require(status in denied, f"{context}. Expected rejection. Body: {raw[:500]!r}")
            return status
        require(200 <= status < 300, f"{context}. Body: {raw[:500]!r}")
        try:
            return json.loads(raw)
        except ValueError as exc:
            raise CheckFailed(f"{context} with invalid JSON: {raw[:500]!r}") from exc

    def get(self, path):
        return self.request("GET", path)

    def change(self, method, path, payload=None, **kwargs):
        response = self.request(method, path, payload=payload, **kwargs)
        require(response.get("ok") is True, f"{self.label} {path} did not report success: {response!r}")

    def peer(self, peer):
        return find(self.get("/devices"), peer.id)

    def messages(self, peer):
        return self.get("/messages?" + urlencode({"peer_id": peer.id}))

    def transfer(self, transfer_id):
        return find(self.get("/transfers"), transfer_id)

    def download(self, transfer_id, timeout):
        connection = http.client.HTTPConnection("127.0.0.1", self.http_port, timeout=HTTP_TIMEOUT)
        digest = hashlib.sha256()
        size = 0
        deadline = time.monotonic() + timeout
        try:
            connection.request("GET", f"{API}/transfers/{transfer_id}/download",
                               headers={"Host": self.host})
            with connection.getresponse() as response:
                require(response.status == 200, f"{self.label} download returned HTTP {response.status}")
                require(response.getheader("Content-Disposition", "").startswith("attachment;"),
                        "Download was not served as an attachment")
                for chunk in iter(lambda: response.read(MIB), b""):
                    require(time.monotonic() < deadline, f"Download {transfer_id} timed out")
                    digest.update(chunk)
                    size += len(chunk)
                require(response.getheader("Content-Length") == str(size),
                        f"Download declared a different size. Read {size} bytes.")
        finally:
            connection.close()
        return size, digest.hexdigest()

    def upload(self, peer, path, timeout, denied=None):
        boundary = f"lanlink-qa-{uuid.uuid4().hex}"
        head = (
            f"--{boundary}\r\n"
            f'Content-Disposition: form-data; name="file"; filename="{path.name}"\r\n'
            "Content-Type: application/octet-stream\r\n\r\n"
        ).encode()
        tail = f"\r\n--{boundary}--\r\n".encode()
        headers = {
            "Content-Type": f"multipart/form-data; boundary={boundary}",
            "Content-Length": str(len(head) + path.stat().st_size + len(tail)),
        }
        deadline = time.monotonic() + timeout
        with path.open("rb") as file:
            def chunks():
                yield head
                for chunk in iter(lambda: file.read(MIB), b""):
                    require(time.monotonic() < deadline, f"Upload of {path.name} timed out")
                    yield chunk
                yield tail

            return self.request("POST", "/transfers?" + urlencode({"peer_id": peer.id}),
                                body=chunks(), headers=headers, timeout=timeout, denied=denied)


def find(rows, row_id):
    require(isinstance(rows, list), f"Expected a JSON array, got {type(rows).__name__}")
    matches = [row for row in rows if row["id"] == row_id]
    require(len(matches) <= 1, f"Duplicate ID {row_id!r} in response")
    return matches[0] if matches else None


def ports(count):
    reservations = []
    try:
        for _ in range(count):
            reservation = socket.socket()
            reservations.append(reservation)
            reservation.bind(("127.0.0.1", 0))
        return [reservation.getsockname()[1] for reservation in reservations]
    finally:
        for reservation in reservations:
            reservation.close()


def make_file(path, size):
    digest = hashlib.sha256()
    with path.open("xb") as file:
        remaining = size
        while remaining > 0:
            chunk = os.urandom(min(remaining, MIB))
            file.write(chunk)
            digest.update(chunk)
            remaining -= len(chunk)
    return digest.hexdigest()


def file_digest(path):
    digest = hashlib.sha256()
    with path.open("rb") as file:
        for chunk in iter(lambda: file.read(MIB), b""):
            digest.update(chunk)
    return digest.hexdigest()


def check_http_guards(agent):
    rejection = {400, 401, 403, 421}
    agent.request("GET", "/health", headers={"Host": "invalid.example"}, denied=rejection)
    agent.request("GET", "/session", denied=rejection)
    agent.request("GET", "/session", headers={"Sec-Fetch-Site": "cross-site"}, denied=rejection)
    agent.request("PATCH", "/device", payload={"name": "Wrong origin"}, denied=rejection,
                  headers={"Origin": "http://invalid.example"})
    agent.request("PATCH", "/device", payload={"name": "Wrong token"}, denied=rejection,
                  headers={"X-LANLink-Token": "invalid-token"})
    require(agent.get("/device") == agent.device, f"{agent.label} changed after a rejected request")


def deny_message(sender, recipient, body):
    sender.request("POST", "/messages", payload={"peer_id": recipient.id, "body": body},
                   denied=SEND_REJECTION)
    require(all(row["body"] != body for row in recipient.messages(sender)),
            f"{recipient.label} received a denied message")


def discover(agent, peer):
    row = wait_for(f"{agent.label} mDNS discovery of {peer.label}", lambda: agent.peer(peer),
                   lambda row: row is not None and row["online"] and row["addresses"]
                   and row["port"] == peer.tcp_port, agent.timeout)
    require(row["name"] == peer.name and row["platform"] == peer.device["platform"]
            and row["port"] == peer.tcp_port and row["addresses"],
            f"{agent.label} got incorrect mDNS fields for {peer.label}: {row!r}")
    return row


def check_discovery_lease(observer, advertiser, log):
    observed = discover(observer, advertiser)
    device_id = advertiser.id
    addresses = set(observed["addresses"])
    pid = advertiser.process.pid
    silence_seconds = 35
    log(f"PASS A discovered B at {', '.join(sorted(addresses))} port {observed['port']}")
    os.kill(pid, signal.SIGSTOP)
    try:
        start = time.monotonic()
        while True:
            row = observer.peer(advertiser)
            elapsed = time.monotonic() - start
            require(row is not None and row["online"] and not addresses.isdisjoint(row["addresses"])
                    and row["port"] == observed["port"],
                    f"A lost B's advertised endpoint after {elapsed:.1f}s of mDNS silence: {row!r}")
            if elapsed >= silence_seconds:
                break
            time.sleep(min(0.25, silence_seconds - elapsed))
    finally:
        os.kill(pid, signal.SIGCONT)
    log(f"PASS A kept B's advertised endpoint through {silence_seconds}s of mDNS silence")

    advertiser.stop()
    wait_for("B's mDNS goodbye to remove the discovery record", lambda: observer.peer(advertiser),
             lambda row: row is None, MDNS_GOODBYE_TIMEOUT)
    log("PASS B's graceful shutdown removed its discovery record")

    advertiser.start()
    require(advertiser.id == device_id, "B device ID changed on restart")
    discover(observer, advertiser)
    log("PASS A rediscovered B with the same device ID after restart")


def pair(left, right):
    left_attempt_id = left.request("POST", f"/devices/{right.id}/pair")["id"]
    left_attempt = wait_for("A pairing attempt", lambda: find(left.get("/pairings"), left_attempt_id),
                            bool, left.timeout)

    def pending_on_right():
        attempts = [row for row in right.get("/pairings") if row["peer_id"] == left.id]
        require(len(attempts) <= 1, f"B returned duplicate pending attempts: {attempts!r}")
        return attempts[0] if attempts else None

    right_attempt = wait_for("B pairing attempt", pending_on_right, bool, right.timeout)
    require(left_attempt["peer_id"] == right.id and left_attempt["peer_name"] == right.name
            and right_attempt["peer_name"] == left.name, "Pairing attempts identify the wrong peers")
    for attempt in (left_attempt, right_attempt):
        require(not attempt["local_confirmed"] and not attempt["remote_confirmed"],
                "Pairing was confirmed before either approval")
        require(attempt["expires_at"] > time.time() * 1000, "Pairing attempt already expired")
    code = left_attempt["code"]
    require(isinstance(code, str) and code.strip() and code == right_attempt["code"],
            "Pairing codes are missing or disagree")
    left.change("POST", f"/pairings/{left_attempt_id}/confirm", {"accept": True})
    wait_for("B observes A approval", pending_on_right,
             lambda attempt: attempt is not None and attempt["remote_confirmed"], right.timeout)
    require(left.peer(right)["paired"] is False and right.peer(left)["paired"] is False,
            "One approval established trust")
    deny_message(left, right, "Must reject after A-only approval")
    deny_message(right, left, "Must reject before B approves")
    right.change("POST", f"/pairings/{right_attempt['id']}/confirm", {"accept": True})
    wait_for("bilateral trust", lambda: (left.peer(right), right.peer(left)),
             lambda peers: all(peer is not None and peer["paired"] for peer in peers), left.timeout)


def message_with_ack(sender, recipient, message_id, body):
    outgoing = wait_for(f"{sender.label} durable ACK for {message_id}",
                        lambda: find(sender.messages(recipient), message_id),
                        lambda row: row is not None and row["status"] == "sent", sender.timeout)
    incoming = find(recipient.messages(sender), message_id)
    require(outgoing["body"] == body and outgoing["direction"] == "outgoing"
            and outgoing["peer_id"] == recipient.id,
            f"Incorrect outgoing message: {outgoing!r}")
    require(incoming is not None and incoming["body"] == body and incoming["direction"] == "incoming"
            and incoming["peer_id"] == sender.id,
            f"Incorrect incoming message: {incoming!r}")
    return outgoing, incoming


def send_and_ack(sender, recipient, body):
    created = sender.request("POST", "/messages", payload={"peer_id": recipient.id, "body": body})
    return message_with_ack(sender, recipient, created["id"], body)


def verify_history(sender, recipient, snapshots):
    sent, received = snapshots
    require(find(sender.messages(recipient), sent["id"]) == sent, "Outgoing history changed")
    require(find(recipient.messages(sender), sent["id"]) == received, "Incoming history changed")


def restart(agent):
    old_id, old_token = agent.id, agent.token
    agent.stop()
    agent.start()
    require(agent.id == old_id, f"{agent.label} device ID changed on restart")
    agent.request("PATCH", "/device", payload={"name": "Expired token"}, denied={400, 401, 403},
                  headers={"X-LANLink-Token": old_token})


def download_dir(agent, run_dir):
    path = Path(agent.get("/settings")["download_dir"])
    require(path.is_absolute() and run_dir.resolve() in path.resolve().parents,
            f"{agent.label} download directory must be isolated under {run_dir}, got {path}")
    return path


def expect_offer(sender, receiver, file, digest, timeout, destination_dir):
    outgoing = sender.upload(receiver, file, timeout)
    transfer_id = outgoing["id"]
    incoming = wait_for(f"{receiver.label} incoming offer for {file.name}",
                        lambda: receiver.transfer(transfer_id),
                        lambda row: row is not None and row["status"] == "offered",
                        receiver.timeout)
    require(outgoing["status"] == "awaiting_accept", f"Unexpected outgoing offer: {outgoing!r}")
    for transfer, peer, direction in ((outgoing, receiver, "outgoing"), (incoming, sender, "incoming")):
        require(transfer["peer_id"] == peer.id and transfer["direction"] == direction
                and transfer["name"] == file.name and transfer["size"] == file.stat().st_size
                and transfer["sha256"] == digest and transfer["offset"] == 0,
                f"Incorrect {direction} transfer metadata: {transfer!r}")
    destination = destination_dir / f"{transfer_id}-{file.name}"
    require(not destination.exists(), f"Receiver published {file.name} before acceptance")
    receiver.request("GET", f"/transfers/{transfer_id}/download", denied={400, 403, 404, 409})
    return transfer_id, destination


def wait_for_file(sender, receiver, transfer_id, destination, size, digest, timeout):
    def both_transfers():
        rows = (sender.transfer(transfer_id), receiver.transfer(transfer_id))
        require(all(row is None or row["status"] not in TERMINAL_FAILURES for row in rows),
                f"Transfer ended without completing: {rows!r}")
        return rows

    both = wait_for(f"{destination.name} to complete on both Agents", both_transfers,
                    lambda rows: all(row is not None and row["status"] == "completed" for row in rows),
                    timeout)
    require(all(row["offset"] == size and row["sha256"] == digest for row in both),
            f"Wrong final offset or digest: {both!r}")
    require(destination.is_file() and destination.stat().st_size == size,
            f"Incorrect or missing published file: {destination}")
    require(file_digest(destination) == digest, f"Published bytes have the wrong SHA-256: {destination}")


def stream_offsets(agent, transfer_id):
    pattern = re.compile(r"transfer_id=" + re.escape(transfer_id) + r" offset=(\d+)")
    return [int(match) for path in sorted((agent.data_dir / "logs").glob("agent*"))
            for match in pattern.findall(path.read_text(errors="replace"))]


def interrupt_and_resume(sender, receiver, receiver_transfer_id, size, timeout):
    def in_progress(row):
        if row is None:
            return False
        require(row["status"] != "completed",
                "Transfer completed before it could be interrupted. Increase --resume-mib.")
        require(row["status"] not in TERMINAL_FAILURES, f"Transfer failed before interruption: {row!r}")
        return 0 < row["offset"] < size

    partial = wait_for("a nonzero durable receiver offset", lambda: receiver.transfer(receiver_transfer_id),
                       in_progress, timeout, interval=0.02)
    receiver_id, sender_id = receiver.id, sender.id
    receiver.stop(crash=True)
    sender.stop()
    receiver.start()
    require(receiver.id == receiver_id, "Receiver identity changed after interruption")
    restored = receiver.transfer(receiver_transfer_id)
    require(restored is not None and partial["offset"] <= restored["offset"] < size,
            f"Receiver lost the partial file offset. Before {partial!r}. After {restored!r}")
    prior_streams = len(stream_offsets(sender, receiver_transfer_id))
    sender.start()
    require(sender.id == sender_id, "Sender identity changed after restart")
    discover(sender, receiver)
    discover(receiver, sender)
    current = sender.transfer(receiver_transfer_id)
    require(current is not None, "Sender lost the interrupted transfer")
    require(current["status"] != "failed", f"Interrupted transfer failed: {current!r}")
    offsets = wait_for("resumed sender stream offset",
                       lambda: stream_offsets(sender, receiver_transfer_id)[prior_streams:],
                       bool, timeout)
    require(offsets[0] == restored["offset"],
            f"Sender restarted at {offsets[0]} instead of durable offset {restored['offset']}")
    return restored["offset"]


def run(args, run_dir, log):
    left_http, left_tcp, right_http, right_tcp = ports(4)
    left = Agent("A", args, run_dir, left_http, left_tcp)
    right = Agent("B", args, run_dir, right_http, right_tcp)
    try:
        left.start()
        right.start()
        require(left.id != right.id, "Fresh Agents share a device identity")
        if args.discovery_lease:
            check_discovery_lease(left, right, log)
            return
        restart(left)
        log(f"PASS health and identity persistence on {platform.system()}")

        check_http_guards(left)
        check_http_guards(right)
        log("PASS Host, session bootstrap, Origin, token and expired token guards")

        left_discovery = discover(left, right)
        right_discovery = discover(right, left)
        for agent, row in ((left, left_discovery), (right, right_discovery)):
            require(row["paired"] is False, f"{agent.label} initially trusted a fresh device")
            log(f"PASS {agent.label} discovered {row['id']} through mDNS at "
                f"{', '.join(row['addresses'])} port {row['port']}")

        cancel_file = run_dir / "cancel.bin"
        cancel_digest = make_file(cancel_file, 4096)
        deny_message(left, right, "Must reject before pairing")
        deny_message(right, left, "Must reject before pairing")
        left.upload(right, cancel_file, args.file_timeout, denied=SEND_REJECTION)
        pair(left, right)
        log("PASS unpaired traffic rejection, matching pairing codes and bilateral approval")

        outgoing_a = send_and_ack(left, right, "A to B over LANLink")
        outgoing_b = send_and_ack(right, left, "B to A over LANLink")
        restart(left)
        restart(right)
        require(discover(left, right)["paired"] and discover(right, left)["paired"],
                "Paired keys did not persist across both restarts")
        verify_history(left, right, outgoing_a)
        verify_history(right, left, outgoing_b)
        send_and_ack(left, right, "Post-restart A to B")
        log("PASS bidirectional durable ACK, history and trust after restart")

        right_id = right.id
        right.stop()
        offline_timeout = args.timeout + 15
        if os.name == "nt":
            # TerminateProcess cannot send an mDNS goodbye.
            offline_timeout = max(offline_timeout, MDNS_HOST_TTL + 15)
        wait_for("stopped peer becomes offline", lambda: left.peer(right),
                 lambda peer: peer is not None and not peer["online"], offline_timeout)
        log("PASS stopped peer is shown offline")
        retry_body = "Retry this one message while B is offline"
        queued = left.request("POST", "/messages", payload={"peer_id": right.id, "body": retry_body})
        retry_id = queued["id"]
        require(queued["status"] in {"pending", "failed", "sending"},
                f"Message to a stopped Agent did not enter the queue: {queued!r}")
        wait_for("offline message ready for retry", lambda: find(left.messages(right), retry_id),
                 lambda row: row is not None and row["status"] in {"pending", "failed"}, args.timeout)
        for _ in range(2):
            left.change("POST", f"/messages/{retry_id}/retry")
        right.start()
        require(right.id == right_id, "B device ID changed before queued message delivery")
        wait_for("restarted peer becomes online", lambda: left.peer(right),
                 lambda peer: peer is not None and peer["online"], args.timeout)
        _, received = message_with_ack(left, right, retry_id, retry_body)
        left.request("POST", f"/messages/{retry_id}/retry", denied={400, 409})
        matching = [row for row in right.messages(left) if row["body"] == retry_body]
        require(matching == [received], f"Retry changed or duplicated receiver history: {matching!r}")
        log("PASS offline message retries delivered one receiver history row")

        destination_dir = download_dir(right, run_dir)
        source_file = run_dir / "complete.bin"
        digest = make_file(source_file, args.file_mib * MIB)
        transfer_id, destination = expect_offer(left, right, source_file, digest,
                                                args.file_timeout, destination_dir)
        right.change("POST", f"/transfers/{transfer_id}/accept")
        wait_for_file(left, right, transfer_id, destination, source_file.stat().st_size,
                      digest, args.file_timeout)
        require(right.download(transfer_id, args.file_timeout) == (source_file.stat().st_size, digest),
                "Downloaded attachment differs from the transferred file")
        log(f"PASS accepted {source_file.stat().st_size}-byte transfer, disk and HTTP download SHA-256 {digest}")

        reverse_id, reverse_destination = expect_offer(right, left, source_file, digest,
                                                       args.file_timeout, download_dir(left, run_dir))
        left.change("POST", f"/transfers/{reverse_id}/accept")
        wait_for_file(right, left, reverse_id, reverse_destination, source_file.stat().st_size,
                      digest, args.file_timeout)
        log(f"PASS reverse {source_file.stat().st_size}-byte transfer and SHA-256 {digest}")

        if args.resume_mib:
            resume_file = run_dir / "resume.bin"
            resume_digest = make_file(resume_file, args.resume_mib * MIB)
            resume_size = resume_file.stat().st_size
            resume_id, resume_destination = expect_offer(left, right, resume_file, resume_digest,
                                                         args.file_timeout, destination_dir)
            right.change("POST", f"/transfers/{resume_id}/accept")
            restored_offset = interrupt_and_resume(left, right, resume_id, resume_size, args.file_timeout)
            wait_for_file(left, right, resume_id, resume_destination, resume_size,
                          resume_digest, args.file_timeout)
            log(f"PASS {resume_size}-byte transfer automatically resumed from verified durable byte "
                f"{restored_offset} and published SHA-256 {resume_digest}")
        else:
            log("SKIP interruption and restart recovery (--resume-mib=0)")

        cancel_id, cancel_destination = expect_offer(left, right, cancel_file, cancel_digest,
                                                     args.file_timeout, destination_dir)
        left.change("POST", f"/transfers/{cancel_id}/cancel")
        wait_for("cancellation on both Agents", lambda: (left.transfer(cancel_id), right.transfer(cancel_id)),
                 lambda rows: all(row is not None and row["status"] == "cancelled" for row in rows),
                 args.timeout)
        require(not cancel_destination.exists(), "Cancelled transfer was published")
        log("PASS outgoing cancellation reached receiver without publication")

        rejected_id, rejected_destination = expect_offer(left, right, cancel_file, cancel_digest,
                                                          args.file_timeout, destination_dir)
        right.change("POST", f"/transfers/{rejected_id}/reject")
        wait_for("file rejection on both Agents", lambda: (left.transfer(rejected_id), right.transfer(rejected_id)),
                 lambda rows: all(row is not None and row["status"] == "rejected" for row in rows),
                 args.timeout)
        require(not rejected_destination.exists(), "Rejected transfer was published")
        log("PASS receiver rejection prevents file publication")

        corrupt_id, corrupt_destination = expect_offer(left, right, cancel_file, cancel_digest,
                                                        args.file_timeout, destination_dir)
        snapshot = left.data_dir / "spool" / f"{corrupt_id}.send"
        with snapshot.open("r+b") as damaged:
            first_byte = damaged.read(1)
            damaged.seek(0)
            damaged.write(bytes([first_byte[0] ^ 0xff]))
            damaged.flush()
            os.fsync(damaged.fileno())
        right.change("POST", f"/transfers/{corrupt_id}/accept")
        wait_for("corrupt file rejection", lambda: right.transfer(corrupt_id),
                 lambda row: row is not None and row["status"] == "failed", args.timeout)
        require(not corrupt_destination.exists(), "SHA-256 mismatch was published")
        send_and_ack(left, right, "Healthy message after rejected corrupt file")
        log("PASS changed snapshot fails SHA-256 without publication; subsequent chat succeeds")

        left.change("DELETE", f"/devices/{right.id}/pair")
        wait_for("A revokes trust for B", lambda: left.peer(right),
                 lambda row: row is not None and row["paired"] is False, args.timeout)
        deny_message(left, right, "Must reject after revoke")
        left.upload(right, cancel_file, args.file_timeout, denied=SEND_REJECTION)

        marker = "Revoked device must not reach receiver"
        reverse = right.request("POST", "/messages", payload={"peer_id": left.id, "body": marker},
                                denied=None if right.peer(left)["paired"] else SEND_REJECTION)
        if isinstance(reverse, dict):
            deadline = time.monotonic() + REVOKE_OBSERVATION
            while time.monotonic() < deadline:
                outgoing = find(right.messages(left), reverse["id"])
                require(outgoing is not None and outgoing["status"] != "sent",
                        f"Revoked device received a successful ACK: {outgoing!r}")
                require(all(message["body"] != marker for message in left.messages(right)),
                        "Received a message after revocation")
                time.sleep(0.15)
            log(f"PASS revocation rejects local sends and blocks receipt during {REVOKE_OBSERVATION}s")
        else:
            require(all(message["body"] != marker for message in left.messages(right)),
                    "Received a message after revocation")
            log("PASS revocation rejects new sends on both Agents")
    finally:
        try:
            right.stop()
        finally:
            left.stop()


def parse_args():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--agent", required=True, type=Path, help="Built lanlink-agent executable")
    parser.add_argument("--web-dir", type=Path, help="Built web root passed to both Agents")
    parser.add_argument("--artifacts", type=Path, help="Directory in which to keep each run")
    parser.add_argument("--timeout", type=float, default=25, help="Seconds to wait for discovery or an API state")
    parser.add_argument("--file-timeout", type=float, default=300, help="Seconds to wait for a large transfer")
    parser.add_argument("--file-mib", type=int, default=10, help="Size of the accepted transfer in MiB")
    parser.add_argument("--resume-mib", type=int, default=64,
                        help="Size of the interrupted transfer in MiB. Use 1024 for 1 GiB, or 0 to skip.")
    parser.add_argument("--discovery-lease", action="store_true",
                        help="Run only the mDNS silence, goodbye, and restart checks (POSIX only)")
    args = parser.parse_args()
    args.agent = args.agent.resolve()
    if not args.agent.is_file():
        parser.error(f"Agent binary does not exist: {args.agent}")
    if args.web_dir:
        args.web_dir = args.web_dir.resolve()
        if not args.web_dir.is_dir():
            parser.error(f"Web root does not exist: {args.web_dir}")
    if not 1 <= args.file_mib <= 2048 or not 0 <= args.resume_mib <= 2048:
        parser.error("Transfer sizes must fit in the default 2 GiB file limit")
    if not 0 < args.timeout <= 3600 or not 0 < args.file_timeout <= 3600:
        parser.error("Timeouts must be between zero and 3600 seconds")
    if args.artifacts:
        args.artifacts = args.artifacts.resolve()
        args.artifacts.mkdir(parents=True, exist_ok=True)
    return args


def main():
    args = parse_args()
    if args.discovery_lease and os.name != "posix":
        print("SKIP mDNS silence check requires POSIX process suspension (SIGSTOP and SIGCONT)")
        return 0
    run_dir = Path(tempfile.mkdtemp(prefix="lanlink-smoke-", dir=args.artifacts))
    with (run_dir / "summary.log").open("w", encoding="utf-8") as summary:
        def log(line):
            print(line, flush=True)
            print(line, file=summary, flush=True)

        log(f"Artifacts: {run_dir}")
        log(f"Agent: {args.agent}")
        log(f"Agent SHA-256: {file_digest(args.agent)}")
        log(f"Web root: {args.web_dir}")
        try:
            run(args, run_dir, log)
        except (Exception, KeyboardInterrupt) as exc:
            log(f"FAIL {exc}")
            traceback.print_exc(file=summary)
            return 1
        log("PASS all selected checks against two real Agents on one host")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
