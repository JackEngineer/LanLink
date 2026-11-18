#!/usr/bin/python3
"""Install the LANLink Certbot lineage into its existing Nginx TLS directory."""

import ctypes
import fcntl
import hashlib
import os
from pathlib import Path
import shlex
import shutil
import signal
import socket
import ssl
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass


@dataclass(frozen=True)
class RenewalTarget:
    domain: str = "lanlink.heyyami.com"
    lineage: Path = Path("/etc/letsencrypt/live/lanlink.heyyami.com")
    nginx: Path = Path("/usr/sbin/nginx")
    config: Path = Path("/etc/qingzhou-public-website/nginx.conf")
    pid: Path = Path("/run/qingzhou-public-website/nginx.pid")
    tls: Path = Path("/etc/lanlink-website/tls")


def run(*args):
    return subprocess.run(args, check=True, stdout=subprocess.PIPE,
                          stderr=subprocess.PIPE, timeout=30).stdout


def fingerprint(cert):
    return hashlib.sha256(run("/usr/bin/openssl", "x509", "-in", str(cert),
                              "-outform", "DER")).digest()


def master_identity(target):
    pid = int(target.pid.read_text().strip())
    if pid <= 1:
        raise RuntimeError("Nginx pid must be greater than 1")
    proc = Path(f"/proc/{pid}")
    command = (proc / "cmdline").read_bytes().decode().replace("\0", " ").strip()
    command = command.removeprefix("nginx: master process ")
    expected = [str(target.nginx), "-c", str(target.config)]
    permitted = [expected, expected + ["-g", "daemon", "off;"],
                 expected + ["-g", "daemon off;"]]
    if ((proc / "exe").resolve() != target.nginx.resolve()
            or shlex.split(command) not in permitted):
        raise RuntimeError("Pid file does not identify the expected public Nginx master")
    start_time = (proc / "stat").read_text().rsplit(")", 1)[1].split()[19]
    return pid, start_time


def reload_and_verify(target, identity, expected):
    if master_identity(target) != identity:
        raise RuntimeError("Public Nginx master changed during certificate deployment")
    run("/usr/bin/nsenter", "-t", str(identity[0]), "-m", "--",
        str(target.nginx), "-c", str(target.config), "-t")
    if master_identity(target) != identity:
        raise RuntimeError("Public Nginx master changed during configuration check")
    os.kill(identity[0], signal.SIGHUP)
    context = ssl.create_default_context()
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        try:
            with socket.create_connection(("127.0.0.1", 443), timeout=2) as connection:
                with context.wrap_socket(connection, server_hostname=target.domain) as tls:
                    if hashlib.sha256(tls.getpeercert(binary_form=True)).digest() == expected:
                        return
        except (OSError, ssl.SSLError):
            pass
        time.sleep(0.25)
    raise RuntimeError("Loopback HTTPS did not serve the expected LANLink certificate")


def exchange(left, right):
    # Ubuntu glibc and Linux support atomic directory exchange, including real dirs.
    libc = ctypes.CDLL("libc.so.6", use_errno=True)
    rename = libc.renameat2
    rename.argtypes = [ctypes.c_int, ctypes.c_char_p, ctypes.c_int, ctypes.c_char_p,
                       ctypes.c_uint]
    rename.restype = ctypes.c_int
    if rename(-100, os.fsencode(left), -100, os.fsencode(right), 2) != 0:
        error = ctypes.get_errno()
        raise OSError(error, os.strerror(error))


def deploy(target):
    if target.tls.is_symlink() or not target.tls.is_dir():
        raise RuntimeError("LANLink TLS path must be an existing real directory")
    for filename in ("fullchain.pem", "privkey.pem"):
        path = target.tls / filename
        if path.is_symlink() or not path.is_file():
            raise RuntimeError(f"Existing TLS file must be a regular file: {path}")
    identity = master_identity(target)
    staged = Path(tempfile.mkdtemp(prefix=".tls-backup-", dir=target.tls.parent))
    exchanged = False
    try:
        for filename, mode in (("fullchain.pem", 0o644), ("privkey.pem", 0o600)):
            with open(staged / filename, "wb") as destination:
                os.fchmod(destination.fileno(), mode)
                destination.write((target.lineage / filename).read_bytes())
                destination.flush()
                os.fsync(destination.fileno())
        cert, key = staged / "fullchain.pem", staged / "privkey.pem"
        run("/usr/bin/openssl", "x509", "-in", str(cert), "-noout", "-checkhost", target.domain)
        run("/usr/bin/openssl", "x509", "-in", str(cert), "-noout", "-checkend", "604800")
        public_cert = run("/usr/bin/openssl", "x509", "-in", str(cert), "-pubkey", "-noout")
        public_key = run("/usr/bin/openssl", "pkey", "-in", str(key), "-pubout")
        if public_cert != public_key:
            raise RuntimeError("Renewed certificate and private key do not match")
        expected = fingerprint(cert)
        previous = fingerprint(target.tls / "fullchain.pem")
        if (cert.read_bytes() != (target.tls / "fullchain.pem").read_bytes()
                or key.read_bytes() != (target.tls / "privkey.pem").read_bytes()):
            exchange(staged, target.tls)
            exchanged = True
        reload_and_verify(target, identity, expected)
    except BaseException:
        if exchanged:
            exchange(staged, target.tls)
            exchanged = False
            try:
                reload_and_verify(target, identity, previous)
            except Exception as rollback_error:
                print(f"LANLink rollback reload failed: {rollback_error}", file=sys.stderr)
        raise
    finally:
        if not exchanged:
            shutil.rmtree(staged)
    print(f"LANLink certificate deployed and verified for {target.domain}")
    if exchanged:
        print(f"Previous TLS directory retained at {staged}")


def main():
    target = RenewalTarget()
    lineage = os.environ.get("RENEWED_LINEAGE", "")
    domains = set(os.environ.get("RENEWED_DOMAINS", "").split())
    if Path(lineage) != target.lineage or domains != {target.domain}:
        return
    if os.geteuid() != 0:
        raise RuntimeError("LANLink certificate deployment requires root")
    os.umask(0o077)
    with open("/run/lock/lanlink-certbot-deploy.lock", "w") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        deploy(target)


if __name__ == "__main__":
    try:
        main()
    except subprocess.CalledProcessError as error:
        print(f"LANLink certificate deployment failed: {error.stderr.decode().strip()}", file=sys.stderr)
        sys.exit(1)
    except Exception as error:
        print(f"LANLink certificate deployment failed: {error}", file=sys.stderr)
        sys.exit(1)
