#!/usr/bin/env python3
"""Verify the pinned React ADMIN console (new-facereview-admin-front ab0386c) against the real AIP service.

This is separate from the member journeys (facereview-ui-member-browser.py, facereview-ui-profile-browser.py): another repository, another
build, another fixture set. Nothing is mocked in the browser. The console runs its own Vite dev server with `VITE_AIP_URL` pointing at the
service, logs in over `/auth/login`, and every list, dashboard figure and write goes through the generated TypeScript SDK.

Recorded evidence is allowlist-only: request method, path, status, whether the contract header matched, the resource, aggregate, extension
or operation NAME (schema constants) and the envelope's ok/code. No credential, token, cookie, request or response value is written.
"""

import argparse
import json
import os
from pathlib import Path
import re
import signal
import socket
import subprocess
import sys
import time
import traceback
from urllib.parse import urlsplit
import urllib.request

from playwright.sync_api import expect, sync_playwright

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
from product.tests.facereview_ui_browser_evidence import (  # noqa: E402
    admin_request_evidence,
    body_failure_reason,
    safe_page_error_name,
    safe_browser_failure_type,
    safe_wire_code,
)

UI_REVISION = "ab0386c878a5ba5c7c079a262b987c10d211d22d"
APPROVED_URL = "reqvideo001"
REJECTED_URL = "reqvideo002"
MEMBER_ID = "77777777-7777-4777-8777-777777777777"
ADMIN_ID = "55555555-5555-4555-8555-555555555555"
SUPER_ADMIN_ID = "33333333-3333-4333-8333-333333333333"
APPROVED_TITLE = "승인 표본 영상"
APPROVED_CHANNEL = "표본 채널"
APPROVED_DURATION = "321"
APPROVED_CATEGORY = "vlog"
REJECT_REASON = "중복 영상입니다"
SEED_USERS = ["앨리스", "밥", "캐롤", "데이브", "에린", "일반 표본회원"]
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
    member_email = os.environ["AIP_FACEREVIEW_MEMBER_EMAIL"]
    member_password = os.environ["AIP_FACEREVIEW_MEMBER_PASSWORD"]
    secondary_admin_email = os.environ["AIP_FACEREVIEW_SECONDARY_ADMIN_EMAIL"]
    secondary_admin_password = os.environ["AIP_FACEREVIEW_SECONDARY_ADMIN_PASSWORD"]
    expected_contract = os.environ["AIP_FACEREVIEW_EXPECTED_CONTRACT"]
    redis_configured = os.environ.get("AIP_FACEREVIEW_REDIS_CONFIGURED") == "1"
    ui_port = os.environ.get("AIP_FACEREVIEW_UI_PORT", "5173")
    output = args.out.resolve()
    output.mkdir(parents=True, exist_ok=True)
    api_port = urlsplit(args.api).port

    # A Vite already listening here would be somebody else's server (or a leaked one), and the readiness probe below would trust it.
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as probe:
        probe.settimeout(1)
        if probe.connect_ex(("127.0.0.1", int(ui_port))) == 0:
            raise SystemExit(f"UI port {ui_port} is already in use; pick a free AIP_FACEREVIEW_UI_PORT")

    server_env = os.environ.copy()
    server_env["VITE_AIP_URL"] = args.api
    for name in ("AIP_FACEREVIEW_ADMIN_PASSWORD", "AIP_FACEREVIEW_MEMBER_PASSWORD", "AIP_FACEREVIEW_ADMIN_EMAIL", "AIP_FACEREVIEW_MEMBER_EMAIL"):
        server_env.pop(name, None)
    server = subprocess.Popen(
        ["node", "node_modules/vite/bin/vite.js", "--host", "127.0.0.1", "--port", ui_port, "--strictPort"],
        cwd=args.ui,
        env=server_env,
        start_new_session=True,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    requests: list[dict[str, object]] = []
    rest_fallback: list[str] = []
    pending_bodies: dict[object, tuple[object, dict[str, object]]] = {}
    page_errors: list[dict[str, str]] = []
    state: dict[str, object] = {"stage": "startup", "host_metrics_null": None}
    checks: dict[str, object] = {}
    failure_type = None
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
                if parsed.port != api_port and (parsed.path.startswith("/api/") or "/v2/" in parsed.path):
                    rest_fallback.append(parsed.path.split("/")[1] if parsed.path.count("/") else "")
                if parsed.port != api_port:
                    return
                row: dict[str, object] = {"stage": state["stage"], "method": response.request.method, "path": parsed.path, "status": response.status}
                if parsed.path in {"/read", "/apply", "/extension", "/jobs/enqueue", "/jobs/status", "/jobs/result"}:
                    row["contract_matches_runtime"] = response.request.headers.get("x-aip-contract") == expected_contract
                try:
                    request_body = response.request.post_data_json
                except Exception:
                    request_body = None
                row.update(admin_request_evidence(parsed.path, request_body))
                requests.append(row)
                pending_bodies[response.request] = (response, row)

            def capture_body(request) -> None:
                # `requestfinished` fires after the body is complete; the response event can precede it and a read there may lose the body.
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
                    if row.get("extension") == "User.systemStatus" and isinstance(envelope.get("output"), dict):
                        try:
                            host = json.loads(envelope["output"]["body"])["server"]
                            state["host_metrics_null"] = host.get("cpu_usage") is None
                        except Exception:
                            state["host_metrics_null"] = None

            def settle() -> None:
                deadline = time.monotonic() + 20
                while pending_bodies and time.monotonic() < deadline:
                    page.wait_for_timeout(50)
                if pending_bodies:
                    raise AssertionError("API response bodies did not finish before the next step")

            def settled_idle() -> None:
                page.wait_for_load_state("networkidle", timeout=20000)
                settle()

            def field(scope, label: str):
                # The console's TextInput has no id, so a label is not tied to its input; scope by the wrapper that holds the label text.
                return scope.locator(".text-input", has_text=label).locator("input")

            def stored_auth() -> dict[str, object]:
                raw = page.evaluate("localStorage.getItem('admin-auth-storage')")
                parsed = json.loads(raw) if raw else {}
                saved = parsed.get("state", {}) if isinstance(parsed, dict) else {}
                return {"is_sign_in": saved.get("is_sign_in") is True, "has_access_token": bool(saved.get("access_token")), "role": saved.get("role") if saved.get("role") in {"", "GENERAL", "ADMIN", "SUPER_ADMIN"} else "unrecognized"}

            page.on("response", capture_response)
            page.on("requestfinished", capture_body)

            try:
                # Vite optimizes dependencies on the first load of a fresh checkout and may reload the page once; do that before any form is filled.
                stage("warm_up")
                page.goto(f"{base}/login", wait_until="domcontentloaded")
                page.wait_for_load_state("networkidle", timeout=60000)
                page.reload(wait_until="domcontentloaded")
                page.wait_for_load_state("networkidle", timeout=60000)

                # 1. Unauthenticated: a protected route sends the visitor to the login page and no admin data is requested.
                stage("unauthenticated_redirect")
                page.goto(f"{base}/users", wait_until="domcontentloaded")
                expect(page).to_have_url(re.compile(r"/login$"))
                expect(page.get_by_role("heading", name="FaceReview Admin")).to_be_visible()
                checks["unauthenticated_redirected_to_login"] = True
                settled_idle()
                if any(row["path"] in {"/read", "/extension", "/jobs/enqueue"} for row in requests):
                    raise RuntimeError("an unauthenticated visit requested administrator data")

                # 2. A member signs in successfully (the service issues member tokens) and the console refuses them at the role check.
                stage("member_login_refused")
                field(page, "이메일").fill(member_email)
                field(page, "비밀번호").fill(member_password)
                page.get_by_role("button", name="로그인", exact=True).click()
                expect(page.get_by_text("관리자 권한이 없습니다.")).to_be_visible()
                expect(page).to_have_url(re.compile(r"/login$"))
                settled_idle()
                member_state = stored_auth()
                if member_state["is_sign_in"] or member_state["has_access_token"]:
                    raise RuntimeError("the console kept a member session")
                checks["member_login_refused_by_console"] = True
                if any(row.get("aggregate") or row.get("path") in {"/extension", "/jobs/enqueue"} for row in requests):
                    raise RuntimeError("a member session requested administrator aggregates, extensions or jobs")
                page.screenshot(path=str(output / "admin-member-refused.png"), full_page=True)
                page.goto(f"{base}/users", wait_until="domcontentloaded")
                expect(page).to_have_url(re.compile(r"/login$"))
                checks["member_cannot_open_protected_route"] = True

                # 3. The SUPER_ADMIN signs in; the console lands on the dashboard.
                stage("admin_login")
                field(page, "이메일").fill(admin_email)
                field(page, "비밀번호").fill(admin_password)
                page.get_by_role("button", name="로그인", exact=True).click()
                expect(page).to_have_url(re.compile(r"/$"), timeout=30000)
                settled_idle()
                admin_state = stored_auth()
                if not admin_state["is_sign_in"] or admin_state["role"] != "SUPER_ADMIN":
                    raise RuntimeError("the administrator session was not stored as SUPER_ADMIN")
                checks["admin_login_role"] = admin_state["role"]

                stage("dashboard")
                if state["host_metrics_null"] is True:
                    expect(page.get_by_role("heading", name="대시보드", exact=True)).to_be_visible()
                    expect(page.get_by_text("호스트 지표를 사용할 수 없습니다.")).to_be_visible()
                    expect(page.get_by_text("주간 활성 시청자")).to_be_visible()
                    expect(page.locator(".overview-strip__connection").get_by_text("정상")).to_have_count(3)
                    settled_idle()
                    page.screenshot(path=str(output / "admin-dashboard-host-metrics-unavailable.png"), full_page=True)
                    checks["dashboard"] = "rendered_partial_host_metrics_unavailable"
                else:
                    expect(page.get_by_role("heading", name="대시보드", exact=True)).to_be_visible()
                    expect(page.get_by_text("주간 활성 시청자")).to_be_visible()
                    expect(page.get_by_text("CPU ")).to_be_visible()
                    if redis_configured:
                        expect(page.locator(".overview-strip__connection").get_by_text("정상")).to_have_count(3)
                    settled_idle()
                    page.screenshot(path=str(output / "admin-dashboard.png"), full_page=True)
                    checks["dashboard"] = "rendered"
                    for period in ("7d",):
                        checks[f"signup_trend_{period}"] = any(row.get("aggregate") == "User.signupCount" for row in requests)

                # 4. Users: the list, its total and a keyword search.
                stage("users")
                # Both dashboard outcomes (full host figures, or the partial state without them) leave the console on its dashboard.
                if str(checks["dashboard"]).startswith("rendered"):
                    page.get_by_role("link", name="회원 관리").click()
                expect(page.get_by_role("heading", name="회원 관리")).to_be_visible()
                for name in SEED_USERS:
                    expect(page.locator("tr", has_text=name).first).to_be_visible()
                expect(page.get_by_text("전체 6건")).to_be_visible()
                page.get_by_placeholder("이름 또는 이메일 검색").fill("밥")
                expect(page.get_by_text("전체 1건")).to_be_visible()
                expect(page.locator("tbody tr")).to_have_count(1)
                settled_idle()
                page.get_by_placeholder("이름 또는 이메일 검색").fill("")
                expect(page.get_by_text("전체 6건")).to_be_visible()
                settled_idle()
                page.screenshot(path=str(output / "admin-users.png"), full_page=True)
                checks["users_total"] = 6

                # SUPER_ADMIN can promote a GENERAL member to ADMIN through the actual console SDK.
                stage("users_write")
                promoted_member = page.locator("tr", has_text="legacy@example.com")
                promoted_member.get_by_role("button", name="ADMIN 지정", exact=True).click()
                expect(page.get_by_text("권한을 변경했습니다.")).to_be_visible()
                expect(promoted_member.get_by_text("ADMIN", exact=True)).to_be_visible()
                settled_idle()
                checks["super_admin_promoted_general_to_admin"] = True

                # 5. Requests: approve one (a durable job), reject another, and read both states back through the status filter.
                stage("requests_approve")
                page.get_by_role("link", name="영상 요청").click()
                expect(page.get_by_role("heading", name="영상 요청 관리")).to_be_visible()
                approved_row = page.locator("tr", has_text=APPROVED_URL)
                expect(approved_row).to_be_visible()
                expect(approved_row.get_by_text("대기중")).to_be_visible()
                expect(page.locator("tr", has_text=REJECTED_URL).get_by_text("대기중")).to_be_visible()
                settled_idle()
                approved_row.get_by_role("button", name="승인", exact=True).click()
                panel = page.locator(".confirm-modal__panel")
                field(panel, "영상 제목").fill(APPROVED_TITLE)
                field(panel, "채널명").fill(APPROVED_CHANNEL)
                field(panel, "영상 길이 (초)").fill(APPROVED_DURATION)
                panel.locator("select").select_option(APPROVED_CATEGORY)
                with page.expect_response(lambda response: urlsplit(response.url).path == "/jobs/enqueue") as enqueue:
                    panel.get_by_role("button", name="승인", exact=True).click()
                if enqueue.value.status != 200:
                    raise RuntimeError("the approval job was not accepted")
                expect(page.get_by_text("영상 요청을 승인했습니다.")).to_be_visible(timeout=90000)
                expect(approved_row.get_by_text("승인됨")).to_be_visible()
                settled_idle()
                page.screenshot(path=str(output / "admin-requests-approved.png"), full_page=True)
                checks["approval_job_accepted_and_row_accepted"] = True

                stage("requests_reject")
                page.locator("select").first.select_option("PENDING")
                expect(page.locator("tbody tr")).to_have_count(1)
                rejected_row = page.locator("tr", has_text=REJECTED_URL)
                expect(rejected_row).to_be_visible()
                rejected_row.get_by_role("button", name="거절", exact=True).click()
                panel = page.locator(".confirm-modal__panel")
                field(panel, "거절 사유").fill(REJECT_REASON)
                panel.get_by_role("button", name="거절", exact=True).click()
                expect(page.get_by_text("영상 요청을 거절했습니다.")).to_be_visible(timeout=30000)
                expect(page.get_by_text("데이터가 없습니다.")).to_be_visible()
                page.locator("select").first.select_option("REJECTED")
                expect(page.locator("tr", has_text=REJECTED_URL).get_by_text(REJECT_REASON)).to_be_visible()
                settled_idle()
                page.screenshot(path=str(output / "admin-requests-rejected.png"), full_page=True)
                checks["rejection_row_rejected_with_reason"] = True

                # The original exposes no action once a request has left PENDING. A direct generated-SDK transition must keep that server
                # boundary even if a caller fabricates the old action; the envelope and row state both remain unsuccessful/unchanged.
                stage("closed_request_refusal")
                stale_request = page.evaluate("""async (requestId) => {
                  const { adminWrites } = await import('/src/api/aipClient.ts');
                  try {
                    await adminWrites.rejectVideoRequest(requestId, 'stale repeat');
                    return { code: 'UNEXPECTED_SUCCESS' };
                  } catch (error) {
                    return { code: typeof error?.code === 'string' ? error.code : 'UNKNOWN' };
                  }
                }""", "ffffffff-0000-4000-8000-000000000002")
                checks["closed_request_refusal_code"] = stale_request["code"]
                if stale_request["code"] != "WIRE.INVALID_STATE":
                    raise RuntimeError("a non-PENDING request did not preserve the declared server transition boundary")
                expect(page.locator("tr", has_text=REJECTED_URL).get_by_text(REJECT_REASON)).to_be_visible()

                # The same request, already REJECTED, cannot be approved either: the durable job refuses it and no video is made.
                stage("closed_request_approval_refusal")
                stale_approval = page.evaluate("""async (requestId) => {
                  const { adminApproval } = await import('/src/api/aipClient.ts');
                  try {
                    await adminApproval.approveVideoRequest(requestId, { youtube_title: 'stale approval', channel_name: 'stale', duration: 5, category: 'etc' });
                    return { code: 'UNEXPECTED_SUCCESS' };
                  } catch (error) {
                    return { code: typeof error?.code === 'string' ? error.code : 'UNKNOWN' };
                  }
                }""", "ffffffff-0000-4000-8000-000000000002")
                checks["closed_request_approval_code"] = stale_approval["code"]
                if stale_approval["code"] != "WIRE.EXTENSION_ERROR":
                    raise RuntimeError("approving an already decided request did not end as a failed approval job")
                expect(page.locator("tr", has_text=REJECTED_URL).get_by_text(REJECT_REASON)).to_be_visible()

                # 6. Videos: the approved video is now listed; the soft-deleted and the NULL-flag seed videos are not.
                stage("videos")
                page.get_by_role("link", name="영상 관리").click()
                expect(page.get_by_role("heading", name="영상 관리")).to_be_visible()
                expect(page.locator("tr", has_text=APPROVED_TITLE)).to_be_visible()
                expect(page.locator("tr", has_text="고양이 영상")).to_be_visible()
                expect(page.get_by_text("삭제된 영상")).to_have_count(0)
                expect(page.get_by_text("전체 3건")).to_be_visible()
                settled_idle()
                page.screenshot(path=str(output / "admin-videos.png"), full_page=True)
                checks["videos_total"] = 3
                stage("videos_write")
                page.locator("tr", has_text="요리 영상 고양이").get_by_role("button", name="삭제", exact=True).click()
                page.locator(".confirm-modal__panel").get_by_role("button", name="삭제", exact=True).click()
                expect(page.get_by_text("영상을 삭제했습니다.")).to_be_visible()
                expect(page.get_by_text("요리 영상 고양이")).to_have_count(0)
                expect(page.get_by_text("전체 2건")).to_be_visible()
                settled_idle()
                checks["video_deleted_and_hidden"] = True
                # The original deletes a video again without complaint; AIP's Video row policy hides a deleted video, so the transition cannot
                # find it. Recorded as a difference from the original, with the row left as it was (the DB is read back by the Rust test).
                stage("deleted_video_redelete")
                redelete = page.evaluate("""async (videoId) => {
                  const { adminWrites } = await import('/src/api/aipClient.ts');
                  try {
                    await adminWrites.deleteVideo(videoId);
                    return { code: 'UNEXPECTED_SUCCESS' };
                  } catch (error) {
                    return { code: typeof error?.code === 'string' ? error.code : 'UNKNOWN' };
                  }
                }""", "aaaaaaaa-0000-4000-8000-000000000002")
                checks["deleted_video_redelete_code"] = redelete["code"]
                if redelete["code"] != "WIRE.MISSING_TARGET":
                    raise RuntimeError("deleting an already deleted video did not end in the declared missing-target refusal")

                # 7. Comments, then logout.
                stage("comments")
                page.get_by_role("link", name="댓글 관리").click()
                expect(page.get_by_role("heading", name="댓글 관리")).to_be_visible()
                expect(page.locator("tr", has_text="첫 댓글")).to_be_visible()
                expect(page.get_by_text("전체 5건")).to_be_visible()
                settled_idle()
                page.screenshot(path=str(output / "admin-comments.png"), full_page=True)
                checks["comments_total"] = 5
                stage("comments_write")
                comment_row = page.locator("tr", has_text="첫 댓글")
                comment_row.get_by_role("button", name="삭제", exact=True).click()
                page.locator(".confirm-modal__panel").get_by_role("button", name="삭제", exact=True).click()
                expect(page.get_by_text("댓글을 삭제했습니다.")).to_be_visible()
                expect(comment_row.get_by_text("삭제됨", exact=True)).to_be_visible()
                expect(page.get_by_text("전체 5건")).to_be_visible()
                settled_idle()
                page.screenshot(path=str(output / "admin-comments-after-delete.png"), full_page=True)
                checks["comment_soft_deleted"] = True

                stage("logout")
                page.get_by_role("button", name="로그아웃").click()
                expect(page).to_have_url(re.compile(r"/login$"))
                settled_idle()
                if stored_auth()["is_sign_in"]:
                    raise RuntimeError("the console kept the session after logout")
                checks["logout_cleared_session"] = True

                # A regular ADMIN may deactivate a GENERAL member. Direct generated-SDK calls with forged targets/roles must still be
                # rejected by the server; the visible list keeps the protected rows unchanged.
                stage("admin_login")
                field(page, "이메일").fill(secondary_admin_email)
                field(page, "비밀번호").fill(secondary_admin_password)
                page.get_by_role("button", name="로그인", exact=True).click()
                expect(page).to_have_url(re.compile(r"/$"), timeout=30000)
                settled_idle()
                secondary_state = stored_auth()
                if not secondary_state["is_sign_in"] or secondary_state["role"] != "ADMIN":
                    raise RuntimeError("the second administrator session was not stored as ADMIN")
                checks["admin_login_role"] = secondary_state["role"]
                stage("admin_general_deactivation")
                page.get_by_role("link", name="회원 관리").click()
                expect(page.get_by_role("heading", name="회원 관리")).to_be_visible()
                general_row = page.locator("tr", has_text="d@example.com")
                general_row.get_by_role("button", name="비활성화", exact=True).click()
                page.locator(".confirm-modal__panel").get_by_role("button", name="비활성화", exact=True).click()
                expect(page.get_by_text("회원을 비활성화했습니다.")).to_be_visible()
                expect(general_row.get_by_text("탈퇴", exact=True)).to_be_visible()
                settled_idle()
                checks["admin_deactivated_general"] = True

                stage("admin_permission_denials")
                denied = page.evaluate("""async ({ adminId, superAdminId }) => {
                  const { adminWrites } = await import('/src/api/aipClient.ts');
                  const attempts = [
                    ['admin_target_admin', () => adminWrites.deactivateUser(adminId)],
                    ['admin_target_super_admin', () => adminWrites.deactivateUser(superAdminId)],
                    ['admin_forges_super_admin_role', () => adminWrites.changeUserRole(adminId, 'SUPER_ADMIN')],
                    ['admin_targets_super_admin_role', () => adminWrites.changeUserRole(superAdminId, 'GENERAL')],
                  ];
                  const result = {};
                  for (const [name, call] of attempts) {
                    try { await call(); result[name] = 'UNEXPECTED_SUCCESS'; }
                    catch (error) { result[name] = typeof error?.code === 'string' ? error.code : 'UNKNOWN'; }
                  }
                  return result;
                }""", {"adminId": ADMIN_ID, "superAdminId": SUPER_ADMIN_ID})
                checks["admin_permission_denials"] = denied
                if any(code != "WIRE.FORBIDDEN" for code in denied.values()):
                    raise RuntimeError("an ADMIN changed role or deactivated a protected role through the generated SDK")
                expect(page.locator("tr", has_text="e@example.com").get_by_text("ADMIN", exact=True)).to_be_visible()
                expect(page.locator("tr", has_text="c@example.com").get_by_text("SUPER_ADMIN", exact=True)).to_be_visible()
                page.screenshot(path=str(output / "admin-admin-permission-denials.png"), full_page=True)
                settled_idle()
            except Exception:
                # The page is still alive here; once the exception leaves the Playwright block the driver is gone.
                try:
                    page.screenshot(path=str(output / "admin-failure-state.png"), full_page=True)
                except Exception:
                    pass
                raise
            context.close()
            browser.close()
            browser = None

        uncollected = [row for row in requests if row.get("body_captured") is not True]
        if uncollected:
            raise RuntimeError("an admin API body was not collected (measurement gap): " + json.dumps(uncollected, ensure_ascii=False))
        if rest_fallback:
            raise RuntimeError("the console called the original REST API: " + json.dumps(sorted(set(rest_fallback))))
        unknown = [row for row in requests if row["path"] not in AIP_PATHS]
        if unknown:
            raise RuntimeError("the console called an unexpected AIP path: " + json.dumps(sorted({str(row["path"]) for row in unknown})))
        expected_refusals = {
            ("closed_request_refusal", "WIRE.INVALID_STATE"),
            ("deleted_video_redelete", "WIRE.MISSING_TARGET"),
            ("admin_permission_denials", "WIRE.FORBIDDEN"),
        }
        failed = [row for row in requests if row["path"] in {"/read", "/apply", "/extension", "/jobs/enqueue", "/jobs/status", "/jobs/result"}
                  and row.get("ok") is not True and (row["stage"], row.get("code")) not in expected_refusals]
        if failed:
            raise RuntimeError("an admin SDK request did not succeed: " + json.dumps(failed, ensure_ascii=False))
        mismatched = [row for row in requests if row.get("contract_matches_runtime") is False]
        if mismatched:
            raise RuntimeError("an admin SDK request used another contract fingerprint")
        aggregates = {str(row["aggregate"]) for row in requests if "aggregate" in row}
        for required in ("User.totalCount", "VideoRequest.totalCount", "Video.totalCount", "Comment.totalCount"):
            if required not in aggregates:
                raise RuntimeError(f"the console never asked the server for {required}")
        reads = {str(row["read"]) for row in requests if "read" in row}
        for required in ("User", "VideoRequest", "Video", "Comment"):
            if required not in reads:
                raise RuntimeError(f"the console never read {required}")
        if not any(row.get("operation") == "approveVideoRequest" and row["path"] == "/jobs/enqueue" for row in requests):
            raise RuntimeError("the approval did not use the durable job")
        applied = {str(row.get("apply")) for row in requests if row["path"] == "/apply"}
        for transition in ("VideoRequest.reject", "User.changeRole", "User.deactivate", "Video.remove", "Comment.remove"):
            if transition not in applied:
                raise RuntimeError(f"the console never applied the declared transition {transition}")
        if checks.get("dashboard") in {"rendered", "rendered_partial_host_metrics_unavailable"} and page_errors:
            raise RuntimeError("browser reported an unhandled React page error")
        if checks.get("dashboard") not in {"rendered", "rendered_partial_host_metrics_unavailable"}:
            raise RuntimeError("the admin dashboard did not render")
        report = {
            "ui_revision": UI_REVISION,
            "phase": "admin",
            "checks": checks,
            "api_requests": requests,
            "journey_stage": state["stage"],
            "page_error_count": len(page_errors),
            "page_errors": page_errors,
            "rest_fallback_requests": 0,
            "credentials_recorded": False,
            "browser_api_mocks": False,
        }
        (output / "admin-browser-result.json").write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
        print(json.dumps({"phase": "admin", "journey_stage": state["stage"], "checks": checks}, ensure_ascii=False))
        return 0
    except Exception as error:
        failure_type = safe_browser_failure_type(error)
        report = {
            "ui_revision": UI_REVISION,
            "phase": "admin",
            "checks": checks,
            "api_requests": requests,
            "journey_stage": state["stage"],
            "failure_type": failure_type,
            "failure_detail": (str(error).splitlines() or [""])[0][:300],
            # The script line that raised (an integer), so a failed run says which step it was without a trace of page content.
            "failure_line": next((frame.lineno for frame in reversed(traceback.extract_tb(error.__traceback__)) if frame.filename == __file__), None),
            "page_error_count": len(page_errors),
            "page_errors": page_errors,
            "credentials_recorded": False,
            "browser_api_mocks": False,
        }
        (output / "admin-browser-result.json").write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
        print(json.dumps({"phase": "admin", "journey_stage": state["stage"], "failure_type": failure_type, "detail": report["failure_detail"]}, ensure_ascii=False))
        return 1
    finally:
        if browser is not None:
            # After a failure inside `with sync_playwright()` the driver is already stopped, so closing can raise; the dev server must still go.
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
