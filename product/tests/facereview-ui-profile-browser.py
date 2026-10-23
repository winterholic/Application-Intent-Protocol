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
import traceback
import time
from urllib.parse import urlsplit

from playwright.sync_api import expect, sync_playwright
sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
from product.tests.facereview_ui_browser_evidence import (
    auth_transport_trace_evidence,
    body_failure_reason,
    profile_render_evidence,
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
    parser.add_argument("--journey", choices=("profile", "reissue"), required=True)
    args = parser.parse_args()
    email = os.environ["AIP_FACEREVIEW_JOURNEY_EMAIL"]
    password = os.environ["AIP_FACEREVIEW_JOURNEY_PASSWORD"]
    expected_contract = os.environ["AIP_FACEREVIEW_EXPECTED_CONTRACT"]
    access_ttl_secs = int(os.environ.get("AIP_FACEREVIEW_ACCESS_TTL_SECS", "0"))
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
    request_started: dict[object, float] = {}
    page_errors: list[str] = []
    stage = "startup"
    profile_fields: dict[str, object] = {}
    member_reissue_fields: dict[str, object] = {}
    failure_type = None
    failure_assertion = "unclassified"
    browser = None
    page = None
    auth_trace: dict[str, object] = {"requests": [], "expired_refusal_count": 0, "reauth_branch_inputs_observed": 0, "reissue_request_count": 0}
    artifact_name = "profile-browser-result.json" if args.journey == "profile" else "member-reissue-browser-result.json"
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
            page.add_init_script("""() => {
              const trace = [];
              window.__aipFetchTrace = trace;
              const nativeFetch = window.fetch.bind(window);
              window.fetch = async (input, init) => {
                const rawUrl = typeof input === 'string' ? input : input instanceof URL ? input.href : input.url;
                const parsed = new URL(rawUrl, location.href);
                const method = init?.method ?? (input instanceof Request ? input.method : 'GET');
                const headers = new Headers(init?.headers ?? (input instanceof Request ? input.headers : undefined));
                const frames = String(new Error().stack ?? '').split('\\n').slice(2).map((line) => {
                  const match = line.match(/\\bat (?:async )?(?:.*\\.)?([A-Za-z_$][\\w$]*) \\(/);
                  return match?.[1] ?? null;
                }).filter(Boolean).slice(0, 12);
                const response = await nativeFetch(input, init);
                trace.push({method, path: parsed.pathname, status: response.status,
                  authorization_header_present: headers.has('authorization'),
                  content_type: response.headers.get('content-type'), fetch_frames: frames});
                return response;
              };
            }""")

            def capture_request(request) -> None:
                parsed = urlsplit(request.url)
                if parsed.port == urlsplit(args.api).port:
                    request_started[request] = time.monotonic() * 1000

            def capture_response(response) -> None:
                parsed = urlsplit(response.url)
                if parsed.port != urlsplit(args.api).port:
                    return
                request_headers = response.request.headers
                now = time.monotonic() * 1000
                row: dict[str, object] = {
                    "method": response.request.method,
                    "path": parsed.path,
                    "status": response.status,
                    "authorization_header_present": "authorization" in request_headers,
                    "content_type": response.headers.get("content-type"),
                    "request_started_ms": request_started.get(response.request),
                    "response_headers_ms": now,
                }
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
                finished_ms = time.monotonic() * 1000
                row["response_body_complete_ms"] = finished_ms
                row["request_finished_ms"] = finished_ms
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

            def mark_network_idle_since(mark: int) -> float:
                completed_ms = time.monotonic() * 1000
                for row in requests[mark:]:
                    row["network_idle_completed_ms"] = completed_ms
                return completed_ms

            def auth_trace_evidence() -> dict[str, object]:
                fetch_rows = page.evaluate("() => window.__aipFetchTrace ?? []")
                unused = list(fetch_rows) if isinstance(fetch_rows, list) else []
                merged = []
                for request in requests:
                    match_index = next((index for index, row in enumerate(unused)
                                        if row.get("path") == request.get("path")
                                        and row.get("method") == request.get("method")
                                        and row.get("status") == request.get("status")), None)
                    fetch = unused.pop(match_index) if match_index is not None else {}
                    merged.append({**fetch, **request, "fetch_frames": fetch.get("fetch_frames", [])})
                return auth_transport_trace_evidence(merged)

            def profile_render_snapshot() -> dict[str, object]:
                raw = page.evaluate("""(apiBodiesSettled) => {
                  const names = [
                    'my-page-container', 'my-page-user-container',
                    'my-page-watched-contents-container', 'my-page-emotion-container',
                  ];
                  const nodes = {};
                  for (const name of names) {
                    const element = document.querySelector('.' + name);
                    if (!element) continue;
                    const style = getComputedStyle(element);
                    const cssMilliseconds = (value) => {
                      const first = value.split(',')[0].trim();
                      const amount = Number.parseFloat(first);
                      if (!Number.isFinite(amount)) return null;
                      return first.endsWith('ms') ? amount : amount * 1000;
                    };
                    let effectiveOpacity = 1;
                    let visible = true;
                    for (let current = element; current && current !== document.documentElement; current = current.parentElement) {
                      const currentStyle = getComputedStyle(current);
                      effectiveOpacity *= Number(currentStyle.opacity);
                      if (currentStyle.display === 'none' || currentStyle.visibility !== 'visible') visible = false;
                    }
                    const rect = element.getBoundingClientRect();
                    nodes[name] = {
                      display: style.display,
                      visibility: visible ? style.visibility : 'hidden',
                      opacity: Number(style.opacity),
                      effective_opacity: effectiveOpacity,
                      transform: style.transform === 'none' ? 'none' : 'applied',
                      animation_name: style.animationName.split(',')[0].trim(),
                      animation_duration_ms: cssMilliseconds(style.animationDuration),
                      animation_delay_ms: cssMilliseconds(style.animationDelay),
                      animation_fill_mode: style.animationFillMode.split(',')[0].trim(),
                      animation_play_state: style.animationPlayState.split(',')[0].trim(),
                      transition_properties: style.transitionProperty.split(',').map((value) => value.trim()),
                      transition_duration_ms: cssMilliseconds(style.transitionDuration),
                      rect: {width: rect.width, height: rect.height},
                    };
                  }
                  const root = document.querySelector('.my-page-container');
                  const animations = root ? root.getAnimations({subtree: true}).map((animation) => {
                    const timing = animation.effect?.getComputedTiming();
                    const properties = new Set();
                    for (const frame of animation.effect?.getKeyframes?.() ?? []) {
                      for (const property of Object.keys(frame)) {
                        if (property === 'opacity' || property === 'transform' || property === 'visibility') properties.add(property);
                      }
                    }
                    return {
                      name: animation.animationName ?? animation.transitionProperty ?? 'other',
                      play_state: animation.playState,
                      current_ms: animation.currentTime !== null && Number.isFinite(Number(animation.currentTime)) ? Number(animation.currentTime) : null,
                      duration_ms: Number.isFinite(Number(timing?.duration)) ? Number(timing.duration) : null,
                      delay_ms: Number.isFinite(Number(timing?.delay)) ? Number(timing.delay) : null,
                      properties: [...properties],
                    };
                  }) : [];
                  return {
                    path: location.pathname,
                    api_bodies_settled: apiBodiesSettled,
                    body_text_length: document.body.innerText.length,
                    viewport: {width: innerWidth, height: innerHeight},
                    nodes,
                    animations,
                  };
                }""", not pending_bodies)
                return profile_render_evidence(raw)

            def finish_profile_animations() -> None:
                # Wait for observed finite profile animations, not an arbitrary screenshot delay.
                page.evaluate("""async () => {
                  const root = document.querySelector('.my-page-container');
                  if (!root) throw new Error('profile root is missing');
                  const finiteAnimations = root.getAnimations({subtree: true}).filter((animation) => {
                    const timing = animation.effect?.getComputedTiming();
                    return animation.playState !== 'finished' && Number.isFinite(Number(timing?.duration))
                      && Number.isFinite(Number(timing?.iterations));
                  });
                  await Promise.all(finiteAnimations.map((animation) => animation.finished.catch(() => undefined)));
                }""")

            def access_token_digest() -> str | None:
                # The token stays in the page; only a digest is returned to prove rotation.
                return page.evaluate("""async () => {
                  const raw = localStorage.getItem('auth-storage');
                  const token = raw ? JSON.parse(raw)?.state?.access_token : null;
                  if (typeof token !== 'string' || !token) return null;
                  const digest = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(token));
                  return Array.from(new Uint8Array(digest)).map((byte) => byte.toString(16).padStart(2, '0')).join('');
                }""")

            page.on("request", capture_request)
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
            wait_for_api_bodies()
            page.wait_for_load_state("networkidle", timeout=15000)
            login_token_digest = access_token_digest()
            if login_token_digest is None:
                failure_assertion = "member_login_access_token_stored"
                raise AssertionError("the visible member login did not store its access token")
            if any(row.get("path") == "/auth/reissue" for row in requests):
                failure_assertion = "no_reissue_before_expiry"
                raise AssertionError("the member token reissued before the expiry journey began")
            if args.journey == "reissue":
                login_at = time.monotonic()
                remaining = access_ttl_secs + 1.5 - (time.monotonic() - login_at)
                if remaining > 0:
                    page.wait_for_timeout(int(remaining * 1000))
                expiry_request_mark = len(requests)
                stage = "member_token_expired"
                page.get_by_role("button", name="마이페이지로 이동", exact=True).click()
                expect(page).to_have_url(re.compile(r"/my$"), timeout=15000)
                expect(page.get_by_role("heading", name=f"{PROFILE_NAME}님", exact=True)).to_be_visible()
                wait_for_api_bodies()
                page.wait_for_load_state("networkidle", timeout=15000)
                wait_for_api_bodies()
                network_idle_ms = mark_network_idle_since(expiry_request_mark)
                auth_trace = auth_trace_evidence()
                expiry_requests = requests[expiry_request_mark:]
                refused = [row for row in expiry_requests if row.get("code") == "WIRE.TOKEN_EXPIRED"]
                reissues = [row for row in expiry_requests if row.get("path") == "/auth/reissue"]
                reissue_index = expiry_requests.index(reissues[0]) if reissues else -1
                retried = [row for row in expiry_requests[reissue_index + 1:] if row.get("path") in {"/read", "/extension"}] if reissues else []
                if not refused:
                    failure_assertion = "member_navigation_exercises_expired_access_token"
                    raise AssertionError("member navigation did not exercise an expired access token")
                if len(reissues) != 1 or reissues[0].get("status") != 200:
                    failure_assertion = "member_concurrent_expiry_shares_one_successful_reissue"
                    raise AssertionError("member concurrent expired reads did not share exactly one successful reissue")
                if not retried or any(row.get("status") != 200 or row.get("ok") is not True for row in retried):
                    failure_assertion = "member_reads_recover_after_reissue"
                    raise AssertionError("member reads did not recover after reissue")
                renewed_token_digest = access_token_digest()
                if renewed_token_digest is None or renewed_token_digest == login_token_digest:
                    failure_assertion = "member_auth_storage_replaces_expired_token"
                    raise AssertionError("member auth storage did not replace the expired access token")
                stage = "member_reissued_profile_visible"
                finish_profile_animations()
                member_render = profile_render_snapshot()
                required_member_nodes = {"my-page-container", "my-page-user-container", "my-page-watched-contents-container", "my-page-emotion-container"}
                member_visible_nodes = {node.get("node") for node in member_render.get("nodes", []) if node.get("visually_ready") is True}
                if member_render.get("route") != "my" or not required_member_nodes.issubset(member_visible_nodes):
                    failure_assertion = "member_profile_visible_after_reissue"
                    raise AssertionError("member profile did not visibly recover after token reissue")
                page.screenshot(path=str(output / "member-after-token-reissue.png"), full_page=True)
                member_reissue_fields = {
                    "expired_refusals": len(refused),
                    "reissue_requests": len(reissues),
                    "successful_retries": len(retried),
                    "new_access_token_stored": True,
                    "network_idle_completed_ms": network_idle_ms,
                    "render": member_render,
                }
                report = {
                    "ui_revision": "2e7c021fa5abebd72278d841b8066a33d45cfbb3",
                    "phase": "member_reissue",
                    "member_reissue": member_reissue_fields,
                    "auth_transport_trace": auth_trace,
                    "api_requests": requests,
                    "journey_stage": "completed",
                    "page_error_count": len(page_errors),
                    "page_error_types": page_errors,
                    "credentials_recorded": False,
                    "browser_api_mocks": False,
                }
                (output / artifact_name).write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
                print(json.dumps({"phase": "member_reissue", "journey_stage": "completed", "auth_transport_trace": auth_trace}, ensure_ascii=False))
                return 0
            stage = "member_profile"
            profile_request_mark = len(requests)
            page.get_by_role("button", name="마이페이지로 이동", exact=True).click()
            expect(page).to_have_url(re.compile(r"/my$"), timeout=15000)
            expect(page.get_by_role("heading", name=f"{PROFILE_NAME}님", exact=True)).to_be_visible()
            wait_for_api_bodies()
            page.wait_for_load_state("networkidle", timeout=15000)
            wait_for_api_bodies()
            mark_network_idle_since(profile_request_mark)
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
                "member_token_reissue": member_reissue_fields,
            }
            page.screenshot(path=str(output / "profile-before-save.png"), full_page=True)

            # Exercise the original edit form through the overlay's generated SDK client.
            page.get_by_label("닉네임", exact=True).fill(UPDATED_PROFILE_NAME)
            page.get_by_role("button", name=re.compile("공포")).click()
            page.get_by_role("button", name=re.compile("동물")).click()
            expect(page.get_by_role("button", name=re.compile("공포"))).to_have_attribute("aria-pressed", "false")
            expect(page.get_by_role("button", name=re.compile("동물"))).to_have_attribute("aria-pressed", "true")
            save_request_mark = len(requests)
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
            mark_network_idle_since(save_request_mark)
            expect(page.get_by_role("heading", name=f"{UPDATED_PROFILE_NAME}님", exact=True)).to_be_visible()
            save_api_requests = requests[save_request_mark:]
            page_data_requests = [row for row in save_api_requests if row.get("path") in {"/read", "/extension"}]
            if not page_data_requests or any(row.get("status") != 200 or row.get("ok") is not True for row in page_data_requests):
                raise AssertionError("profile destination page had an incomplete or refused API response")
            profile_fields["after_save_before_animation"] = profile_render_snapshot()
            page.screenshot(path=str(output / "profile-after-save-before-animation.png"), full_page=True)
            finish_profile_animations()
            after_animation = profile_render_snapshot()
            profile_fields["after_save_after_animation"] = after_animation
            if after_animation.get("route") != "my" or after_animation.get("api_bodies_settled") is not True:
                raise AssertionError("profile screenshot did not follow the /my route and completed API response bodies")
            required_nodes = {"my-page-container", "my-page-user-container", "my-page-watched-contents-container", "my-page-emotion-container"}
            visible_nodes = {node.get("node") for node in after_animation.get("nodes", []) if node.get("visually_ready") is True}
            if not required_nodes.issubset(visible_nodes):
                raise AssertionError("profile sections were not visually ready after their observed animations completed")
            if any(animation.get("play_state") == "running" for animation in after_animation.get("animations", [])):
                raise AssertionError("a profile animation remained active at the final screenshot")
            profile_fields["saved_name"] = UPDATED_PROFILE_NAME
            profile_fields["saved_genres"] = sorted(UPDATED_PROFILE_GENRES)
            profile_fields["write_bundle_ok"] = True
            profile_fields["after_save_api_responses"] = [
                {"path": row.get("path"), "status": row.get("status"), "ok": row.get("ok"), "code": row.get("code")}
                for row in save_api_requests
            ]
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
            "phase": "profile_after_save",
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
        failure_line = next((frame.lineno for frame in reversed(traceback.extract_tb(error.__traceback__))
                             if Path(frame.filename).resolve() == Path(__file__).resolve()), None)
        if args.journey == "reissue" and page is not None:
            try:
                auth_trace = auth_trace_evidence()
            except Exception:
                pass
        report = {
            "ui_revision": "2e7c021fa5abebd72278d841b8066a33d45cfbb3",
            "phase": "profile_after_save" if args.journey == "profile" else "member_reissue",
            "fixture": {"name": PROFILE_NAME, "favorite_genres": sorted(PROFILE_GENRES), "other_member_marker": "vlog"} if args.journey == "profile" else {},
            "profile_ui": profile_fields,
            "member_reissue": member_reissue_fields,
            "auth_transport_trace": auth_trace if args.journey == "reissue" else None,
            "api_requests": requests,
            "journey_stage": stage,
            "failure_type": failure_type,
            "assertion": failure_assertion,
            "failure_line": failure_line,
            "page_error_count": len(page_errors),
            "page_error_types": page_errors,
            "credentials_recorded": False,
            "browser_api_mocks": False,
        }
        (output / artifact_name).write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
        print(json.dumps({"phase": report["phase"], "journey_stage": stage, "failure_type": failure_type, "assertion": failure_assertion, "failure_line": failure_line}, ensure_ascii=False))
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
