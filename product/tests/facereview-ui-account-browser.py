#!/usr/bin/env python3
"""Pinned original AuthPage signup and TutorialPage UI against the live service and disposable MySQL fixture.

Linux CI only. Tokens, passwords and hashes never enter artifacts/stdout.
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
    member_signup_evidence,
    member_tutorial_evidence,
    safe_browser_failure_type,
)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--ui", type=Path, required=True)
    parser.add_argument("--api", required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--phase", choices=["signup", "complete", "skip"], required=True)
    args = parser.parse_args()
    email = os.environ["AIP_FACEREVIEW_JOURNEY_EMAIL"]
    password = os.environ["AIP_FACEREVIEW_JOURNEY_PASSWORD"]
    fixture_name = os.environ["AIP_FACEREVIEW_JOURNEY_NAME"]
    user_id = os.environ.get("AIP_FACEREVIEW_JOURNEY_USER_ID", "")
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
        if path in {"/auth/login", "/auth/check-email", "/auth/signup"}:
            expected = ({"email": email} if path == "/auth/check-email" else
                        {"email": email, "password": password} if path == "/auth/login" else
                        {"email": email, "password": password, "name": fixture_name, "favorite_genres": ["eating", "animal"]})
            row["payload_matches"] = payload == expected
        elif path == "/apply" and isinstance(payload, dict):
            request_payload = payload.get("request")
            if isinstance(request_payload, dict) and request_payload.get("apply") == "User.completeTutorial":
                row["tutorial_action"] = True
                row["payload_matches"] = bool(user_id) and request_payload == {"apply": "User.completeTutorial", "target": {"ids": [user_id]}}
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
            error_code = body.get("code")
            if not row["ok"] and isinstance(error_code, str) and re.fullmatch(r"[A-Z][A-Z0-9_]*(\.[A-Z][A-Z0-9_]*)+", error_code):
                row["error_code"] = error_code
            if row["path"] == "/auth/check-email" and type(body.get("is_duplicate")) is bool:
                row["is_duplicate"] = body["is_duplicate"]

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
                    # The original tutorial enters with a finite opacity animation.
                    # Wait for its rendered frame before keeping visual evidence.
                    if name.startswith("tutorial-"):
                        page.evaluate("""async () => {
                          const root = document.querySelector('.tutorial-content');
                          if (!root) return;
                          const animations = root.getAnimations({subtree: true}).filter(a => {
                            const t = a.effect?.getComputedTiming();
                            return Number.isFinite(Number(t?.duration)) && Number.isFinite(Number(t?.iterations));
                          });
                          await Promise.all(animations.map(a => a.finished));
                        }""")
                    page.screenshot(path=str(output / name), full_page=True)
                    screenshots.append(name)

                def check_email():
                    page.locator("#authEmail").fill(email)
                    page.get_by_role("button", name="다음", exact=True).click()
                    expect(page).to_have_url(re.compile(r"/auth/2$"))
                    expect(page.locator("#authPassword")).to_be_visible()

                stage = "start_auth"
                page.goto(f"http://localhost:{ui_port}/auth/1", wait_until="domcontentloaded")
                if args.phase == "signup":
                    stage = "new_email_branch"
                    check_email()
                    expect(page.locator("#authPasswordConfirm")).to_be_visible()
                    ui["new_email_branch"] = True
                    stage = "signup_profile"
                    page.locator("#authPassword").fill(password)
                    page.locator("#authPasswordConfirm").fill(password)
                    page.get_by_role("button", name="다음", exact=True).click()
                    expect(page).to_have_url(re.compile(r"/auth/3$"))
                    page.locator("#authNickname").fill(fixture_name)
                    for label in ("먹방", "동물"):
                        button = page.get_by_role("button", name=re.compile(label))
                        button.click()
                        expect(button).to_have_attribute("aria-pressed", "true")
                    ui["categories_selected"] = True
                    shot("signup-profile-selected.png")
                    stage = "submit_signup"
                    with page.expect_response(lambda r: urlsplit(r.url).port == api_port and urlsplit(r.url).path == "/auth/signup") as reply:
                        page.get_by_role("button", name="회원가입", exact=True).click()
                    reply.value.finished()
                    capture_body(reply.value.request)
                    expect(page).to_have_url(re.compile(r"/auth/1$"))
                    ui["returned_to_login"] = True
                    # The original retains form state after SPA navigation; reload clears credentials before a screenshot.
                    page.reload(wait_until="domcontentloaded")
                    expect(page.locator("#authEmail")).to_have_value("")
                    shot("signup-returned-to-login.png")
                    stage = "existing_email_branch"
                    check_email()
                    expect(page.locator("#authPasswordConfirm")).to_have_count(0)
                    expect(page.get_by_role("button", name="로그인", exact=True)).to_be_visible()
                    ui["existing_email_branch"] = True
                    page.wait_for_load_state("networkidle")
                    evidence = member_signup_evidence(rows, ui)
                else:
                    stage = "login"
                    check_email()
                    page.locator("#authPassword").fill(password)
                    page.get_by_role("button", name="로그인", exact=True).click()
                    expect(page).to_have_url(re.compile(r"/tutorial/1$"), timeout=30000)
                    expect(page.get_by_role("heading", name="감정 기반 영상 추천", exact=True)).to_be_visible()
                    ui["initial_step"] = True
                    shot("tutorial-initial.png")
                    if args.phase == "complete":
                        stage = "tutorial_steps"
                        page.get_by_role("button", name="다음", exact=True).click()
                        expect(page).to_have_url(re.compile(r"/tutorial/2$"))
                        expect(page.get_by_role("heading", name="실시간 표정 기록", exact=True)).to_be_visible()
                        shot("tutorial-second.png")
                        page.get_by_role("button", name="다음", exact=True).click()
                        expect(page).to_have_url(re.compile(r"/tutorial/3$"))
                        expect(page.get_by_role("heading", name="더 정확한 맞춤 추천", exact=True)).to_be_visible()
                        ui["all_steps"] = True
                        shot("tutorial-final-step.png")
                    stage = "tutorial_action"
                    with page.expect_response(lambda r: urlsplit(r.url).port == api_port and urlsplit(r.url).path == "/apply") as reply:
                        page.get_by_role("button", name="완료" if args.phase == "complete" else "건너뛰기", exact=True).click()
                    reply.value.finished()
                    capture_body(reply.value.request)
                    ui["action_clicked"] = True
                    expect(page).to_have_url(re.compile(r"/main$"), timeout=30000)
                    expect(page.get_by_role("button", name="마이페이지로 이동", exact=True)).to_be_visible()
                    ui["main_visible"] = True
                    page.wait_for_load_state("networkidle")
                    stage = "reload_main"
                    page.reload(wait_until="domcontentloaded")
                    expect(page.get_by_role("button", name="마이페이지로 이동", exact=True)).to_be_visible()
                    # App mounts getUserName() after reload and refreshes this store.
                    # The pinned partialize intentionally omits user_tutorial from localStorage.
                    page.wait_for_function("""async (target) => {
                      const { useAuthStorage } = await import('/src/store/authStore.ts');
                      const s = useAuthStorage.getState();
                      return s.is_sign_in === true && s.user_id === target && s.user_tutorial === 1;
                    }""", arg=user_id)
                    expect(page).to_have_url(re.compile(r"/main$"))
                    page.wait_for_load_state("networkidle")
                    ui["reloaded_tutorial_done"] = True
                    shot("tutorial-main-reloaded.png")
                    evidence = member_tutorial_evidence(rows, ui, args.phase)
                assert evidence["verified"], "exact completed auth/tutorial responses and original UI must be verified"
                assert not failures and started == finished, "all API requests must finish"
                assert all(row.get("body_captured") is True and row["status"] in {200, 201} and row.get("ok") is True for row in rows), "all API bodies must complete successfully"
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
                        page.screenshot(path=str(output / "account-failure.png"), full_page=True)
                        screenshots.append("account-failure.png")
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
                  "member_signup" if args.phase == "signup" else "member_tutorial": evidence, "ui": ui, "api_requests": rows, "failure_type": failure_type, "failure_line": failure_line, "failure_path": failure_path,
                  "page_error_count": len(errors), "api_lifecycle": {"requests": len(started), "finished": len(finished),
                  "failed": len(failures), "unfinished": len(started - finished)}, "screenshots": screenshots,
                  "credentials_recorded": False, "browser_api_mocks": False}
        (output / f"{args.phase}-browser-result.json").write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
        print(json.dumps({"journey": args.phase, "stage": stage, "status": status}))
    return status


if __name__ == "__main__":
    raise SystemExit(main())
