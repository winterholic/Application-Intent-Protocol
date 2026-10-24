#!/usr/bin/env python3
"""Pinned original MyPage email verification UI against the live service and a run-owned in-memory mailbox.

Linux CI only. Codes, passwords and hashes never enter artifacts/stdout.
"""
import argparse
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import time
import traceback
from urllib.error import HTTPError
from urllib.parse import urlsplit
from urllib.request import urlopen

from playwright.sync_api import expect, sync_playwright

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
from product.tests.facereview_ui_browser_evidence import member_email_evidence, safe_browser_failure_type


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--ui", type=Path, required=True)
    parser.add_argument("--api", required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    email = os.environ["AIP_FACEREVIEW_JOURNEY_EMAIL"]
    password = os.environ["AIP_FACEREVIEW_JOURNEY_PASSWORD"]
    inbox_url = os.environ["AIP_FACEREVIEW_TEST_INBOX"]
    expected_contract = os.environ["AIP_FACEREVIEW_EXPECTED_CONTRACT"]
    ui_port = os.environ.get("AIP_FACEREVIEW_UI_PORT", "5173")
    binding = (args.ui / "src/aip/facereview.ts").read_text()
    match = re.search(r'export const contractFingerprint = "([a-f0-9]{64})";', binding)
    assert match and match.group(1) == expected_contract, "pinned binding must match service contract"
    assert urlsplit(inbox_url).hostname == "127.0.0.1", "test mailbox must be run-owned loopback"
    output = args.out.resolve()
    output.mkdir(parents=True, exist_ok=True)
    server_env = {key: value for key, value in os.environ.items() if not key.startswith("AIP_FACEREVIEW_")}
    server_env["VITE_AIP_URL"] = args.api
    server = subprocess.Popen(
        ["node", ".yarn/releases/yarn-4.12.0.cjs", "dev", "--host", "0.0.0.0", "--port", ui_port, "--strictPort"],
        cwd=args.ui, env=server_env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True,
    )
    rows = []
    pending = {}
    started = set()
    finished = set()
    failures = []
    errors = []
    ui = {}
    screenshots = []
    stage = "startup"
    code = ""
    evidence = {}
    failure_type = None
    failure_line = None
    failure_path = None
    page = None
    status = 1
    api_port = urlsplit(args.api).port

    def capture(response):
        if urlsplit(response.url).port != api_port:
            return
        request = response.request
        path = urlsplit(response.url).path
        row = {"path": path, "method": request.method, "status": response.status,
               "authorization_present": "authorization" in request.headers, "body_captured": False}
        if path in {"/read", "/extension", "/apply"}:
            row["contract_matches_runtime"] = request.headers.get("x-aip-contract") == expected_contract
        try:
            payload = request.post_data_json
        except Exception:
            payload = None
        if path == "/auth/login":
            kind = "current"
            row["password_kind"] = kind
            row["payload_matches"] = payload == {"email": email, "password": password}
        elif path == "/auth/email/verification":
            row["payload_matches"] = payload == {}
        elif path == "/auth/email/verify":
            row["payload_matches"] = bool(code) and payload == {"code": code}
        if path == "/apply" and isinstance(payload, dict):
            request_payload = payload.get("request")
            if isinstance(request_payload, dict) and request_payload.get("extension") == "User.myEmotionSummary":
                row["write_extension"] = "User.myEmotionSummary"
        rows.append(row)
        pending[request] = (response, row)

    def capture_body(request):
        if request not in pending:
            return
        response, row = pending.pop(request)
        finished.add(request)
        try:
            body = json.loads(response.body())
        except Exception:
            return
        row["body_captured"] = True
        if isinstance(body, dict):
            row["ok"] = body.get("ok") is True
            code = body.get("code")
            # Only an enumerated wire code (no message or detail) explains a refused SDK call.
            if not row["ok"] and isinstance(code, str) and re.fullmatch(r"[A-Z][A-Z0-9_]*(\.[A-Z][A-Z0-9_]*)+", code):
                row["error_code"] = code

    try:
        deadline = time.monotonic() + 30
        while True:
            if server.poll() is not None:
                raise RuntimeError("UI server exited")
            try:
                with urlopen(f"http://127.0.0.1:{ui_port}/", timeout=1) as response:
                    if response.status == 200:
                        break
            except Exception:
                if time.monotonic() > deadline:
                    raise RuntimeError("UI startup timeout") from None
                time.sleep(0.2)
        with sync_playwright() as playwright:
            browser = playwright.chromium.launch(headless=True)
            try:
                page = browser.new_page(viewport={"width": 1280, "height": 900})
                page.set_default_timeout(20000)
                page.on("pageerror", lambda error: errors.append(type(error).__name__))
                page.on("request", lambda request: started.add(request) if urlsplit(request.url).port == api_port else None)
                page.on("response", capture)
                page.on("requestfinished", capture_body)
                page.on("requestfailed", lambda request: failures.append({"path": urlsplit(request.url).path}) if urlsplit(request.url).port == api_port else None)

                def login(login_password):
                    page.locator("#authEmail").fill(email)
                    page.get_by_role("button", name="다음", exact=True).click()
                    expect(page).to_have_url(re.compile(r"/auth/2$"))
                    page.locator("#authPassword").fill(login_password)
                    page.get_by_role("button", name="로그인", exact=True).click()
                    expect(page).to_have_url(re.compile(r"/main$"), timeout=30000)
                    page.wait_for_load_state("networkidle")
                    page.get_by_role("button", name="마이페이지로 이동", exact=True).click()
                    expect(page).to_have_url(re.compile(r"/my$"))
                    expect(page.get_by_role("button", name="비밀번호 변경", exact=True)).to_be_visible()
                    page.wait_for_load_state("networkidle")

                def shot(name):
                    page.screenshot(path=str(output / name), full_page=True)
                    screenshots.append(name)

                stage = "login"
                page.goto(f"http://localhost:{ui_port}/auth/1", wait_until="domcontentloaded")
                login(password)
                banner = page.get_by_role("button", name="이메일 인증 진행하기", exact=True)
                badge = page.get_by_role("img", name="이메일 인증 완료된 계정입니다.", exact=True)
                stage = "unverified_banner"
                expect(banner).to_be_visible()
                expect(badge).to_have_count(0)
                ui["unverified_banner"] = True
                # Let the original content fade complete before screenshots.
                page.locator(".my-page-container").evaluate("el => Promise.all(el.getAnimations({subtree:true}).filter(a => a.animationName === 'fadeInUp').map(a => a.finished))")
                shot("email-unverified-banner.png")
                stage = "send_code"
                banner.click()
                expect(page.get_by_role("heading", name="이메일 인증", exact=True)).to_be_visible()
                code_input = page.get_by_role("textbox", name="6자리 인증 코드", exact=True)
                expect(code_input).to_be_visible()
                ui["code_modal"] = True
                shot("email-code-modal.png")
                deadline = time.monotonic() + 15
                while not code:
                    try:
                        with urlopen(inbox_url, timeout=2) as response:
                            candidate = json.loads(response.read()).get("code")
                            if isinstance(candidate, str) and re.fullmatch(r"[0-9]{6}", candidate):
                                code = candidate
                    except HTTPError as error:
                        if error.code != 404:
                            raise RuntimeError("test mailbox rejected") from None
                    if time.monotonic() > deadline:
                        raise RuntimeError("test mailbox deadline")
                    if not code:
                        page.wait_for_timeout(50)
                stage = "verify_code"
                code_input.fill(code)
                with page.expect_response(lambda response: urlsplit(response.url).port == api_port
                                          and urlsplit(response.url).path == "/auth/email/verify") as verified_reply:
                    page.get_by_role("button", name="확인", exact=True).click()
                verified_reply.value.finished()
                capture_body(verified_reply.value.request)
                expect(code_input).to_have_count(0)
                expect(banner).to_have_count(0)
                expect(badge).to_be_visible()
                ui["verified_badge"] = True
                ui["banner_removed"] = True
                shot("email-verified.png")
                stage = "reload_verified"
                page.reload(wait_until="domcontentloaded")
                expect(page.get_by_role("heading", name="AIP 비밀번호 표본회원님", exact=True)).to_be_visible()
                expect(badge).to_be_visible()
                expect(banner).to_have_count(0)
                page.wait_for_load_state("networkidle")
                page.locator(".my-page-container").evaluate("el => Promise.all(el.getAnimations({subtree:true}).filter(a => a.animationName === 'fadeInUp').map(a => a.finished))")
                ui["reloaded_verified"] = True
                shot("email-verified-reloaded.png")
                evidence = member_email_evidence(rows, ui)
                assert evidence["verified"], "email verification bodies and persistent original UI must all be verified"
                assert not failures and started == finished, "all API requests must finish"
                assert all(row.get("body_captured") is True and row["status"] == 200 for row in rows), "all API bodies must complete successfully"
                assert all(row.get("contract_matches_runtime", True) is True for row in rows), "SDK headers must match"
                assert not errors, "React page errors"
                stage = "completed"
                status = 0
            except Exception as error:
                failure_type = safe_browser_failure_type(type(error).__name__)
                failure_line = next((frame.lineno for frame in reversed(traceback.extract_tb(error.__traceback__)) if Path(frame.filename).resolve() == Path(__file__).resolve()), None)
                if page is not None:
                    failure_path = urlsplit(page.url).path
                    try:
                        page.locator("input").evaluate_all("els => els.forEach(e => { e.value = ''; })")
                        page.screenshot(path=str(output / "email-failure.png"), full_page=True)
                        screenshots.append("email-failure.png")
                    except Exception:
                        pass
            finally:
                browser.close()
    except Exception as error:
        failure_type = safe_browser_failure_type(type(error).__name__)
        failure_line = next((frame.lineno for frame in reversed(traceback.extract_tb(error.__traceback__)) if Path(frame.filename).resolve() == Path(__file__).resolve()), None)
        status = 1
    finally:
        if server.poll() is None:
            os.killpg(server.pid, signal.SIGTERM)
            try:
                server.wait(timeout=10)
            except subprocess.TimeoutExpired:
                os.killpg(server.pid, signal.SIGKILL)
                server.wait(timeout=5)
        report = {"ui_revision": "2e7c021fa5abebd72278d841b8066a33d45cfbb3", "journey_stage": stage,
                  "member_email": evidence, "ui": ui, "api_requests": rows, "failure_type": failure_type, "failure_line": failure_line, "failure_path": failure_path,
                  "page_error_count": len(errors), "api_lifecycle": {"requests": len(started), "finished": len(finished),
                  "failed": len(failures), "unfinished": len(started - finished)}, "screenshots": screenshots,
                  "credentials_recorded": False, "browser_api_mocks": False}
        (output / "email-browser-result.json").write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
        print(json.dumps({"journey": "member_email", "stage": stage, "status": status}))
    return status


if __name__ == "__main__":
    raise SystemExit(main())
