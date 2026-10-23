#!/usr/bin/env python3
"""Real-browser check of the pinned FaceReview admin console's token reissue (WBS 7.2.9 / 7.2.10).

The original console keeps the access token in memory/localStorage and, when an API call answers 401, calls POST /v2/auth/reissue once
(the refresh token is an HttpOnly cookie) and repeats the call. This journey lets the access token of a real SUPER_ADMIN login expire on
the server (the service under test is started with a short access TTL), then uses the console only through its own navigation links.
It creates no operation the original UI does not have. Token values never leave the page: only a SHA-256 comparison result is kept.
"""
from __future__ import annotations

import argparse
import json
import os
import re
import signal
import socket
import subprocess
import sys
import time
import traceback
import urllib.request
from pathlib import Path
from urllib.parse import urlsplit

from playwright.sync_api import expect, sync_playwright

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
from product.tests.facereview_ui_browser_evidence import (  # noqa: E402
    admin_request_evidence,
    body_failure_reason,
    safe_browser_failure_type,
    safe_page_error_name,
    safe_wire_code,
)

UI_REVISION = "ab0386c878a5ba5c7c079a262b987c10d211d22d"
# The service refuses a call with HTTP 200 and one of these wire codes (not HTTP 401); the console must answer that with a reissue.
REFUSED_TOKEN_CODES = {"WIRE.TOKEN_EXPIRED", "WIRE.UNAUTHENTICATED"}
AIP_PATHS = {"/read", "/apply", "/extension", "/session", "/jobs/enqueue", "/jobs/status", "/jobs/result",
             "/auth/login", "/auth/reissue", "/auth/logout"}


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--ui", type=Path, required=True)
    parser.add_argument("--api", required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    admin_email = os.environ["AIP_FACEREVIEW_ADMIN_EMAIL"]
    admin_password = os.environ["AIP_FACEREVIEW_ADMIN_PASSWORD"]
    ttl_secs = int(os.environ["AIP_FACEREVIEW_ACCESS_TTL_SECS"])
    ui_port = os.environ.get("AIP_FACEREVIEW_UI_PORT", "5173")
    output = args.out.resolve()
    output.mkdir(parents=True, exist_ok=True)
    api_port = urlsplit(args.api).port

    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as probe:
        probe.settimeout(1)
        if probe.connect_ex(("127.0.0.1", int(ui_port))) == 0:
            raise SystemExit(f"UI port {ui_port} is already in use; pick a free AIP_FACEREVIEW_UI_PORT")

    server_env = os.environ.copy()
    server_env["VITE_AIP_URL"] = args.api
    for name in ("AIP_FACEREVIEW_ADMIN_PASSWORD", "AIP_FACEREVIEW_ADMIN_EMAIL"):
        server_env.pop(name, None)
    server = subprocess.Popen(
        ["node", "node_modules/vite/bin/vite.js", "--host", "127.0.0.1", "--port", ui_port, "--strictPort"],
        cwd=args.ui, env=server_env, start_new_session=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )
    requests: list[dict[str, object]] = []
    pending_bodies: dict[object, tuple[object, dict[str, object]]] = {}
    page_errors: list[dict[str, str]] = []
    state: dict[str, object] = {"stage": "startup"}
    checks: dict[str, object] = {}
    browser = None

    def stage(name: str) -> None:
        state["stage"] = name

    try:
        ready = False
        for _ in range(60):
            if server.poll() is not None:
                raise RuntimeError("Vite exited before becoming ready")
            try:
                with urllib.request.urlopen(f"http://127.0.0.1:{ui_port}/", timeout=1) as response:
                    if response.status == 200:
                        ready = True
                        break
            except Exception:
                time.sleep(1)
        if not ready:
            raise RuntimeError("Vite did not become ready")

        with sync_playwright() as playwright:
            browser = playwright.chromium.launch(headless=True)
            context = browser.new_context(viewport={"width": 1440, "height": 1000})
            page = context.new_page()
            page.set_default_timeout(20000)
            page.on("pageerror", lambda error: page_errors.append({"stage": str(state["stage"]), "type": safe_page_error_name(error)}))
            base = f"http://localhost:{ui_port}"

            def capture_response(response) -> None:
                parsed = urlsplit(response.url)
                if parsed.port != api_port:
                    return
                row: dict[str, object] = {"stage": state["stage"], "method": response.request.method, "path": parsed.path, "status": response.status}
                try:
                    request_body = response.request.post_data_json
                except Exception:
                    request_body = None
                row.update(admin_request_evidence(parsed.path, request_body))
                requests.append(row)
                pending_bodies[response.request] = (response, row)

            def capture_body(request) -> None:
                entry = pending_bodies.pop(request, None)
                if entry is None:
                    return
                response, row = entry
                try:
                    envelope = json.loads(response.body().decode("utf-8"))
                    row["body_captured"] = True
                except Exception as error:
                    envelope = {}
                    row["body_captured"] = False
                    row["body_failure_reason"] = "json_decode_error" if isinstance(error, ValueError) else body_failure_reason(error)
                if isinstance(envelope, dict):
                    if isinstance(envelope.get("ok"), bool):
                        row["ok"] = envelope["ok"]
                    code = safe_wire_code(envelope.get("code"))
                    if code is not None:
                        row["code"] = code

            def settled_idle() -> None:
                page.wait_for_load_state("networkidle", timeout=20000)
                deadline = time.monotonic() + 20
                while pending_bodies and time.monotonic() < deadline:
                    page.wait_for_timeout(50)
                if pending_bodies:
                    raise AssertionError("API response bodies did not finish before the next step")

            def field(scope, label: str):
                return scope.locator(".text-input", has_text=label).locator("input")

            def token_digest() -> str | None:
                # The digest is computed inside the page; the token itself is never returned to this script.
                return page.evaluate("""async () => {
                  const raw = localStorage.getItem('admin-auth-storage');
                  const token = raw ? JSON.parse(raw)?.state?.access_token : null;
                  if (!token) return null;
                  const bytes = new TextEncoder().encode(token);
                  const digest = await crypto.subtle.digest('SHA-256', bytes);
                  return Array.from(new Uint8Array(digest)).map((b) => b.toString(16).padStart(2, '0')).join('');
                }""")

            def signed_in() -> bool:
                return bool(page.evaluate("JSON.parse(localStorage.getItem('admin-auth-storage') || '{}')?.state?.is_sign_in === true"))

            page.on("response", capture_response)
            page.on("requestfinished", capture_body)

            try:
                stage("warm_up")
                page.goto(f"{base}/login", wait_until="domcontentloaded")
                page.wait_for_load_state("networkidle", timeout=60000)
                page.reload(wait_until="domcontentloaded")
                page.wait_for_load_state("networkidle", timeout=60000)

                stage("admin_login")
                field(page, "이메일").fill(admin_email)
                field(page, "비밀번호").fill(admin_password)
                page.get_by_role("button", name="로그인", exact=True).click()
                expect(page).to_have_url(re.compile(r"/$"), timeout=30000)
                login_at = time.monotonic()
                settled_idle()
                first_digest = token_digest()
                if first_digest is None or not signed_in():
                    raise RuntimeError("the administrator session was not stored")
                if any(row["path"] == "/auth/reissue" for row in requests):
                    raise RuntimeError("the access token was reissued before it could have expired; raise the access TTL of this journey")
                if any(row["status"] == 401 or row.get("code") in REFUSED_TOKEN_CODES for row in requests):
                    raise RuntimeError("a request was refused before the access token could have expired; raise the access TTL of this journey")

                links = [("expiry_users", "회원 관리", "회원 관리"), ("expiry_videos", "영상 관리", "영상 관리")]
                digests = [first_digest]
                for round_no, (stage_name, link, heading) in enumerate(links, start=1):
                    stage(stage_name)
                    # The server's JWT check has no leeway, so one second past expiry is enough; the token is the one of this login or of the
                    # last reissue, which was issued at most a moment before this wait began.
                    remaining = ttl_secs + 1.5 - (time.monotonic() - login_at)
                    if remaining > 0:
                        page.wait_for_timeout(int(remaining * 1000))
                    mark = len(requests)
                    page.get_by_role("link", name=link).click()
                    expect(page.get_by_role("heading", name=heading)).to_be_visible()
                    if heading == "회원 관리":
                        expect(page.locator("tbody tr").first).to_be_visible()
                    # Playwright's sync API delivers network events only while it is being called, so wait for the sequence itself (a refused
                    # request, the reissue, a request answered after it) instead of assuming the events have already been processed.
                    deadline = time.monotonic() + 30
                    while time.monotonic() < deadline:
                        window = requests[mark:]
                        reissue_at = next((index for index, row in enumerate(window) if row["path"] == "/auth/reissue"), None)
                        if reissue_at is not None and any(row["path"] in {"/read", "/extension"} and row["status"] == 200 for row in window[reissue_at + 1:]):
                            break
                        page.wait_for_timeout(100)
                    settled_idle()
                    page.wait_for_timeout(500)
                    settled_idle()
                    login_at = time.monotonic()
                    window = requests[mark:]
                    refused = [index for index, row in enumerate(window) if row["status"] == 401 or row.get("code") in REFUSED_TOKEN_CODES]
                    reissued = [index for index, row in enumerate(window) if row["path"] == "/auth/reissue"]
                    if not refused:
                        raise RuntimeError("no request met the expired access token, so no reissue was needed")
                    if len(reissued) != 1:
                        raise RuntimeError("the expired token was not reissued exactly once (a concurrent refresh must share one reissue)")
                    reissue_row = window[reissued[0]]
                    if reissue_row["status"] != 200 or reissue_row.get("ok") is not True:
                        raise RuntimeError("the reissue was not accepted")
                    if min(refused) > reissued[0]:
                        raise RuntimeError("the reissue happened before any request was refused")
                    if any(row["path"] == "/auth/login" for row in window):
                        raise RuntimeError("the console signed in again instead of reissuing")
                    after_reissue = [row for row in window[reissued[0] + 1:] if row["path"] in {"/read", "/extension", "/session", "/apply"}]
                    if not after_reissue or any(row["status"] != 200 or row.get("ok") is not True for row in after_reissue):
                        raise RuntimeError("the repeated requests after the reissue did not succeed: " + json.dumps([[row["path"], row["status"], row.get("ok")] for row in after_reissue]))
                    if re.search(r"/login$", page.url) or not signed_in():
                        raise RuntimeError("the console signed the administrator out during the reissue")
                    digest = token_digest()
                    if digest is None or digest in digests:
                        raise RuntimeError("the reissue did not store a new access token")
                    digests.append(digest)
                    checks[f"reissue_round_{round_no}"] = {
                        "refused_requests": len(refused),
                        "reissue_requests": len(reissued),
                        "requests_after_reissue_ok": len(after_reissue),
                        "new_access_token_stored": True,
                    }
                    page.screenshot(path=str(output / f"admin-reissue-{round_no}.png"), full_page=True)
                checks["reissue_rounds"] = len(links)
            except Exception:
                try:
                    page.screenshot(path=str(output / "admin-reissue-failure-state.png"), full_page=True)
                except Exception:
                    pass
                raise
            context.close()
            browser.close()
            browser = None

        unknown = [row for row in requests if row["path"] not in AIP_PATHS]
        if unknown:
            raise RuntimeError("the console called an unexpected AIP path: " + json.dumps(sorted({str(row["path"]) for row in unknown})))
        uncollected = [row for row in requests if row.get("body_captured") is not True]
        if uncollected:
            raise RuntimeError("an API body was not collected (measurement gap): " + json.dumps(uncollected, ensure_ascii=False))
        if page_errors:
            raise RuntimeError("browser reported an unhandled React page error")
        report = {
            "ui_revision": UI_REVISION, "phase": "admin_reissue", "access_ttl_secs": ttl_secs, "checks": checks, "api_requests": requests,
            "journey_stage": state["stage"], "page_error_count": len(page_errors), "page_errors": page_errors,
            "credentials_recorded": False, "tokens_recorded": False, "browser_api_mocks": False,
        }
        (output / "admin-reissue-result.json").write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
        print(json.dumps({"phase": "admin_reissue", "journey_stage": state["stage"], "checks": checks}, ensure_ascii=False))
        return 0
    except Exception as error:
        failure_type = safe_browser_failure_type(error)
        report = {
            "ui_revision": UI_REVISION, "phase": "admin_reissue", "access_ttl_secs": ttl_secs, "checks": checks, "api_requests": requests,
            "journey_stage": state["stage"], "failure_type": failure_type,
            "failure_detail": (str(error).splitlines() or [""])[0][:300],
            "failure_line": next((frame.lineno for frame in reversed(traceback.extract_tb(error.__traceback__)) if frame.filename == __file__), None),
            "page_error_count": len(page_errors), "page_errors": page_errors,
            "credentials_recorded": False, "tokens_recorded": False, "browser_api_mocks": False,
        }
        (output / "admin-reissue-result.json").write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
        print(json.dumps({"phase": "admin_reissue", "journey_stage": state["stage"], "failure_type": failure_type, "detail": report["failure_detail"]}, ensure_ascii=False))
        return 1
    finally:
        if browser is not None:
            try:
                browser.close()
            except Exception:
                pass
        try:
            os.killpg(server.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        try:
            server.wait(timeout=10)
        except subprocess.TimeoutExpired:
            try:
                os.killpg(server.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            server.wait(timeout=10)


if __name__ == "__main__":
    raise SystemExit(main())
