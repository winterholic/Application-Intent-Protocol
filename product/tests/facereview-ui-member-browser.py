#!/usr/bin/env python3
"""Exercise the pinned FaceReview React screens against a live AIP service."""

import argparse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
import re
from pathlib import Path
import subprocess
import ssl
import sys
import tempfile
import threading
import time
import traceback
from urllib.parse import urlsplit

from playwright.sync_api import expect, sync_playwright
sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
from product.tests.facereview_ui_browser_evidence import (
    extension_request_evidence,
    extension_response_evidence,
    fixture_thumbnail_redirect_url,
)


VIDEO_ID = "aaaaaaaa-0000-4000-8000-000000000001"
VIDEO_TITLE = "고양이 영상"
VIDEO_YOUTUBE_ID = "dQw4w9WgXcQ"


class FixtureThumbnailHandler(BaseHTTPRequestHandler):
    def do_GET(self) -> None:
        if self.path != "/fixture-thumbnail.svg":
            self.send_error(404)
            return
        body = (
            b'<svg xmlns="http://www.w3.org/2000/svg" width="320" height="180" '
            b'viewBox="0 0 320 180"><rect width="320" height="180" fill="#303846"/>'
            b'<circle cx="160" cy="90" r="34" fill="#69d3b0"/></svg>'
        )
        self.send_response(200)
        self.send_header("Content-Type", "image/svg+xml")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, _format: str, *_args: object) -> None:
        return


def start_fixture_thumbnail_server() -> tuple[
    ThreadingHTTPServer, threading.Thread, tempfile.TemporaryDirectory, str
]:
    cert_dir = tempfile.TemporaryDirectory(prefix="aip-face-thumbnail-")
    certificate = Path(cert_dir.name) / "fixture-cert.pem"
    private_key = Path(cert_dir.name) / "fixture-key.pem"
    subprocess.run(
        [
            "openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1",
            "-subj", "/CN=127.0.0.1", "-addext", "subjectAltName=IP:127.0.0.1",
            "-keyout", str(private_key), "-out", str(certificate),
        ],
        check=True,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    server = ThreadingHTTPServer(("127.0.0.1", 0), FixtureThumbnailHandler)
    tls = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    tls.load_cert_chain(certificate, private_key)
    server.socket = tls.wrap_socket(server.socket, server_side=True)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    url = f"https://127.0.0.1:{server.server_address[1]}/fixture-thumbnail.svg"
    return server, thread, cert_dir, url


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
    adapter_list_evidence: list[dict[str, object]] = []
    page_errors: list[str] = []
    console_error_count = 0
    blocked_external_image_count = 0
    fixture_thumbnail_redirect_count = 0
    fixture_thumbnail_response_statuses: list[int] = []
    rendered_state: dict[str, int] = {}
    screenshot_names: list[str] = []
    thumbnail_server = None
    thumbnail_thread = None
    thumbnail_cert_dir = None
    thumbnail_url = ""
    try:
        thumbnail_server, thumbnail_thread, thumbnail_cert_dir, thumbnail_url = start_fixture_thumbnail_server()
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
                # This self-signed endpoint serves only a local static image. Other
                # external media stays blocked and AIP API requests remain untouched.
                context = browser.new_context(
                    ignore_https_errors=True, viewport={"width": 1280, "height": 900}
                )
                page = context.new_page()
                page.set_default_timeout(15000)
                page.add_init_script(
                    "window.__AIP_SAFE_LIST_FIXTURE_ID__ = " + json.dumps(VIDEO_ID) + ";"
                    "window.__AIP_SAFE_LIST_EVIDENCE__ = (event) => "
                    "console.info('AIP_SAFE_LIST_EVIDENCE:' + JSON.stringify(event));"
                )

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
                        if parsed.path == "/extension":
                            try:
                                extension_request = extension_request_evidence(response.request.post_data_json)
                            except Exception:
                                extension_request = {}
                            if extension_request:
                                row["extension_request"] = extension_request
                                row.update(extension_response_evidence(
                                    str(extension_request["extension"]), envelope, VIDEO_ID
                                ))
                        api_paths.append(row)

                page.on("response", capture_response)
                page.on(
                    "response",
                    lambda response: fixture_thumbnail_response_statuses.append(response.status)
                    if response.url == thumbnail_url
                    else None,
                )
                page.on("pageerror", lambda error: page_errors.append(type(error).__name__))

                def capture_console_error(message) -> None:
                    nonlocal console_error_count
                    text = message.text
                    prefix = "AIP_SAFE_LIST_EVIDENCE:"
                    if text.startswith(prefix):
                        try:
                            event = json.loads(text[len(prefix):])
                        except (TypeError, ValueError):
                            return
                        allowed_extensions = {"Video.homeVideoAll", "Video.homeCategory", "Video.homePersonalized"}
                        if (isinstance(event, dict)
                                and event.get("extension") in allowed_extensions
                                and isinstance(event.get("adapter_list_length"), int)
                                and isinstance(event.get("contains_fixture_video"), bool)):
                            adapter_list_evidence.append({
                                "extension": event["extension"],
                                "adapter_list_length": event["adapter_list_length"],
                                "contains_fixture_video": event["contains_fixture_video"],
                            })
                    elif message.type == "error":
                        console_error_count += 1

                page.on("console", capture_console_error)

                # The fixture's YouTube ID is not fetched or played. Its availability
                # thumbnail request is continued to a local static fixture image.
                def block_external_media(route) -> None:
                    nonlocal blocked_external_image_count, fixture_thumbnail_redirect_count
                    local_thumbnail = fixture_thumbnail_redirect_url(
                        route.request.url,
                        route.request.resource_type,
                        VIDEO_YOUTUBE_ID,
                        thumbnail_url,
                    )
                    if local_thumbnail is not None:
                        fixture_thumbnail_redirect_count += 1
                        route.continue_(url=local_thumbnail)
                        return
                    host = urlsplit(route.request.url).hostname or ""
                    if host.endswith(("youtube.com", "youtube-nocookie.com", "ytimg.com", "googlevideo.com")):
                        if route.request.resource_type == "image":
                            blocked_external_image_count += 1
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
                    page.wait_for_load_state("networkidle", timeout=10000)

                    def capture_render_state() -> None:
                        nonlocal rendered_state
                        rendered_state = page.evaluate("""(fixtureTitle) => {
                          const cards = [...document.querySelectorAll('.video-item-container')];
                          const countIn = (selector) => document.querySelectorAll(`${selector} .video-item-container`).length;
                          return {
                            rendered_card_count: cards.length,
                            fixture_card_count: cards.filter((card) =>
                              [...card.querySelectorAll('.video-title')].some((title) => title.textContent?.trim() === fixtureTitle)
                            ).length,
                            personalized_card_count: countIn('.personal-recommend-contents-container'),
                            genre_card_count: countIn('.genre-contents-container'),
                            popular_card_count: countIn('.hot-contents-container'),
                          };
                        }""", VIDEO_TITLE)

                    capture_render_state()
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
                    try:
                        rendered_state = page.evaluate("""(fixtureTitle) => {
                          const cards = [...document.querySelectorAll('.video-item-container')];
                          const countIn = (selector) => document.querySelectorAll(`${selector} .video-item-container`).length;
                          return {
                            rendered_card_count: cards.length,
                            fixture_card_count: cards.filter((card) =>
                              [...card.querySelectorAll('.video-title')].some((title) => title.textContent?.trim() === fixtureTitle)
                            ).length,
                            personalized_card_count: countIn('.personal-recommend-contents-container'),
                            genre_card_count: countIn('.genre-contents-container'),
                            popular_card_count: countIn('.hot-contents-container'),
                          };
                        }""", VIDEO_TITLE)
                    except Exception:
                        rendered_state = {}
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
        if fixture_thumbnail_redirect_count < 1 or 200 not in fixture_thumbnail_response_statuses:
            raise RuntimeError("the local fixture thumbnail did not load successfully")

        report = {
            "ui_revision": "2e7c021fa5abebd72278d841b8066a33d45cfbb3",
            "phase": args.phase,
            "fixture_video": {"id": VIDEO_ID, "title": VIDEO_TITLE},
            "server_listening_fingerprint": expected_contract,
            "ui_binding_fingerprint": binding_contract,
            "api_requests": api_paths,
            "adapter_list_evidence": adapter_list_evidence,
            "rendered_state": rendered_state,
            "page_error_count": len(page_errors),
            "page_error_types": page_errors,
            "console_error_count": console_error_count,
            "blocked_external_image_count": blocked_external_image_count,
            "fixture_thumbnail_redirect_count": fixture_thumbnail_redirect_count,
            "fixture_thumbnail_response_statuses": fixture_thumbnail_response_statuses,
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
            "adapter_list_evidence": adapter_list_evidence,
            "rendered_state": rendered_state,
            "fixture_video": {"id": VIDEO_ID},
            "page_error_count": len(page_errors),
            "page_error_types": page_errors,
            "console_error_count": console_error_count,
            "blocked_external_image_count": blocked_external_image_count,
            "fixture_thumbnail_redirect_count": fixture_thumbnail_redirect_count,
            "fixture_thumbnail_response_statuses": fixture_thumbnail_response_statuses,
            "screenshots": screenshot_names,
            "credentials_recorded": False,
            "browser_api_mocks": False,
        }
        (output / f"{args.phase}-browser-result.json").write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
        safe_trace = traceback.format_exc().replace(email, "[fixture-email]").replace(password, "[fixture-password]")
        print(safe_trace, file=os.sys.stderr, end="")
        return 1
    finally:
        if thumbnail_server is not None:
            thumbnail_server.shutdown()
            thumbnail_server.server_close()
        if thumbnail_thread is not None:
            thumbnail_thread.join(timeout=2)
        if thumbnail_cert_dir is not None:
            thumbnail_cert_dir.cleanup()
        server.terminate()
        try:
            server.wait(timeout=10)
        except subprocess.TimeoutExpired:
            server.kill()
            server.wait(timeout=10)


if __name__ == "__main__":
    raise SystemExit(main())
