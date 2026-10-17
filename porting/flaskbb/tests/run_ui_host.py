"""Start the UI renderer with a pre-bound synthetic test identity for AIP integration tests."""
import json
import os
import signal
from werkzeug.serving import make_server

from app import create_app


def main():
    signal.signal(signal.SIGTERM, lambda *_: os._exit(0))
    if os.environ.get("FLASKBB_TEST_INTEGRATION") != "1":
        raise SystemExit("test harness only")
    session_id = "aip-integration-session"
    sessions = {
        session_id: {
            "access_token": os.environ["FLASKBB_TEST_ACCESS_TOKEN"],
            "sub": os.environ["FLASKBB_TEST_SUBJECT"],
            "username": os.environ["FLASKBB_TEST_USERNAME"],
            "csrf_token": "aip-integration-csrf-token",
        }
    }
    app = create_app(
        {
            "SECRET_KEY": "aip-integration-only-session-secret",
            "AIP_URL": os.environ["FLASKBB_TEST_AIP_URL"],
        },
        sessions=sessions,
    )
    server = make_server("127.0.0.1", 0, app)
    print(json.dumps({"port": server.server_port}), flush=True)
    server.serve_forever()


if __name__ == "__main__":
    main()
