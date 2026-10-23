#!/usr/bin/env python3
"""Exercise the pinned FaceReview React screens against a live AIP service."""

import argparse
import json
import os
import re
from pathlib import Path
import subprocess
import time
import traceback
from urllib.parse import urlsplit

from playwright.sync_api import expect, sync_playwright


VIDEO_ID = "aaaaaaaa-0000-4000-8000-000000000001"
VIDEO_TITLE = "고양이 영상"


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--ui", type=Path, required=True)
    parser.add_argument("--api", required=True)
    parser.add_argument("--phase", choices=("add", "remove"), required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    email = os.environ["AIP_FACEREVIEW_JOURNEY_EMAIL"]
    password = os.environ["AIP_FACEREVIEW_JOURNEY_PASSWORD"]
    expected_contract = os.environ["AIP_FACEREVIEW_EXPECTED_CONTRACT"]
    binding_source = (args.ui / "src/aip/facereview.ts").read_text(encoding="utf-8")
    binding_match = re.search(r'export const contractFingerprint = "([a-f0-9]{64})";', binding_source)
    if binding_match is None:
        raise RuntimeError("generated UI binding has no readable contract fingerprint")
    binding_contract = binding_match.group(1)
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
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    api_paths: list[dict[str, object]] = []
    page_errors: list[str] = []
    screenshot_names: list[str] = []
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
            try:
                page = browser.new_page(viewport={"width": 1280, "height": 900})
                page.set_default_timeout(15000)

                def capture_response(response) -> None:
                    parsed = urlsplit(response.url)
                    if parsed.port == urlsplit(args.api).port:
                        request_contract = response.request.headers.get("x-aip-contract")
                        try:
                            envelope = response.json()
                        except Exception:
                            envelope = {}
                        row: dict[str, object] = {
                            "method": response.request.method,
                            "path": parsed.path,
                            "status": response.status,
                        }
                        if parsed.path in {"/read", "/extension", "/apply"}:
                            row["contract_matches_runtime"] = request_contract == expected_contract
                            row["x_aip_contract"] = request_contract
                        if isinstance(envelope, dict):
                            for key in ("ok", "code"):
                                if key in envelope and isinstance(envelope[key], (bool, str)):
                                    row[key] = envelope[key]
                        api_paths.append(row)

                page.on("response", capture_response)
                page.on("pageerror", lambda _error: page_errors.append("unhandled-page-error"))

                # The fixture's YouTube ID is not fetched or played; AIP API traffic remains untouched.
                def block_external_media(route) -> None:
                    host = urlsplit(route.request.url).hostname or ""
                    if host.endswith(("youtube.com", "youtube-nocookie.com", "ytimg.com", "googlevideo.com")):
                        route.abort()
                    else:
                        route.continue_()

                page.route("**/*", block_external_media)
                try:
                    page.goto("http://localhost:5173/auth/1", wait_until="domcontentloaded")
                    page.locator("#authEmail").fill(email)
                    page.get_by_role("button", name="다음", exact=True).click()
                    expect(page).to_have_url(re.compile(r"/auth/2$"))
                    page.locator("#authPassword").fill(password)
                    page.get_by_role("button", name="로그인", exact=True).click()
                    expect(page).to_have_url(re.compile(r"/main$"), timeout=30000)
                    video_card = page.locator(".video-item-container").filter(has_text=VIDEO_TITLE).first
                    expect(video_card).to_be_visible(timeout=30000)
                    screenshot = output / f"{args.phase}-member-video-list.png"
                    page.screenshot(path=str(screenshot), full_page=True)
                    screenshot_names.append(screenshot.name)
                    video_card.click()
                    expect(page).to_have_url(re.compile(rf"/watch/{re.escape(VIDEO_ID)}$"))
                    expect(page.get_by_text(VIDEO_TITLE, exact=True).first).to_be_visible(timeout=30000)

                    bookmark_add = page.get_by_role("button", name="즐겨찾기 추가", exact=True)
                    bookmark_remove = page.get_by_role("button", name="즐겨찾기 해제", exact=True)
                    if args.phase == "add":
                        expect(bookmark_add).to_be_visible()
                        bookmark_add.click()
                        expect(bookmark_remove).to_have_attribute("aria-pressed", "true")
                        page.reload(wait_until="domcontentloaded")
                        expect(bookmark_remove).to_have_attribute("aria-pressed", "true", timeout=30000)
                        label = "bookmarked"
                    else:
                        expect(bookmark_remove).to_be_visible()
                        bookmark_remove.click()
                        expect(bookmark_add).to_have_attribute("aria-pressed", "false")
                        page.reload(wait_until="domcontentloaded")
                        expect(bookmark_add).to_have_attribute("aria-pressed", "false", timeout=30000)
                        label = "bookmark-removed"

                    screenshot = output / f"{args.phase}-{label}.png"
                    page.screenshot(path=str(screenshot), full_page=True)
                    screenshot_names.append(screenshot.name)
                except Exception:
                    # Keep a safe view of the last UI state even when this phase fails.
                    email_field = page.locator("#authEmail")
                    if email_field.count():
                        email_field.evaluate("element => { element.value = ''; }")
                    screenshot = output / f"{args.phase}-failure.png"
                    page.screenshot(path=str(screenshot), full_page=True)
                    screenshot_names.append(screenshot.name)
                    raise
            finally:
                browser.close()

        observed_paths = {row["path"] for row in api_paths}
        required_paths = {"/auth/check-email", "/auth/login", "/extension", "/apply"}
        if not required_paths.issubset(observed_paths):
            raise RuntimeError("browser did not issue the expected real AIP API requests")
        if any(row.get("x_aip_contract") != binding_contract for row in api_paths if row["path"] in {"/read", "/extension", "/apply"}):
            raise RuntimeError("the real browser request contract header differs from the generated UI binding")
        if any(row["path"] in required_paths and row["status"] != 200 for row in api_paths):
            raise RuntimeError("an expected AIP API request did not return HTTP 200")
        if page_errors:
            raise RuntimeError("browser reported an unhandled React page error")

        report = {
            "ui_revision": "2e7c021fa5abebd72278d841b8066a33d45cfbb3",
            "phase": args.phase,
            "fixture_video": {"id": VIDEO_ID, "title": VIDEO_TITLE},
            "server_listening_fingerprint": expected_contract,
            "ui_binding_fingerprint": binding_contract,
            "api_requests": api_paths,
            "screenshots": screenshot_names,
            "external_video_provider": "blocked; not fetched or played",
            "credentials_recorded": False,
            "browser_api_mocks": False,
        }
        (output / f"{args.phase}-browser-result.json").write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
        print(json.dumps({"phase": args.phase, "api_paths": sorted(observed_paths), "screenshots": screenshot_names}, ensure_ascii=False))
        return 0
    except Exception:
        # Preserve safe request metadata even on failure; never serialize response bodies or credentials.
        report = {
            "ui_revision": "2e7c021fa5abebd72278d841b8066a33d45cfbb3",
            "phase": args.phase,
            "server_listening_fingerprint": expected_contract,
            "ui_binding_fingerprint": binding_contract,
            "api_requests": api_paths,
            "screenshots": screenshot_names,
            "credentials_recorded": False,
            "browser_api_mocks": False,
        }
        (output / f"{args.phase}-browser-result.json").write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
        safe_trace = traceback.format_exc().replace(email, "[fixture-email]").replace(password, "[fixture-password]")
        print(safe_trace, file=os.sys.stderr, end="")
        return 1
    finally:
        server.terminate()
        try:
            server.wait(timeout=10)
        except subprocess.TimeoutExpired:
            server.kill()
            server.wait(timeout=10)


if __name__ == "__main__":
    raise SystemExit(main())
