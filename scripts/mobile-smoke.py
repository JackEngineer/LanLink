#!/usr/bin/env python3
"""Verify browser-portal access control and file delivery against a built Agent."""

import argparse
from contextlib import closing
import http.client
import json
from pathlib import Path
import tempfile
from urllib.parse import urlsplit
import uuid

from smoke import Agent, ports, require, wait_for


def request(origin, method, path, *, body=None, headers=None):
    url = urlsplit(origin)
    with closing(http.client.HTTPConnection(url.hostname, url.port, timeout=5)) as connection:
        connection.request(method, path, body=body, headers=headers or {})
        with connection.getresponse() as response:
            headers = {name.lower(): value for name, value in response.getheaders()}
            return response.status, headers, response.read()


def expect(response, *statuses):
    status, headers, body = response
    require(status in statuses, f"Expected HTTP {statuses}, received {status}: {body[:160]!r}")
    return headers, body


def multipart(name, content):
    boundary = uuid.uuid4().hex
    prefix = f'--{boundary}\r\nContent-Disposition: form-data; name="file"; filename="{name}"\r\n\r\n'
    suffix = f'\r\n--{boundary}--\r\n'
    headers = {"Content-Type": f"multipart/form-data; boundary={boundary}"}
    return prefix.encode() + content + suffix.encode(), headers


def check(agent):
    def desktop_state():
        return agent.request("GET", "/mobile", headers={"X-LANLink-Token": agent.token})

    agent.request("GET", "/mobile", denied=(401, 403))
    desktop = desktop_state()
    require(desktop["session"] is None and desktop["addresses"], "No available LAN IPv4 address")
    agent.request("PATCH", "/settings", payload={"max_file_size": 256 * 1024})
    address = desktop["addresses"][0]
    agent.request("POST", "/mobile", payload={"address": "127.0.0.1"}, denied=range(400, 500))
    session = agent.request("POST", "/mobile", payload={"address": address})["session"]
    url = urlsplit(session["url"])
    require(url.hostname == address and url.path == "/mobile/" and url.fragment,
            "QR URL must point to the selected LAN adapter with an invitation fragment")
    origin = f"{url.scheme}://{url.netloc}"
    expect(request(origin, "GET", "/mobile/"), 200)
    expect(request(origin, "GET", "/mobile/main.js"), 200)
    expect(request(origin, "GET", "/mobile/style.css"), 200)
    expect(request(origin, "GET", "/api/v1/settings"), 404)
    expect(request(origin, "GET", "/api/v1/session"), 404)
    expect(request(origin, "GET", "/mobile/state"), 401, 403, 404)

    claim = json.dumps({"invite": url.fragment}).encode()
    claim_headers = {"Content-Type": "application/json", "Origin": origin}
    expect(request(origin, "POST", "/mobile/claim", body=claim,
                   headers={**claim_headers, "Origin": "http://example.invalid"}), 403)
    headers, body = expect(request(origin, "POST", "/mobile/claim", body=claim,
                                   headers=claim_headers), 200)
    pending = json.loads(body)
    code = pending["code"]
    require(pending["phase"] == "pending" and len(code) == 6 and code.isdigit(),
            "The mobile browser did not receive its pending confirmation code")
    require(desktop_state()["session"]["code"] == code,
            "The desktop and mobile confirmation codes differ")
    cookie = headers.get("set-cookie", "")
    require("httponly" in cookie.lower() and "samesite=strict" in cookie.lower(),
            "The browser credential must be a scoped HttpOnly cookie")
    mobile_headers = {"Cookie": cookie.split(";", 1)[0]}
    expect(request(origin, "POST", "/mobile/claim", body=claim,
                   headers=claim_headers), 400, 401, 403, 404, 409)
    incoming = b"mobile to desktop\0" * 2048
    upload, encoding = multipart("from-mobile.bin", incoming)
    upload_headers = {**encoding, **mobile_headers, "Origin": origin}
    expect(request(origin, "POST", "/mobile/files", body=upload,
                   headers=upload_headers), 400, 401, 403, 409)
    wrong_code = "111111" if code == "000000" else "000000"
    agent.request("POST", "/mobile/approve", payload={"code": wrong_code}, denied=range(400, 500))
    approved = agent.request("POST", "/mobile/approve", payload={"code": code})["session"]
    require(approved["phase"] == "approved", "Desktop approval failed")
    _, body = expect(request(origin, "GET", "/mobile/state", headers=mobile_headers), 200)
    require(json.loads(body)["phase"] == "approved", "Approval did not reach the browser")
    print("PASS isolated listener, one-time invitation, and matching desktop approval")

    expect(request(origin, "POST", "/mobile/files", body=upload,
                   headers={**upload_headers, "Origin": "http://example.invalid"}), 403)
    oversized, large_encoding = multipart("large.bin", b"x" * (approved["max_file_size"] + 1))
    expect(request(origin, "POST", "/mobile/files", body=oversized,
                   headers={**upload_headers, **large_encoding}), 400, 413, 422)
    require(desktop_state()["session"]["files"] == [], "Rejected upload became visible")
    inbox = Path(agent.get("/settings")["download_dir"])
    interrupted = http.client.HTTPConnection(url.hostname, url.port, timeout=5)
    try:
        interrupted.putrequest("POST", "/mobile/files")
        for name, value in upload_headers.items():
            interrupted.putheader(name, value)
        interrupted.putheader("Content-Length", str(len(upload) + 1024))
        interrupted.endheaders()
        interrupted.send(upload[:-128])
        wait_for("an incomplete mobile file", lambda: list(inbox.glob("*.part")), bool,
                 agent.timeout)
    finally:
        interrupted.close()
    wait_for("interrupted upload cleanup", lambda: not list(inbox.glob("*.part")), bool,
             agent.timeout)
    require(desktop_state()["session"]["files"] == [], "Interrupted upload became visible")
    _, body = expect(request(origin, "POST", "/mobile/files", body=upload,
                             headers=upload_headers), 200, 201)
    received = json.loads(body)
    require(received["direction"] == "to_computer", "Wrong direction for mobile upload")
    published = list(inbox.iterdir())
    require(len(published) == 1 and published[0].read_bytes() == incoming,
            "The complete mobile file was not published to the desktop download directory")
    path = f"/api/v1/mobile/files/{received['id']}"
    headers, content = expect(request(agent.origin, "GET", path,
                                      headers={"Sec-Fetch-Site": "same-origin"}), 200)
    require(content == incoming and "attachment" in headers.get("content-disposition", ""),
            "The desktop attachment differs from the mobile upload")

    outgoing = b"desktop to mobile\0" * 4096
    upload, encoding = multipart("from-desktop.bin", outgoing)
    sent = agent.request("POST", "/mobile/files", body=upload, headers=encoding)
    require(sent["direction"] == "to_mobile", "Wrong direction for desktop upload")
    path = f"/mobile/files/{sent['id']}"
    expect(request(origin, "GET", path), 401, 403, 404)
    expect(request(origin, "GET", f"/mobile/files/{received['id']}",
                   headers=mobile_headers), 400, 401, 403, 404)
    headers, content = expect(request(origin, "GET", path, headers=mobile_headers), 200)
    require(content == outgoing and "attachment" in headers.get("content-disposition", ""),
            "The mobile attachment differs from the desktop upload")
    print("PASS upload limits, interruption cleanup, scoped downloads, and byte-for-byte delivery")

    require(agent.request("DELETE", "/mobile")["session"] is None, "Session was not revoked")

    def listener_closed():
        try:
            request(origin, "GET", path, headers=mobile_headers)
        except (ConnectionRefusedError, ConnectionResetError, http.client.RemoteDisconnected):
            return True
        return False

    wait_for("revoked listener to close", listener_closed, bool, agent.timeout)
    fresh = agent.request("POST", "/mobile", payload={"address": address})["session"]
    next_url = urlsplit(fresh["url"])
    next_origin = f"{next_url.scheme}://{next_url.netloc}"
    require(next_url.fragment != url.fragment, "The replacement invitation reused the old secret")
    expect(request(next_origin, "POST", "/mobile/claim", body=claim,
                   headers={**claim_headers, "Origin": next_origin}), 400, 401, 403, 404, 409)
    expect(request(next_origin, "GET", "/mobile/state", headers=mobile_headers), 401, 403, 404)
    require(published[0].read_bytes() == incoming, "Revocation removed a received file")
    print("PASS revocation closes the listener, invalidates credentials, and preserves received files")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--agent", required=True, type=Path)
    parser.add_argument("--web-dir", required=True, type=Path)
    parser.add_argument("--timeout", default=10, type=float)
    args = parser.parse_args()
    args.agent = args.agent.resolve()
    args.web_dir = args.web_dir.resolve()
    with tempfile.TemporaryDirectory(prefix="lanlink-mobile-smoke-") as data:
        agent = Agent("desktop", args, Path(data), *ports(2))
        agent.argv.append("--no-mdns")
        try:
            agent.start()
            check(agent)
        finally:
            agent.stop()


if __name__ == "__main__":
    main()
