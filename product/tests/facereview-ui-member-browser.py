#!/usr/bin/env python3
"""Exercise the pinned FaceReview React screens against a live AIP service."""

import argparse
import hashlib
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
    app_bookmark_read_observation,
    body_capture_evidence,
    body_failure_reason,
    SAFE_NETWORK_FAILURES,
    compare_bookmark_read_capture,
    timeline_phase,
    extension_request_evidence,
    extension_response_evidence,
    external_media_failure_evidence,
    failed_read_requests,
    fixture_thumbnail_redirect_url,
    youtube_iframe_api_stub,
    response_envelope_evidence,
    read_request_evidence,
    safe_browser_caller_frames,
    safe_browser_failure_type,
    safe_browser_route_class,
    safe_bookmark_control_evidence,
    safe_wire_code,
    console_error_evidence,
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


# Observes what the React app itself receives for the fixture bookmark /read. It passes the original
# fetch result through untouched and emits only length, hash and allowlisted shape on the console.
APP_BOOKMARK_READ_OBSERVER = """
(() => {
  const originalFetch = window.fetch;
  let seq = 0;
  const emit = (event) => { try { console.info('AIP_SAFE_BOOKMARK_READ:' + JSON.stringify(event)); } catch (_) {} };
  const hex = (buffer) => [...new Uint8Array(buffer)].map((byte) => byte.toString(16).padStart(2, '0')).join('');
  const isBookmarkRead = (input, init) => {
    try {
      const url = typeof input === 'string' ? input : input instanceof URL ? input.href : input.url;
      if (new URL(url, location.href).pathname !== '/read') return false;
      const body = init && typeof init.body === 'string' ? init.body : null;
      return body !== null && JSON.parse(body)?.query?.read === 'VideoBookmark';
    } catch (_) { return false; }
  };
  window.fetch = function (input, init) {
    if (!isBookmarkRead(input, init)) return originalFetch.apply(this, arguments);
    const id = ++seq;
    const startedMs = Date.now();
    return originalFetch.apply(this, arguments).then((response) => {
      const responseMs = Date.now();
      response.clone().arrayBuffer().then(async (buffer) => {
        let parse = 'other';
        let ok;
        let rowsLength;
        try {
          const payload = JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(buffer));
          parse = Array.isArray(payload) ? 'array' : payload && typeof payload === 'object' ? 'object' : 'other';
          if (parse === 'object') {
            if (typeof payload.ok === 'boolean') ok = payload.ok;
            if (Array.isArray(payload.rows)) rowsLength = payload.rows.length;
          }
        } catch (_) { parse = 'json_decode_error'; }
        let digest;
        try { digest = hex(await crypto.subtle.digest('SHA-256', buffer)).slice(0, 16); } catch (_) {}
        emit({ seq: id, phase: 'received', started_ms: startedMs, response_ms: responseMs, ended_ms: Date.now(),
          status: response.status, content_type: response.headers.get('content-type'), byte_length: buffer.byteLength,
          sha256_16: digest, parse, ok, rows_length: rowsLength });
      }, (error) => emit({ seq: id, phase: 'clone_read_failed', started_ms: startedMs, response_ms: responseMs,
        ended_ms: Date.now(), error_class: error && error.name }));
      return response;
    }, (error) => {
      emit({ seq: id, phase: 'fetch_rejected', started_ms: startedMs, ended_ms: Date.now(), error_class: error && error.name });
      throw error;
    });
  };
})();
"""


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
    # CI keeps the default; a local run may move off a port that another dev server already holds.
    ui_port = os.environ.get("AIP_FACEREVIEW_UI_PORT", "5173")
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
        ["node", ".yarn/releases/yarn-4.12.0.cjs", "dev", "--host", "0.0.0.0", "--port", ui_port, "--strictPort"],
        cwd=args.ui,
        env=server_env,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    clock_start = time.monotonic()
    epoch_start_ms = round(time.time() * 1000)

    def now_ms() -> int:
        return round((time.monotonic() - clock_start) * 1000)

    timeline: list[dict[str, object]] = []
    marks: dict[str, int] = {}

    def mark(event: str) -> None:
        ms = now_ms()
        marks.setdefault(event, ms)
        timeline.append({"event": event, "ms": ms})

    lifecycle: dict[object, dict[str, object]] = {}
    pending_bodies: dict[object, dict[str, object]] = {}
    api_request_failures: list[dict[str, object]] = []
    app_unpaired_count = 0
    app_bookmark_observations: list[dict[str, object]] = []
    api_paths: list[dict[str, object]] = []
    read_requests: list[dict[str, object]] = []
    read_callers_by_body: dict[bytes, list[dict[str, object]]] = {}
    adapter_list_evidence: list[dict[str, object]] = []
    page_errors: list[str] = []
    console_error_count = 0
    console_errors: list[dict[str, object]] = []
    external_media_failures: list[dict[str, str]] = []
    blocked_external_image_count = 0
    youtube_player_api_fixture_count = 0
    fixture_thumbnail_redirect_count = 0
    fixture_thumbnail_response_statuses: list[int] = []
    rendered_state: dict[str, int] = {}
    journey_stage = "startup"
    failure_type: str | None = None
    bookmark_ui_state: dict[str, object] = {}
    failure_route_class = "unrecognized"
    failure_bookmark_controls: list[dict[str, str]] = []
    failure_watch_content_visible = False
    screenshot_names: list[str] = []
    thumbnail_server = None
    thumbnail_thread = None
    thumbnail_cert_dir = None
    thumbnail_url = ""
    api_port = urlsplit(args.api).port

    def track_api_request(request) -> None:
        if urlsplit(request.url).port != api_port:
            return
        path = urlsplit(request.url).path
        read_resource = None
        if path == "/read":
            try:
                read_resource = read_request_evidence(request.post_data_json).get("read_resource")
            except Exception:
                read_resource = "unrecognized"
        lifecycle[request] = {
            "corr": len(lifecycle) + 1,
            "method": request.method,
            "path": path,
            "read_resource": read_resource,
            "t_request_ms": now_ms(),
        }

    def unfinished_requests() -> list[dict[str, object]]:
        return [
            item for item in lifecycle.values()
            if "t_finished_ms" not in item and "t_failed_ms" not in item
        ]

    def lifecycle_summary() -> dict[str, object]:
        return {
            "requests": len(lifecycle),
            "responses": sum(1 for item in lifecycle.values() if "t_response_ms" in item),
            "finished": sum(1 for item in lifecycle.values() if "t_finished_ms" in item),
            "failed": sum(1 for item in lifecycle.values() if "t_failed_ms" in item),
            "rows": len(api_paths),
            "unfinished": [
                {
                    "corr": item["corr"],
                    "path": item["path"],
                    "read_resource": item["read_resource"],
                    "t_request_ms": item["t_request_ms"],
                }
                for item in unfinished_requests()
            ][:10],
        }

    def apply_body_evidence(entry: dict[str, object], evidence: dict[str, object], raw: bytes | None) -> None:
        # Body-derived fields are filled only once the body outcome is known; a body that was
        # never read leaves no `ok`, so it can never count as a successful response.
        row = entry["row"]
        envelope: object = {}
        if raw is not None:
            try:
                envelope = json.loads(raw.decode("utf-8"))
            except (UnicodeDecodeError, ValueError):
                envelope = {}
        row["body_captured"] = evidence["body_captured"]
        if evidence["body_captured"] is not True:
            row["body_failure_reason"] = evidence["failure_reason"]
        if isinstance(envelope, dict):
            if isinstance(envelope.get("ok"), bool):
                row["ok"] = envelope["ok"]
            code = safe_wire_code(envelope.get("code"))
            if code is not None:
                row["code"] = code
        if row["path"] == "/read":
            row["response_shape"] = evidence["response_shape"]
            read_result = entry["read_result"]
            if isinstance(row.get("ok"), bool):
                read_result["ok"] = row["ok"]
            if isinstance(row.get("code"), str):
                read_result["code"] = row["code"]
            read_result["response_shape"] = evidence["response_shape"]
        extension_request = entry["extension_request"]
        if extension_request:
            row.update(extension_response_evidence(str(extension_request["extension"]), envelope, VIDEO_ID))
        if entry["bookmark_read"]:
            row["bookmark_read_observation"] = {
                "corr": row["corr"],
                "content_type": evidence["content_type"],
                "body_capture": {key: value for key, value in evidence.items() if key != "response_shape"},
            }

    def capture_response(response) -> None:
        parsed = urlsplit(response.url)
        if parsed.port != api_port:
            return
        life = lifecycle.get(response.request, {})
        life["t_response_ms"] = now_ms()
        request_contract = response.request.headers.get("x-aip-contract")
        row: dict[str, object] = {
            "corr": life.get("corr"),
            "method": response.request.method,
            "path": parsed.path,
            "status": response.status,
        }
        if parsed.path in {"/read", "/extension", "/apply"}:
            row["contract_matches_runtime"] = request_contract == expected_contract
            row["x_aip_contract"] = request_contract
        read_result: dict[str, object] | None = None
        bookmark_read = False
        if parsed.path == "/read":
            try:
                read_evidence = read_request_evidence(response.request.post_data_json)
            except Exception:
                read_evidence = {"read_resource": "unrecognized"}
            bookmark_read = read_evidence.get("read_resource") == "VideoBookmark"
            row["read_request"] = read_evidence
            post_data = response.request.post_data
            if isinstance(post_data, str):
                digest = hashlib.sha256(post_data.encode("utf-8")).digest()
                queued = read_callers_by_body.get(digest, [])
                if queued:
                    row["caller_frames"] = queued.pop(0)
                    if not queued:
                        del read_callers_by_body[digest]
            else:
                row["caller_frames"] = []
            read_result = dict(read_evidence)
            read_result["status"] = response.status
            read_requests.append(read_result)
        extension_request: dict[str, object] = {}
        if parsed.path == "/extension":
            try:
                extension_request = extension_request_evidence(response.request.post_data_json)
            except Exception:
                extension_request = {}
            if extension_request:
                row["extension_request"] = extension_request
        api_paths.append(row)
        pending_bodies[response.request] = {
            "response": response,
            "row": row,
            "read_result": read_result,
            "extension_request": extension_request,
            "bookmark_read": bookmark_read,
        }

    def capture_body(request) -> None:
        # `requestfinished` fires after the body is complete, so reading it here does not
        # depend on the response event that precedes the body.
        life = lifecycle.get(request)
        if life is not None:
            life["t_finished_ms"] = now_ms()
        entry = pending_bodies.pop(request, None)
        if entry is None:
            return
        response = entry["response"]
        raw: bytes | None = None
        reason: str | None = None
        if life is not None:
            life["t_body_attempt_ms"] = now_ms()
        try:
            raw = response.body()
        except Exception as error:
            reason = body_failure_reason(error)
        if life is not None:
            life["t_body_done_ms"] = now_ms()
        apply_body_evidence(
            entry, body_capture_evidence(raw, response.headers.get("content-type"), reason), raw
        )

    def capture_request_failure(request) -> None:
        life = lifecycle.get(request)
        if life is None:
            return
        failure = request.failure
        reason = failure if isinstance(failure, str) and failure in SAFE_NETWORK_FAILURES else "other"
        life["t_failed_ms"] = now_ms()
        life["failure"] = reason
        entry = pending_bodies.pop(request, None)
        if entry is not None:
            apply_body_evidence(
                entry, body_capture_evidence(None, None, "request_failed_after_response"), None
            )
            return
        # No response ever arrived: keep it visible instead of silently dropping it.
        try:
            failed_resource = read_request_evidence(request.post_data_json).get("read_resource")
        except Exception:
            failed_resource = "unrecognized"
        api_request_failures.append({
            "corr": life["corr"],
            "path": life["path"],
            "failure": reason,
            "read_resource": failed_resource if life["path"] == "/read" else None,
            "t_request_ms": life["t_request_ms"],
            "t_failed_ms": life["t_failed_ms"],
        })

    def finalize_lifecycle() -> None:
        nonlocal app_unpaired_count
        for request, entry in list(pending_bodies.items()):
            apply_body_evidence(
                entry, body_capture_evidence(None, None, "not_finished_before_report"), None
            )
            del pending_bodies[request]
        bookmark_rows = sorted(
            (row for row in api_paths if "bookmark_read_observation" in row), key=lambda row: row["corr"]
        )
        app_observations = sorted(app_bookmark_observations, key=lambda event: event["seq"])
        for index, row in enumerate(bookmark_rows):
            observation = row["bookmark_read_observation"]
            life = next((item for item in lifecycle.values() if item["corr"] == row["corr"]), {})
            for key in ("t_request_ms", "t_response_ms", "t_finished_ms", "t_body_attempt_ms", "t_body_done_ms"):
                observation[key] = life.get(key)
            observation["body_attempt_phase"] = timeline_phase(life.get("t_body_attempt_ms"), marks)
            observation["response_handler_phase"] = timeline_phase(life.get("t_response_ms"), marks)
            app = app_observations[index] if index < len(app_observations) else None
            observation["app_observation"] = app
            observation["app_comparison"] = compare_bookmark_read_capture(observation["body_capture"], app)
        # The one-to-one pairing is by request order; extra app events mean the pairing is unreliable.
        app_unpaired_count = max(0, len(app_observations) - len(bookmark_rows))


    try:
        thumbnail_server, thumbnail_thread, thumbnail_cert_dir, thumbnail_url = start_fixture_thumbnail_server()
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

                page.add_init_script(APP_BOOKMARK_READ_OBSERVER)

                cdp = context.new_cdp_session(page)
                cdp.send("Network.enable")

                def capture_network_initiator(event: dict[str, object]) -> None:
                    request = event.get("request")
                    if not isinstance(request, dict) or request.get("method") != "POST":
                        return
                    request_url = request.get("url")
                    if not isinstance(request_url, str):
                        return
                    parsed_request_url = urlsplit(request_url)
                    if parsed_request_url.port != urlsplit(args.api).port or parsed_request_url.path != "/read":
                        return
                    post_data = request.get("postData")
                    initiator = event.get("initiator")
                    stack = initiator.get("stack") if isinstance(initiator, dict) else None
                    frames: list[dict[str, object]] = []
                    while isinstance(stack, dict) and len(frames) < 12:
                        call_frames = stack.get("callFrames")
                        if isinstance(call_frames, list):
                            frames.extend(frame for frame in call_frames if isinstance(frame, dict))
                        stack = stack.get("parent")
                    safe_frames = safe_browser_caller_frames(frames)
                    if isinstance(post_data, str):
                        digest = hashlib.sha256(post_data.encode("utf-8")).digest()
                        read_callers_by_body.setdefault(digest, []).append(safe_frames)

                mark("journey_start")
                page.on("framenavigated", lambda frame: (
                    timeline.append({
                        "event": "main_frame_navigated",
                        "ms": now_ms(),
                        "route_class": safe_browser_route_class(frame.url, VIDEO_ID),
                    })
                    if frame.parent_frame is None and len(timeline) < 200
                    else None
                ))
                page.on("close", lambda _page: mark("page_closed"))
                page.on("requestfailed", capture_request_failure)
                page.on("request", track_api_request)
                page.on("response", capture_response)
                page.on("requestfinished", capture_body)
                cdp.on("Network.requestWillBeSent", capture_network_initiator)
                page.on("requestfailed", lambda request: (
                    external_media_failures.append(evidence)
                    if (evidence := external_media_failure_evidence(
                        request.url, request.resource_type, request.failure
                    )) is not None and len(external_media_failures) < 20
                    else None
                ))
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
                    bookmark_prefix = "AIP_SAFE_BOOKMARK_READ:"
                    if text.startswith(bookmark_prefix):
                        try:
                            raw_event = json.loads(text[len(bookmark_prefix):])
                        except (TypeError, ValueError):
                            return
                        if isinstance(raw_event, dict):
                            # The page clock is epoch time; rebase it onto this script's monotonic timeline.
                            for key in ("started_ms", "response_ms", "ended_ms"):
                                if isinstance(raw_event.get(key), int):
                                    raw_event[key] -= epoch_start_ms
                        observation = app_bookmark_read_observation(raw_event)
                        if observation is not None and len(app_bookmark_observations) < 20:
                            app_bookmark_observations.append(observation)
                    elif text.startswith(prefix):
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
                        if len(console_errors) < 12:
                            console_errors.append(console_error_evidence(message.type, message.text, message.location))

                page.on("console", capture_console_error)

                # The fixture's YouTube ID is not fetched or played. Its availability
                # thumbnail uses a local static image; only the iframe API loader gets a no-op local response.
                def block_external_media(route) -> None:
                    nonlocal blocked_external_image_count, fixture_thumbnail_redirect_count, youtube_player_api_fixture_count
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
                    player_api = youtube_iframe_api_stub(route.request.url, route.request.resource_type)
                    if player_api is not None:
                        youtube_player_api_fixture_count += 1
                        route.fulfill(status=200, content_type="application/javascript", body=player_api)
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
                    journey_stage = "login"
                    page.goto(f"http://localhost:{ui_port}/auth/1", wait_until="domcontentloaded")
                    page.locator("#authEmail").fill(email)
                    page.get_by_role("button", name="다음", exact=True).click()
                    expect(page).to_have_url(re.compile(r"/auth/2$"))
                    page.locator("#authPassword").fill(password)
                    page.get_by_role("button", name="로그인", exact=True).click()
                    expect(page).to_have_url(re.compile(r"/main$"), timeout=30000)
                    page.wait_for_load_state("networkidle", timeout=10000)
                    journey_stage = "home_list"

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

                    video_card = page.locator(".video-item-container").filter(has_text=VIDEO_TITLE).first
                    expect(video_card).to_be_visible(timeout=30000)
                    capture_render_state()
                    screenshot = output / f"{args.phase}-member-video-list.png"
                    page.screenshot(path=str(screenshot), full_page=True)
                    screenshot_names.append(screenshot.name)
                    video_card.click()
                    expect(page).to_have_url(re.compile(rf"/watch/{re.escape(VIDEO_ID)}$"))
                    expect(page.get_by_text(VIDEO_TITLE, exact=True).first).to_be_visible(timeout=30000)
                    journey_stage = "video_selected"
                    safety_notice = page.get_by_role("heading", name="안심하세요!", exact=True)
                    expect(safety_notice).to_be_visible(timeout=15000)
                    page.get_by_role("button", name="확인", exact=True).click()
                    expect(safety_notice).to_be_hidden(timeout=15000)
                    journey_stage = "bookmark_action"

                    bookmark_add = page.get_by_role("button", name="즐겨찾기 추가", exact=True)
                    bookmark_remove = page.get_by_role("button", name="즐겨찾기 해제", exact=True)
                    if args.phase == "add":
                        expect(bookmark_add).to_be_visible()
                        mark("bookmark_click")
                        bookmark_add.click()
                        mark("bookmark_click_returned")
                        expect(bookmark_remove).to_have_attribute("aria-pressed", "true")
                        journey_stage = "bookmark_reload"
                        mark("reload_start")
                        page.reload(wait_until="domcontentloaded")
                        mark("reload_return")
                        journey_stage = "bookmark_reloaded"
                        expect(bookmark_remove).to_have_attribute("aria-pressed", "true", timeout=30000)
                        journey_stage = "bookmark_state_confirmed"
                        label = "bookmarked"
                    else:
                        expect(bookmark_remove).to_be_visible()
                        mark("bookmark_click")
                        bookmark_remove.click()
                        mark("bookmark_click_returned")
                        expect(bookmark_add).to_have_attribute("aria-pressed", "false")
                        journey_stage = "bookmark_reload"
                        mark("reload_start")
                        page.reload(wait_until="domcontentloaded")
                        mark("reload_return")
                        journey_stage = "bookmark_reloaded"
                        expect(bookmark_add).to_have_attribute("aria-pressed", "false", timeout=30000)
                        journey_stage = "bookmark_state_confirmed"
                        label = "bookmark-removed"

                    screenshot = output / f"{args.phase}-{label}.png"
                    page.screenshot(path=str(screenshot), full_page=True)
                    screenshot_names.append(screenshot.name)
                    journey_stage = "completed"
                except Exception as error:
                    failure_type = safe_browser_failure_type(error)
                    failure_route_class = safe_browser_route_class(page.url, VIDEO_ID)
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
                    try:
                        control_values = page.locator("button[aria-pressed]").evaluate_all(
                            "elements => elements.slice(0, 10).map(element => ({"
                            "label: element.getAttribute('aria-label'), pressed: element.getAttribute('aria-pressed')"
                            "}))"
                        )
                        failure_bookmark_controls = safe_bookmark_control_evidence(control_values)
                        failure_watch_content_visible = page.locator(".video-information-container").count() > 0
                        bookmark_add = page.get_by_role("button", name="즐겨찾기 추가", exact=True)
                        bookmark_remove = page.get_by_role("button", name="즐겨찾기 해제", exact=True)
                        bookmark_ui_state = {
                            "add_visible": bookmark_add.is_visible() if bookmark_add.count() else False,
                            "add_aria_pressed": bookmark_add.get_attribute("aria-pressed") if bookmark_add.count() else None,
                            "remove_visible": bookmark_remove.is_visible() if bookmark_remove.count() else False,
                            "remove_aria_pressed": bookmark_remove.get_attribute("aria-pressed") if bookmark_remove.count() else None,
                        }
                    except Exception:
                        bookmark_ui_state = {"state": "unavailable"}
                    email_field = page.locator("#authEmail")
                    if email_field.count():
                        email_field.evaluate("element => { element.value = ''; }")
                    screenshot = output / f"{args.phase}-failure.png"
                    page.screenshot(path=str(screenshot), full_page=True)
                    screenshot_names.append(screenshot.name)
                    raise
            finally:
                mark("browser_close_start")
                browser.close()
                mark("browser_close_done")

        finalize_lifecycle()
        observed_paths = {row["path"] for row in api_paths}
        required_paths = {"/auth/check-email", "/auth/login", "/extension", "/apply"}
        if not required_paths.issubset(observed_paths):
            raise RuntimeError("browser did not issue the expected real AIP API requests")
        if any(row.get("x_aip_contract") != binding_contract for row in api_paths if row["path"] in {"/read", "/extension", "/apply"}):
            raise RuntimeError("the real browser request contract header differs from the generated UI binding")
        if any(row["path"] in required_paths and row["status"] != 200 for row in api_paths):
            raise RuntimeError("an expected AIP API request did not return HTTP 200")
        failed_reads = failed_read_requests(api_paths)
        # A read whose body Playwright could not collect says nothing about the server; report it as a
        # measurement gap rather than an error envelope, and never as success.
        uncollected_reads = [row for row in failed_reads if row.get("body_captured") is not True]
        if uncollected_reads:
            raise RuntimeError("React /read body was not collected (measurement gap): " + json.dumps(uncollected_reads, ensure_ascii=False))
        if failed_reads:
            raise RuntimeError("React /read returned an error envelope: " + json.dumps(failed_reads, ensure_ascii=False))
        # A request with no terminal event (a navigation abort leaves none) is not a success either.
        failed_bookmark_reads = [
            failure for failure in api_request_failures if failure.get("read_resource") == "VideoBookmark"
        ]
        unfinished_bookmark_reads = [
            item["corr"] for item in unfinished_requests() if item["read_resource"] == "VideoBookmark"
        ]
        if failed_bookmark_reads or unfinished_bookmark_reads:
            raise RuntimeError(
                "React bookmark /read never reached a terminal request event: "
                + json.dumps({"failed": failed_bookmark_reads, "unfinished_corr": unfinished_bookmark_reads})
            )
        if page_errors:
            raise RuntimeError("browser reported an unhandled React page error")
        if fixture_thumbnail_redirect_count < 1 or 200 not in fixture_thumbnail_response_statuses:
            raise RuntimeError("the local fixture thumbnail did not load successfully")
        if youtube_player_api_fixture_count < 1:
            raise RuntimeError("the local no-op player API fixture was not used")

        report = {
            "ui_revision": "2e7c021fa5abebd72278d841b8066a33d45cfbb3",
            "phase": args.phase,
            "fixture_video": {"id": VIDEO_ID, "title": VIDEO_TITLE},
            "server_listening_fingerprint": expected_contract,
            "ui_binding_fingerprint": binding_contract,
            "api_requests": api_paths,
            "timeline": timeline,
            "api_lifecycle": lifecycle_summary(),
            "api_request_failures": api_request_failures,
            "app_bookmark_read_observations": app_bookmark_observations,
            "app_bookmark_read_unpaired_count": app_unpaired_count,
            "read_requests": read_requests,
            "adapter_list_evidence": adapter_list_evidence,
            "rendered_state": rendered_state,
            "journey_stage": journey_stage,
            "failure_type": failure_type,
            "bookmark_ui_state": bookmark_ui_state,
            "failure_route_class": failure_route_class,
            "failure_bookmark_controls": failure_bookmark_controls,
            "failure_watch_content_visible": failure_watch_content_visible,
            "page_error_count": len(page_errors),
            "page_error_types": page_errors,
            "console_error_count": console_error_count,
            "console_errors": console_errors,
            "external_media_failures": external_media_failures,
            "blocked_external_image_count": blocked_external_image_count,
            "youtube_player_api_fixture_count": youtube_player_api_fixture_count,
            "fixture_thumbnail_redirect_count": fixture_thumbnail_redirect_count,
            "fixture_thumbnail_response_statuses": fixture_thumbnail_response_statuses,
            "screenshots": screenshot_names,
            "external_video_provider": "blocked; local no-op iframe API fixture; not fetched or played",
            "credentials_recorded": False,
            "browser_api_mocks": False,
        }
        (output / f"{args.phase}-browser-result.json").write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
        print(json.dumps({"phase": args.phase, "api_paths": sorted(observed_paths), "screenshots": screenshot_names}, ensure_ascii=False))
        return 0
    except Exception:
        # Preserve safe request metadata even on failure; never serialize response bodies or credentials.
        try:
            finalize_lifecycle()
        except Exception:
            pass
        report = {
            "ui_revision": "2e7c021fa5abebd72278d841b8066a33d45cfbb3",
            "phase": args.phase,
            "server_listening_fingerprint": expected_contract,
            "ui_binding_fingerprint": binding_contract,
            "api_requests": api_paths,
            "timeline": timeline,
            "api_lifecycle": lifecycle_summary(),
            "api_request_failures": api_request_failures,
            "app_bookmark_read_observations": app_bookmark_observations,
            "app_bookmark_read_unpaired_count": app_unpaired_count,
            "read_requests": read_requests,
            "adapter_list_evidence": adapter_list_evidence,
            "rendered_state": rendered_state,
            "journey_stage": journey_stage,
            "failure_type": failure_type,
            "bookmark_ui_state": bookmark_ui_state,
            "failure_route_class": failure_route_class,
            "failure_bookmark_controls": failure_bookmark_controls,
            "failure_watch_content_visible": failure_watch_content_visible,
            "fixture_video": {"id": VIDEO_ID},
            "page_error_count": len(page_errors),
            "page_error_types": page_errors,
            "console_error_count": console_error_count,
            "console_errors": console_errors,
            "external_media_failures": external_media_failures,
            "blocked_external_image_count": blocked_external_image_count,
            "youtube_player_api_fixture_count": youtube_player_api_fixture_count,
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
