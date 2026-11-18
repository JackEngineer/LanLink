#!/usr/bin/env python3
"""Verify remembered mobile browsers across temporary listeners and Agent restarts."""

import argparse
import copy
from http.cookiejar import CookieJar
import json
from pathlib import Path
import tempfile
import time
from urllib.error import HTTPError, URLError
from urllib.parse import urlsplit
from urllib.request import build_opener, HTTPCookieProcessor, ProxyHandler, Request

from smoke import Agent, ports, require, wait_for


class Browser:
    def __init__(self, cookie_jar=None):
        self.cookies = cookie_jar if cookie_jar is not None else CookieJar()
        self.opener = build_opener(ProxyHandler({}), HTTPCookieProcessor(self.cookies))
        self.origin = ""

    def request(self, method, path, payload=None, expected=(200,)):
        headers = {"User-Agent": "Mozilla/5.0 (iPhone) AppleWebKit/605.1.15 Version/18.0 Mobile Safari/604.1"}
        body = None
        if method != "GET":
            headers["Origin"] = self.origin
        if payload is not None:
            headers["Content-Type"] = "application/json"
            body = json.dumps(payload).encode()
        request = Request(self.origin + path, data=body, method=method, headers=headers)
        try:
            response = self.opener.open(request, timeout=5)
        except HTTPError as error:
            response = error
        with response:
            raw = response.read()
            require(response.status in expected,
                    f"{method} {path} returned HTTP {response.status}: {raw[:150]!r}")
            return json.loads(raw)

    def scan(self, session, expected=(200,), invite=None):
        url = urlsplit(session["url"])
        self.origin = f"{url.scheme}://{url.netloc}"
        return self.request("POST", "/mobile/claim",
                            {"invite": url.fragment if invite is None else invite}, expected)


def desktop_state(agent):
    return agent.request("GET", "/mobile", headers={"X-LANLink-Token": agent.token})


def open_session(agent, address):
    agent.request("DELETE", "/mobile")
    return agent.request("POST", "/mobile", payload={"address": address})["session"]


def check(agent):
    address = desktop_state(agent)["addresses"][0]
    known = Browser()
    first = open_session(agent, address)
    pending = known.scan(first)
    require(pending["phase"] == "pending", "A new browser bypassed desktop confirmation")
    state = agent.request("POST", "/mobile/approve",
                          payload={"code": pending["code"], "remember": True})
    remembered_id = state["session"]["remembered_browser_id"]
    require(state["session"]["remembered"] and remembered_id,
            "Desktop approval did not remember the browser")
    require(len(state["remembered_browsers"]) == 1, "Unexpected remembered browser count")
    approved = known.request("GET", "/mobile/state")
    require(approved["remembered"], "Mobile state did not report remembered status")
    persistent = [cookie for cookie in known.cookies
                  if cookie.expires and cookie.expires > time.time() + 86400]
    require(len(persistent) == 1 and persistent[0].has_nonstandard_attr("HttpOnly"),
            "The approved browser did not receive its persistent HttpOnly credential")
    require(persistent[0].get_nonstandard_attr("SameSite") == "Strict",
            "The remembered credential must use SameSite=Strict")
    print("PASS first approval persists one browser and issues a persistent HttpOnly credential")

    for attempt in range(3):
        next_session = open_session(agent, address)
        if urlsplit(next_session["url"]).port != urlsplit(first["url"]).port:
            break
    else:
        raise RuntimeError("The OS repeatedly reused the previous listener port")
    known.origin = next_session["url"].split("/mobile/", 1)[0]
    known.request("GET", "/mobile/state", expected=(403,))
    known.scan(next_session, expected=(403,), invite="invalid-invitation")
    recalled = known.scan(next_session)
    require(recalled["phase"] == "approved"
            and recalled["remembered_browser_id"] == remembered_id,
            "An approved browser did not reconnect across a new listener port")
    require(known.request("GET", "/mobile/state")["phase"] == "approved",
            "Recalled browser did not receive a fresh session credential")
    print("PASS remembered browser requires a valid QR and reconnects across listener ports")

    agent.request("DELETE", "/mobile")
    agent.stop()
    agent.start()
    require(len(desktop_state(agent)["remembered_browsers"]) == 1,
            "The remembered browser was lost after restarting the Agent")
    restarted = open_session(agent, address)
    require(known.scan(restarted)["phase"] == "approved",
            "Remembered credentials did not survive the Agent restart")
    print("PASS remembered authorization survives a real Agent restart")

    guest = Browser()
    guest_session = open_session(agent, address)
    guest_pending = guest.scan(guest_session)
    require(guest_pending["phase"] == "pending", "A guest browser was automatically approved")
    guest_approval = agent.request("POST", "/mobile/approve",
                                   payload={"code": guest_pending["code"], "remember": False})
    require(not guest_approval["session"]["remembered"], "Guest opt-out was ignored")
    guest.request("GET", "/mobile/state")
    require(not any(cookie.expires and cookie.expires > time.time() + 86400
                    for cookie in guest.cookies), "A guest received a persistent credential")
    require(guest.scan(open_session(agent, address))["phase"] == "pending",
            "A guest skipped approval on its next scan")

    tampered_jar = CookieJar()
    tampered = copy.copy(persistent[0])
    tampered.value = "0" * 64
    tampered_jar.set_cookie(tampered)
    unknown = Browser(tampered_jar)
    require(unknown.scan(open_session(agent, address))["phase"] == "pending",
            "An invalid remembered credential bypassed confirmation")
    print("PASS missing credentials, invalid credentials, and guest opt-out require confirmation")

    live = open_session(agent, address)
    require(known.scan(live)["phase"] == "approved", "Could not restore the trusted browser")
    old_origin = known.origin
    forgotten = agent.request("DELETE", f"/mobile/browsers/{remembered_id}")
    require(forgotten["session"] is None and forgotten["remembered_browsers"] == [],
            "Forgetting a browser did not revoke both stored and active authorization")

    def old_connection_ended():
        try:
            known.request("GET", "/mobile/state", expected=(403, 410))
            return True
        except (URLError, ConnectionError):
            return True

    wait_for("forgotten browser listener to stop", old_connection_ended, bool, agent.timeout)
    require(known.origin == old_origin, "The old browser origin changed during revocation")
    revoked = known.scan(open_session(agent, address))
    require(revoked["phase"] == "pending", "A forgotten browser bypassed confirmation")
    agent.request("DELETE", "/mobile")
    agent.stop()
    agent.start()
    require(desktop_state(agent)["remembered_browsers"] == [],
            "A revoked browser returned after restarting the Agent")
    print("PASS forgetting a browser disconnects it and keeps it revoked after restarting")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--agent", required=True, type=Path)
    parser.add_argument("--web-dir", required=True, type=Path)
    parser.add_argument("--timeout", default=10, type=float)
    args = parser.parse_args()
    args.agent = args.agent.resolve()
    args.web_dir = args.web_dir.resolve()
    with tempfile.TemporaryDirectory(prefix="lanlink-mobile-remember-") as directory:
        agent = Agent("desktop", args, Path(directory), *ports(2))
        agent.argv.append("--no-mdns")
        try:
            agent.start()
            check(agent)
        finally:
            agent.stop()


if __name__ == "__main__":
    main()
