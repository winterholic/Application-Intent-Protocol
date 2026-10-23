#!/usr/bin/env python3
"""Verify the pinned React member profile with real SDK reads against AIP."""

import argparse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import re
import signal
import ssl
import subprocess
import sys
import tempfile
import threading
import time
from urllib.parse import urlsplit

from playwright.sync_api import expect, sync_playwright
sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
from product.tests.facereview_ui_browser_evidence import (
    body_failure_reason,
    read_request_evidence,
    safe_browser_failure_type,
    safe_wire_code,
)

PROFILE_NAME = "AIP 프로필 표본회원"
PROFILE_GENRES = {"eating", "horror"}
UPDATED_PROFILE_NAME = "AIP 프로필 수정회원"
UPDATED_PROFILE_GENRES = {"animal", "eating"}
SAFE_GENRES = {
    "drama", "eating", "travel", "cook", "show", "information", "game", "sports",
    "music", "animal", "beauty", "comedy", "horror", "exercise", "vlog", "etc",
}


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--ui", type=Path, required=True)
    parser.add_argument("--api", required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    email = os.environ["AIP_FACEREVIEW_JOURNEY_EMAIL"]
    password = os.environ["AIP_FACEREVIEW_JOURNEY_PASSWORD"]
    expected_contract = os.environ["AIP_FACEREVIEW_EXPECTED_CONTRACT"]
    # CI keeps the default; a local run may move off a port that another dev server already holds.
    ui_port = os.environ.get("AIP_FACEREVIEW_UI_PORT", "5173")
    output = args.out.resolve()
    output.mkdir(parents=True, exist_ok=True)

    server_env = os.environ.copy()
    server_env["VITE_AIP_URL"] = args.api
    server_env.pop("AIP_FACEREVIEW_JOURNEY_EMAIL", None)
    server_env.pop("AIP_FACEREVIEW_JOURNEY_PASSWORD", None)
    server = subprocess.Popen(
        ["node", ".yarn/releases/yarn-4.12.0.cjs", "dev", "--host", "0.0.0.0", "--port", ui_port, "--strictPort"],
        cwd=args.ui,
        env=server_env,
        start_new_session=True,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    requests: list[dict[str, object]] = []
    pending_bodies: dict[object, tuple[object, dict[str, object]]] = {}
    page_errors: list[str] = []
    stage = "startup"
    profile_fields: dict[str, object] = {}
    failure_type = None
    browser = None
    try:
        ready = False
        for _ in range(60):
            if server.poll() is not None:
                raise RuntimeError("Vite exited before becoming ready")
            try:
                import urllib.request

                with urllib.request.urlopen(f"http://127.0.0.1:{ui_port}/auth/1", timeout=1) as response:
                    if response.status == 200:
                        ready = True
                        break
            except Exception:
                time.sleep(1)
        if not ready:
            raise RuntimeError("Vite did not become ready")

        with sync_playwright() as playwright:
            browser = playwright.chromium.launch(headless=True)
            context = browser.new_context(viewport={"width": 1280, "height": 900})
            page = context.new_page()
            page.set_default_timeout(15000)
            page.on("pageerror", lambda error: page_errors.append(type(error).__name__))

            def capture_response(response) -> None:
                parsed = urlsplit(response.url)
                if parsed.port != urlsplit(args.api).port:
                    return
                row: dict[str, object] = {"method": response.request.method, "path": parsed.path, "status": response.status}
                if parsed.path in {"/read", "/apply", "/extension"}:
                    row["contract_matches_runtime"] = response.request.headers.get("x-aip-contract") == expected_contract
                    row["x_aip_contract"] = response.request.headers.get("x-aip-contract")
                if parsed.path == "/read":
                    try:
                        row["read_request"] = read_request_evidence(response.request.post_data_json)
                    except Exception:
                        row["read_request"] = {"read_resource": "unrecognized"}
                requests.append(row)
                pending_bodies[response.request] = (response, row)

            def capture_body(request) -> None:
                # `requestfinished` fires after the body is complete; the response event precedes it and
                # a read there can lose the body to navigation. A body never read leaves no `ok`.
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
                    # JSONDecodeError and UnicodeDecodeError are ValueErrors; everything else is Playwright's.
                    row["body_failure_reason"] = "json_decode_error" if isinstance(error, ValueError) else body_failure_reason(error)
                if isinstance(envelope, dict):
                    if isinstance(envelope.get("ok"), bool):
                        row["ok"] = envelope["ok"]
                    code = safe_wire_code(envelope.get("code"))
                    if code is not None:
                        row["code"] = code
                    if row["path"] == "/read" and isinstance(envelope.get("rows"), list):
                        genres = {
                            item.get("genre") for item in envelope["rows"]
                            if isinstance(item, dict)
                            and isinstance(item.get("genre"), str)
                            and item.get("genre") in SAFE_GENRES
                        }
                        if genres:
                            row["genre_values"] = sorted(genres)

            def wait_for_api_bodies() -> None:
                deadline = time.monotonic() + 15
                while pending_bodies and time.monotonic() < deadline:
                    page.wait_for_timeout(50)
                if pending_bodies:
                    raise AssertionError("API response bodies did not finish before the screenshot")

            page.on("response", capture_response)
            page.on("requestfinished", capture_body)
            stage = "login"
            page.goto(f"http://localhost:{ui_port}/auth/1", wait_until="domcontentloaded")
            page.locator("#authEmail").fill(email)
            page.get_by_role("button", name="다음", exact=True).click()
            expect(page).to_have_url(re.compile(r"/auth/2$"))
            page.locator("#authPassword").fill(password)
            page.get_by_role("button", name="로그인", exact=True).click()
            expect(page).to_have_url(re.compile(r"/main$"), timeout=30000)
            page.get_by_role("button", name="마이페이지로 이동", exact=True).click()
            expect(page).to_have_url(re.compile(r"/my$"), timeout=15000)
            expect(page.get_by_role("heading", name=f"{PROFILE_NAME}님", exact=True)).to_be_visible()
            stage = "member_profile"
            page.locator(".my-page-name-wrapper").get_by_role("button", name="다음", exact=True).click()
            expect(page).to_have_url(re.compile(r"/edit$"), timeout=15000)
            expect(page.get_by_label("닉네임", exact=True)).to_have_value(PROFILE_NAME)
            expect(page.get_by_role("button", name=re.compile("먹방"))).to_have_attribute("aria-pressed", "true")
            expect(page.get_by_role("button", name=re.compile("공포"))).to_have_attribute("aria-pressed", "true")
            expect(page.get_by_role("button", name=re.compile("동물"))).to_have_attribute("aria-pressed", "false")
            selected_count = page.locator(".categories-container button[aria-pressed='true']").count()
            if selected_count != len(PROFILE_GENRES):
                raise AssertionError(f"profile genre selection count was {selected_count}")
            stage = "profile_fields_confirmed"
            profile_fields = {
                "displayed_name": PROFILE_NAME,
                "selected_genres": sorted(PROFILE_GENRES),
                "selected_genre_count": selected_count,
            }
            page.screenshot(path=str(output / "profile-before-save.png"), full_page=True)

            # Exercise the original edit form through the overlay's generated SDK client.
            page.get_by_label("닉네임", exact=True).fill(UPDATED_PROFILE_NAME)
            page.get_by_role("button", name=re.compile("공포")).click()
            page.get_by_role("button", name=re.compile("동물")).click()
            expect(page.get_by_role("button", name=re.compile("공포"))).to_have_attribute("aria-pressed", "false")
            expect(page.get_by_role("button", name=re.compile("동물"))).to_have_attribute("aria-pressed", "true")
            with page.expect_response(
                lambda response: urlsplit(response.url).path == "/apply" and response.request.method == "POST"
            ) as profile_write_response:
                page.get_by_role("button", name="수정", exact=True).click()
            write_response = profile_write_response.value
            # Wait for the write's body to be collected on requestfinished before judging it.
            wait_for_api_bodies()
            try:
                write_envelope = json.loads(write_response.body().decode("utf-8"))
            except Exception:
                write_envelope = {}
            if write_response.status != 200 or not isinstance(write_envelope, dict) or write_envelope.get("ok") is not True:
                raise RuntimeError("React profile update bundle did not succeed against the matching AIP contract")
            if write_response.request.headers.get("x-aip-contract") != expected_contract:
                raise RuntimeError("React profile update did not use the server's AIP contract fingerprint")
            expect(page).to_have_url(re.compile(r"/my$"), timeout=15000)
            expect(page.get_by_role("heading", name=f"{UPDATED_PROFILE_NAME}님", exact=True)).to_be_visible()
            # The destination page issues its own reads; let them finish and render before the screenshot,
            # otherwise it captures only the header while the body is still loading.
            wait_for_api_bodies()
            page.wait_for_load_state("networkidle", timeout=15000)
            wait_for_api_bodies()
            expect(page.get_by_role("heading", name=f"{UPDATED_PROFILE_NAME}님", exact=True)).to_be_visible()
            profile_fields["after_save_visible_text_length"] = page.evaluate("document.body.innerText.length")
            profile_fields["saved_name"] = UPDATED_PROFILE_NAME
            profile_fields["saved_genres"] = sorted(UPDATED_PROFILE_GENRES)
            profile_fields["write_bundle_ok"] = True
            page.screenshot(path=str(output / "profile-after-save.png"), full_page=True)
            context.close()
            browser.close()
            browser = None

        uncollected = [row for row in requests if row.get("body_captured") is not True]
        if uncollected:
            raise RuntimeError("React profile API body was not collected (measurement gap): " + json.dumps(uncollected, ensure_ascii=False))
        reads = [row for row in requests if row.get("path") == "/read"]
        by_resource = {row.get("read_request", {}).get("read_resource"): row for row in reads}
        for resource in ("User", "UserFavoriteGenre"):
            row = by_resource.get(resource)
            if row is None or row.get("status") != 200 or row.get("ok") is not True or row.get("contract_matches_runtime") is not True:
                raise RuntimeError(f"React profile {resource} read did not succeed against the matching AIP contract")
        user_fields = set(by_resource["User"]["read_request"].get("select_fields", []))
        required_user_fields = {"id", "email", "name", "role", "profile_image_id", "is_tutorial_done", "is_verify_email_done", "created_at"}
        if not required_user_fields.issubset(user_fields):
            raise RuntimeError("React profile User read did not request the original profile fields")
        if "genre" not in set(by_resource["UserFavoriteGenre"]["read_request"].get("select_fields", [])):
            raise RuntimeError("React profile UserFavoriteGenre read did not request genre")
        user_filters = by_resource["User"]["read_request"].get("filter_fields", [])
        genre_filters = by_resource["UserFavoriteGenre"]["read_request"].get("filter_fields", [])
        if user_filters != [{"field": "id", "op": "eq", "value_type": "str"}] or genre_filters:
            raise RuntimeError("React profile reads used an unexpected client filter")
        if by_resource["UserFavoriteGenre"].get("genre_values") != sorted(PROFILE_GENRES):
            raise RuntimeError("React profile genre read was not constrained to the signed-in member's rows")
        if page_errors:
            raise RuntimeError("browser reported an unhandled React page error")
        report = {
            "ui_revision": "2e7c021fa5abebd72278d841b8066a33d45cfbb3",
            "phase": "profile",
            "fixture": {"name": PROFILE_NAME, "favorite_genres": sorted(PROFILE_GENRES), "other_member_marker": "vlog"},
            "profile_ui": profile_fields,
            "api_requests": requests,
            "journey_stage": stage,
            "page_error_count": len(page_errors),
            "page_error_types": page_errors,
            "credentials_recorded": False,
            "browser_api_mocks": False,
        }
        (output / "profile-browser-result.json").write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
        print(json.dumps({"phase": "profile", "journey_stage": stage, "profile_ui": profile_fields}, ensure_ascii=False))
        return 0
    except Exception as error:
        failure_type = safe_browser_failure_type(error)
        report = {
            "ui_revision": "2e7c021fa5abebd72278d841b8066a33d45cfbb3",
            "phase": "profile",
            "fixture": {"name": PROFILE_NAME, "favorite_genres": sorted(PROFILE_GENRES), "other_member_marker": "vlog"},
            "profile_ui": profile_fields,
            "api_requests": requests,
            "journey_stage": stage,
            "failure_type": failure_type,
            "page_error_count": len(page_errors),
            "page_error_types": page_errors,
            "credentials_recorded": False,
            "browser_api_mocks": False,
        }
        (output / "profile-browser-result.json").write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
        print(json.dumps({"phase": "profile", "journey_stage": stage, "failure_type": failure_type}, ensure_ascii=False))
        return 1
    finally:
        if browser is not None:
            browser.close()
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
