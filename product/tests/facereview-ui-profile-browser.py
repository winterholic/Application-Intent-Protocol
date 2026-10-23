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
import tempfile
import threading
import time
from urllib.parse import urlsplit

from playwright.sync_api import expect, sync_playwright

from product.tests.facereview_ui_browser_evidence import (
    read_request_evidence,
    safe_browser_failure_type,
    safe_wire_code,
)

PROFILE_NAME = "AIP 프로필 표본회원"
PROFILE_GENRES = {"eating", "horror"}


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--ui", type=Path, required=True)
    parser.add_argument("--api", required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    email = os.environ["AIP_FACEREVIEW_JOURNEY_EMAIL"]
    password = os.environ["AIP_FACEREVIEW_JOURNEY_PASSWORD"]
    expected_contract = os.environ["AIP_FACEREVIEW_EXPECTED_CONTRACT"]
    output = args.out.resolve()
    output.mkdir(parents=True, exist_ok=True)

    server_env = os.environ.copy()
    server_env["VITE_AIP_URL"] = args.api
    server_env.pop("AIP_FACEREVIEW_JOURNEY_EMAIL", None)
    server_env.pop("AIP_FACEREVIEW_JOURNEY_PASSWORD", None)
    server = subprocess.Popen(
        ["node", ".yarn/releases/yarn-4.12.0.cjs", "dev", "--host", "0.0.0.0", "--port", "5173", "--strictPort"],
        cwd=args.ui,
        env=server_env,
        start_new_session=True,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    requests: list[dict[str, object]] = []
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

                with urllib.request.urlopen("http://127.0.0.1:5173/auth/1", timeout=1) as response:
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
                try:
                    envelope = response.json()
                except Exception:
                    envelope = {}
                if isinstance(envelope, dict):
                    if isinstance(envelope.get("ok"), bool):
                        row["ok"] = envelope["ok"]
                    code = safe_wire_code(envelope.get("code"))
                    if code is not None:
                        row["code"] = code
                if parsed.path == "/read":
                    try:
                        row["read_request"] = read_request_evidence(response.request.post_data_json)
                    except Exception:
                        row["read_request"] = {"read_resource": "unrecognized"}
                requests.append(row)

            page.on("response", capture_response)
            stage = "login"
            page.goto("http://localhost:5173/auth/1", wait_until="domcontentloaded")
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
            page.screenshot(path=str(output / "profile-screen.png"), full_page=True)
            context.close()
            browser.close()
            browser = None

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
        if user_filters != [{"field": "id", "op": "eq", "value_type": "str"}] or genre_filters != [
            {"field": "user", "op": "eq", "value_type": "str"}
        ]:
            raise RuntimeError("React profile reads were not scoped to the signed-in member")
        if page_errors:
            raise RuntimeError("browser reported an unhandled React page error")
        report = {
            "ui_revision": "2e7c021fa5abebd72278d841b8066a33d45cfbb3",
            "phase": "profile",
            "fixture": {"name": PROFILE_NAME, "favorite_genres": sorted(PROFILE_GENRES)},
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
            "fixture": {"name": PROFILE_NAME, "favorite_genres": sorted(PROFILE_GENRES)},
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
