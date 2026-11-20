"""Real Chromium reproduction of the late first-main requests behind Verify 44117223558.

A SPA navigation keeps the previous document's networkidle, the page schedules its
queries after render, and Playwright's sync API delivers `request` events only while
it is pumped. The journey must therefore wait for the requests it expects instead of
trusting an empty started == finished. Skipped where Playwright/Chromium are absent.
"""
import json
import threading
import time
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlsplit

from product.tests.facereview_ui_browser_evidence import (
    FIRST_MAIN_EXTENSIONS,
    extension_request_evidence,
    wait_for_api_completion,
    wait_for_expected_extensions,
)

try:
    from playwright.sync_api import sync_playwright
except ImportError:  # pragma: no cover
    sync_playwright = None

QUERY_DELAY_MS = 900  # past networkidle's settling window, so the queries start after the first drain check

PAGE = """<!doctype html><meta charset=utf-8><div id=root></div><script>
const call = (path, extension) => fetch('http://127.0.0.1:%(api)d' + path, {method: 'POST',
  headers: {'content-type': 'application/json', 'x-aip-contract': 'c'}, body: JSON.stringify({extension})}).then(r => r.text());
const root = document.getElementById('root');
const mainQueries = () => ['Video.homePersonalized', 'Video.homeVideoAll', 'Video.homeCategory'].map(n => call('/extension', n));
if (location.pathname === '/main') {
  root.innerHTML = '<button>main</button>';
  mainQueries();
} else {
  root.innerHTML = '<button>go</button>';
  root.firstChild.onclick = () => {
    history.pushState({}, '', '/main');  // SPA navigation: no load event, networkidle stays cached
    root.innerHTML = '<button>main</button>';
    setTimeout(mainQueries, %(delay)d);   // queries are scheduled after render
  };
}
</script>"""


class _Handler(BaseHTTPRequestHandler):
    api_port = 0

    def log_message(self, *args):
        pass

    def _send(self, status, body=b"", content_type="application/json"):
        self.send_response(status)
        self.send_header("access-control-allow-origin", "*")
        self.send_header("access-control-allow-headers", "*")
        self.send_header("content-type", content_type)
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_OPTIONS(self):
        self._send(204)

    def do_POST(self):
        self.rfile.read(int(self.headers.get("content-length") or 0))
        time.sleep(0.2)
        self._send(200, b'{"ok":true}')

    def do_GET(self):
        self._send(200, (PAGE % {"api": self.api_port, "delay": QUERY_DELAY_MS}).encode(), "text/html")


def _chromium_available():
    if sync_playwright is None:
        return False
    try:
        with sync_playwright() as playwright:
            playwright.chromium.launch(headless=True).close()
        return True
    except Exception:  # pragma: no cover
        return False


@unittest.skipUnless(_chromium_available(), "Playwright Chromium is not installed")
class FirstMainRequestsBrowserTests(unittest.TestCase):
    def test_reload_after_spa_navigation_leaves_no_first_main_request_unfinished(self):
        server = ThreadingHTTPServer(("127.0.0.1", 0), _Handler)
        _Handler.api_port = server.server_address[1]
        threading.Thread(target=server.serve_forever, daemon=True).start()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)

        started, finished, pending, failures, names = set(), set(), {}, [], set()
        phases = {}
        stage = "tutorial_action"

        def tracked(request):
            return urlsplit(request.url).port == server.server_address[1] and request.method == "POST"

        def on_request(request):
            if not tracked(request):
                return
            started.add(request)
            phases[request] = stage
            if stage == "tutorial_action":
                name = extension_request_evidence(request.post_data_json).get("extension")
                if name:
                    names.add(name)

        def on_response(response):
            if tracked(response.request):
                pending[response.request] = response

        def on_finished(request):
            response = pending.pop(request, None)
            if response is not None:
                response.body()
                finished.add(request)

        with sync_playwright() as playwright:
            browser = playwright.chromium.launch(headless=True)
            page = browser.new_page()
            page.on("request", on_request)
            page.on("response", on_response)
            page.on("requestfinished", on_finished)
            page.on("requestfailed", lambda request: failures.append(request) if tracked(request) else None)
            page.goto(f"http://127.0.0.1:{server.server_address[1]}/", wait_until="domcontentloaded")
            page.get_by_role("button", name="go").click()
            page.get_by_role("button", name="main").wait_for()
            page.wait_for_load_state("networkidle")
            wait_for_expected_extensions(page, names, FIRST_MAIN_EXTENSIONS, failures)
            wait_for_api_completion(page, started, finished, pending, failures)
            self.assertEqual(names, set(FIRST_MAIN_EXTENSIONS))
            self.assertEqual({phases[request] for request in started}, {"tutorial_action"})
            stage = "reload_main"
            page.reload(wait_until="domcontentloaded")
            page.get_by_role("button", name="main").wait_for()
            wait_for_api_completion(page, started, finished, pending, failures)
            browser.close()
        self.assertEqual((len(started), len(finished), failures), (6, 6, []))


if __name__ == "__main__":
    unittest.main()
