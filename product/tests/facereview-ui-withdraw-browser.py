#!/usr/bin/env python3
"""Pinned original EditPage withdrawal UI against the live service.

Linux CI only. Passwords, tokens and hashes never enter artifacts/stdout.
One disposable member: login -> /my -> /edit -> withdraw modal -> confirm ->
success toast -> logout -> /main signed out.
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
from urllib.parse import urlsplit
from urllib.request import urlopen

from playwright.sync_api import expect, sync_playwright

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
from product.tests.facereview_ui_browser_evidence import (
    body_failure_reason,
    member_withdraw_evidence,
    safe_browser_failure_type,
)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--ui", type=Path, required=True)
    parser.add_argument("--api", required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    email = os.environ["AIP_FACEREVIEW_JOURNEY_EMAIL"]
    password = os.environ["AIP_FACEREVIEW_JOURNEY_PASSWORD"]
    user_id = os.environ["AIP_FACEREVIEW_JOURNEY_USER_ID"]
    expected_contract = os.environ["AIP_FACEREVIEW_EXPECTED_CONTRACT"]
    ui_port = os.environ.get("AIP_FACEREVIEW_UI_PORT", "5173")
    binding = (args.ui / "src/aip/facereview.ts").read_text()
    match = re.search(r'export const contractFingerprint = "([a-f0-9]{64})";', binding)
    assert match and match.group(1) == expected_contract, "pinned binding must match service contract"
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
            row["payload_matches"] = payload == {"email": email, "password": password}
        elif path == "/auth/logout":
            row["payload_matches"] = payload == {}
            row["refresh_cookie_present"] = "refresh_token=" in request.all_headers().get("cookie", "")
        if path == "/apply" and isinstance(payload, dict):
            request_payload = payload.get("request")
            if isinstance(request_payload, dict) and request_payload.get("apply") == "User.withdraw":
                row["withdraw_action"] = True
                row["payload_matches"] = bool(user_id) and request_payload == {"apply": "User.withdraw", "target": {"ids": [user_id]}}
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
        except Exception as error:
            # JSONDecodeError is a ValueError; everything else is Playwright's. The gate still fails.
            row["body_failure_reason"] = "json_decode_error" if isinstance(error, ValueError) else body_failure_reason(error)
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

                def shot(name):
                    page.screenshot(path=str(output / name), full_page=True)
                    screenshots.append(name)

                stage = "login"
                page.goto(f"http://localhost:{ui_port}/auth/1", wait_until="domcontentloaded")
                page.locator("#authEmail").fill(email)
                page.get_by_role("button", name="다음", exact=True).click()
                expect(page).to_have_url(re.compile(r"/auth/2$"))
                page.locator("#authPassword").fill(password)
                page.get_by_role("button", name="로그인", exact=True).click()
                expect(page).to_have_url(re.compile(r"/main$"), timeout=30000)
                page.wait_for_load_state("networkidle")
                page.get_by_role("button", name="마이페이지로 이동", exact=True).click()
                expect(page).to_have_url(re.compile(r"/my$"))
                page.wait_for_load_state("networkidle")
                stage = "edit_page"
                edit_entry = page.locator(".my-page-name-wrapper").get_by_role("button", name="다음", exact=True)
                expect(edit_entry).to_be_visible()
                edit_entry.click()
                expect(page).to_have_url(re.compile(r"/edit$"))
                expect(page.locator(".edit-page-withdraw-button")).to_be_visible()
                ui["edit_page"] = True
                shot("withdraw-before-confirm.png")
                stage = "open_modal"
                page.locator(".edit-page-withdraw-button").click()
                expect(page.get_by_role("alertdialog")).to_be_visible()
                expect(page.get_by_role("heading", name="정말 탈퇴하시겠어요?", exact=True)).to_be_visible()
                ui["withdraw_modal"] = True
                shot("withdraw-modal.png")
                stage = "confirm_withdraw"
                with page.expect_response(lambda response: urlsplit(response.url).port == api_port
                                          and urlsplit(response.url).path == "/apply", timeout=30000) as reply:
                    page.locator(".withdraw-modal-confirm").click()
                reply.value.finished()
                capture_body(reply.value.request)
                # The original toasts success, closes the modal, then logs out to /main.
                expect(page.get_by_text("탈퇴가 완료되었어요", exact=True)).to_be_visible(timeout=10000)
                ui["withdraw_confirmed"] = True
                shot("withdraw-confirmed.png")
                expect(page).to_have_url(re.compile(r"/main$"), timeout=30000)
                page.wait_for_function("""() => {
                  const s = JSON.parse(localStorage.getItem('auth-storage') || 'null')?.state;
                  return s?.is_sign_in === false && s?.access_token === '' && s?.user_id === '';
                }""")
                page.wait_for_load_state("networkidle")
                ui["signed_out"] = True
                shot("withdraw-signed-out.png")
                evidence = member_withdraw_evidence(rows, ui)
                assert evidence["verified"], "withdraw bodies and original UI must all be verified"
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
                        page.screenshot(path=str(output / "withdraw-failure.png"), full_page=True)
                        screenshots.append("withdraw-failure.png")
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
                  "member_withdraw": evidence, "ui": ui, "api_requests": rows, "failure_type": failure_type, "failure_line": failure_line, "failure_path": failure_path,
                  "page_error_count": len(errors), "api_lifecycle": {"requests": len(started), "finished": len(finished),
                  "failed": len(failures), "unfinished": len(started - finished)}, "screenshots": screenshots,
                  "credentials_recorded": False, "browser_api_mocks": False}
        (output / "withdraw-browser-result.json").write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
        print(json.dumps({"journey": "member_withdraw", "stage": stage, "status": status}))
    return status


if __name__ == "__main__":
    raise SystemExit(main())
