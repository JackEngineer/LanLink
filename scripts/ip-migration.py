#!/usr/bin/env python3
"""Check automatic peer IP migration with two real LANLink Agents on one Mac."""

import argparse
from contextlib import ExitStack, closing, contextmanager
import ipaddress
import json
from pathlib import Path
import select
import shutil
import signal
import socket
from socketserver import BaseRequestHandler, ThreadingTCPServer
import sqlite3
import subprocess
import sys
import tempfile
from threading import Event, Lock, Thread
import traceback
import uuid

from smoke import (
    Agent, CheckFailed, file_digest, find, message_with_ack, pair, ports, require, send_and_ack,
    wait_for,
)

LOOPBACK = "127.0.0.1"
SERVICE_TYPE = "_lanlink._tcp"


class ForwardConnection(BaseRequestHandler):
    def handle(self):
        client = self.request
        client.settimeout(2)
        try:
            with socket.create_connection(self.server.destination, timeout=2) as agent:
                self.server.record_connection()
                targets = {client: agent, agent: client}
                while not self.server.stopping.is_set():
                    readable, _, _ = select.select(targets, [], [], 0.2)
                    for source in readable:
                        chunk = source.recv(64 * 1024)
                        if not chunk:
                            return
                        targets[source].sendall(chunk)
        except OSError:
            return


class Proxy(ThreadingTCPServer):
    def __init__(self, endpoint, destination):
        super().__init__(endpoint, ForwardConnection)
        self.destination = destination
        self.stopping = Event()
        self.lock = Lock()
        self.connection_count = 0

    def record_connection(self):
        with self.lock:
            self.connection_count += 1

    def connections(self):
        with self.lock:
            return self.connection_count


@contextmanager
def forward(endpoint, destination):
    with Proxy(endpoint, destination) as proxy:
        worker = Thread(target=proxy.serve_forever, kwargs={"poll_interval": 0.2})
        worker.start()
        try:
            yield proxy
        finally:
            proxy.stopping.set()
            proxy.shutdown()
            worker.join()


@contextmanager
def advertise(instance, endpoint, agent, log_path):
    address, port = endpoint
    command = [
        "dns-sd", "-P", instance, SERVICE_TYPE, "local.", str(port),
        f"{instance}.local.", address,
        f"id={agent.id}", f"name={agent.name}", f"os={agent.device['platform']}",
        "version=0.2.0",
    ]
    with log_path.open("w", encoding="utf-8") as log:
        print(command, file=log, flush=True)
        process = subprocess.Popen(command, stdin=subprocess.DEVNULL, stdout=log,
                                   stderr=subprocess.STDOUT, start_new_session=True)
        try:
            yield
            require(process.poll() is None, f"dns-sd exited early. See {log_path}")
        finally:
            if process.poll() is None:
                process.send_signal(signal.SIGINT)
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)


def discover(observer, agent, endpoint):
    address, port = endpoint
    row = wait_for(f"{agent.label} mDNS discovery at {address}:{port}",
                   lambda: observer.peer(agent),
                   lambda row: row is not None and row["online"]
                   and row["addresses"] == [address] and row["port"] == port,
                   observer.timeout)
    require(row["name"] == agent.name and row["platform"] == agent.device["platform"],
            f"mDNS metadata differs from {agent.label} identity: {row!r}")
    return row


def offline(row):
    return (row is not None and row["paired"] and not row["online"]
            and not row["addresses"] and row["port"] == 0)


def require_refused(endpoint):
    try:
        with socket.create_connection(endpoint, timeout=2):
            raise CheckFailed(f"Old TCP endpoint {endpoint} still accepts connections")
    except ConnectionRefusedError:
        return


def verify_persisted(agent, sender, message):
    database = (agent.data_dir / "lanlink.sqlite3").as_uri()
    with closing(sqlite3.connect(database + "?mode=ro", uri=True)) as connection:
        values = connection.execute("SELECT value FROM messages WHERE peer = ?", (sender.id,))
        rows = [json.loads(value) for (value,) in values]
    matches = [row for row in rows if row["id"] == message["id"] or row["body"] == message["body"]]
    require(matches == [message], f"Expected one matching persisted message in B: {matches!r}")


def run(args, run_dir, log):
    left_http, left_tcp, right_http, right_tcp, advertised_port = ports(5)
    left = Agent("A", args, run_dir, left_http, left_tcp)
    right = Agent("B", args, run_dir, right_http, right_tcp)
    right.argv.append("--no-mdns")
    old_endpoint = (args.old_ip, advertised_port)
    new_endpoint = (LOOPBACK, advertised_port)
    destination = (LOOPBACK, right_tcp)

    with ExitStack() as agents:
        for agent in (left, right):
            agents.callback(agent.stop)
            agent.start()
        pids = (left.process.pid, right.process.pid)
        log(f"Started isolated Agents with PIDs {pids}. A={left.id}, B={right.id}")
        require(left.peer(right) is None and right.peer(left) is None,
                "Agents know each other before the test advertisement")

        instance = f"lanlink-qa-{right.id[:16]}"
        with ExitStack() as announcement:
            with forward(old_endpoint, destination) as proxy:
                announcement.enter_context(advertise(instance, old_endpoint, right, run_dir / "mdns-old.log"))
                require(not discover(left, right, old_endpoint)["paired"], "B is already paired")
                pair(left, right, right_discovers_left=False)
                original, _ = send_and_ack(left, right, f"Before IP migration {uuid.uuid4()}")
                require(proxy.connections() >= 1, "The old endpoint forwarded no connections")
                log(f"PASS bilateral pairing and ACK {original['id']} via {old_endpoint}, "
                    f"forwarded connections={proxy.connections()}")
            require_refused(old_endpoint)
            wait_for("B to lose its old authenticated connection", lambda: right.peer(left),
                     offline, args.timeout)
            log(f"PASS {old_endpoint} refused TCP and B reported the authenticated peer offline")

        wait_for("A to remove B's old mDNS endpoint", lambda: left.peer(right), offline, args.timeout)
        log("PASS A removed the old mDNS endpoint and retained trust in B")
        marker = f"Pending during IP migration {uuid.uuid4()}"
        queued = left.request("POST", "/messages", payload={"peer_id": right.id, "body": marker})
        require(queued["status"] == "pending" and find(left.messages(right), queued["id"]) == queued,
                f"Message did not remain pending while B was offline: {queued!r}")
        require(find(right.messages(left), queued["id"]) is None, "B received the message while offline")
        log(f"PASS A stored offline message {queued['id']} as pending")

        with forward(new_endpoint, destination) as proxy:
            require(offline(left.peer(right)) and proxy.connections() == 0,
                    "A knows the new endpoint before its announcement")
            with advertise(instance, new_endpoint, right, run_dir / "mdns-new.log"):
                require(discover(left, right, new_endpoint)["paired"], "A lost trust after migration")
                require_refused(old_endpoint)
                log(f"PASS A discovered the same B at {new_endpoint} and the old endpoint still refused TCP")
                _, incoming = message_with_ack(left, right, queued["id"], marker)
                require(proxy.connections() >= 1, "The new endpoint forwarded no connections")
                require((left.process.pid, right.process.pid) == pids
                        and left.get("/device") == left.device and right.get("/device") == right.device
                        and right.peer(left)["paired"] and not left.get("/pairings")
                        and not right.get("/pairings"), "An Agent or pairing changed during migration")
                log(f"PASS original PIDs {pids} and pairing retained. ACK {queued['id']} "
                    f"via {new_endpoint}, forwarded connections={proxy.connections()}")

    verify_persisted(right, left, incoming)
    log(f"PASS B stored exactly one copy of {incoming['id']} in SQLite after shutdown")


def parse_args():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--agent", required=True, type=Path, help="Path to lanlink-agent")
    parser.add_argument("--old-ip", required=True, type=ipaddress.IPv4Address,
                        help="Existing private IPv4 address of this Mac's LAN interface")
    parser.add_argument("--artifacts", type=Path, help="Parent directory for the run logs and databases")
    parser.add_argument("--timeout", type=float, default=145, help="Seconds to wait for each checkpoint")
    args = parser.parse_args()
    if sys.platform != "darwin" or not shutil.which("dns-sd"):
        parser.error("This check requires macOS and dns-sd")
    args.agent = args.agent.resolve()
    if not args.agent.is_file():
        parser.error(f"Agent binary does not exist: {args.agent}")
    if not args.old_ip.is_private or args.old_ip.is_loopback or args.old_ip.is_unspecified:
        parser.error("--old-ip must be a private LAN IPv4 address on this Mac")
    if not 0 < args.timeout <= 3600:
        parser.error("--timeout must be between zero and 3600 seconds")
    if args.artifacts:
        args.artifacts = args.artifacts.resolve()
        args.artifacts.mkdir(parents=True, exist_ok=True)
    args.old_ip = str(args.old_ip)
    args.web_dir = None
    return args


def interrupted(_signum, _frame):
    raise KeyboardInterrupt("test interrupted")


def main():
    args = parse_args()
    signal.signal(signal.SIGTERM, interrupted)
    run_dir = Path(tempfile.mkdtemp(prefix="lanlink-ip-migration-", dir=args.artifacts))
    with (run_dir / "summary.log").open("w", encoding="utf-8") as summary:
        def log(line):
            print(line, flush=True)
            print(line, file=summary, flush=True)

        log(f"Artifacts: {run_dir}")
        log(f"Agent: {args.agent}")
        log(f"Agent SHA-256: {file_digest(args.agent)}")
        log("Scope: one Mac, real mDNS, TCP, Noise sessions, and SQLite; interface settings untouched")
        try:
            run(args, run_dir, log)
        except (Exception, KeyboardInterrupt) as exc:
            log(f"FAIL {exc}")
            traceback.print_exc(file=summary)
            return 1
        log("PASS automatic reconnection and delivery after peer IP migration")
        return 0


if __name__ == "__main__":
    raise SystemExit(main())
