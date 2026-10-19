"""SDK steps of the 7.3.3.9 live baseline: read, permission denial, write, soft delete, job, all as real users.

Called by run-baseline.sh against an already served deployment; prints one JSON document with the raw
results and exits non-zero when an expectation fails.

  python3 baseline_client.py BASE_URL FINGERPRINT WORKDIR
"""

import json
import pathlib
import sys

HERE = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE.parent.parent.parent / "product" / "python"))

import jwks  # noqa: E402
from aip_sdk import connect  # noqa: E402


def main(base, fingerprint, workdir):
    def client(subject):
        return connect(base, jwks.mint(workdir, subject), fingerprint)

    alice, bob, carol, dan = client("alice"), client("bob"), client("carol"), client("dan")
    report, failures = {}, []

    def expect(name, condition, detail):
        report[name] = {"pass": bool(condition), "detail": detail}
        if not condition:
            failures.append(name)

    # SDK read, 1 row per principal view
    users_bob = bob.read({"read": "User", "select": ["id", "email"], "sort": [{"field": "id"}]})
    expect("sdk_read_users_as_member", [u["email"] for u in users_bob] ==
           ["alice@acme.test", "bob@acme.test", "carol@acme.test", "dan@acme.test"], users_bob)
    expect("sdk_read_project_hidden_from_org_member_without_membership",
           dan.read({"read": "Project", "select": ["id"]}) == [], "dan sees no project")

    # write as member, denied as viewer
    gen = bob.bundle([{"create": "Secret", "values": {"folder": "1", "key": "DB_URL", "owner": "2"}}], key="baseline-secret")
    expect("member_creates_secret", gen.get("ok") is True, gen)
    secret_id = gen["created"][0]["id"]
    denied = carol.bundle([{"create": "Secret", "values": {"folder": "1", "key": "VIEWER_WROTE", "owner": "3"}}],
                          key="baseline-viewer")
    expect("viewer_write_denied", denied.get("ok") is False, denied)
    seen = carol.read({"read": "Secret", "select": ["id", "key"], "filter": [{"field": "key", "op": "eq", "value": "DB_URL"}]})
    expect("viewer_reads_secret_name", [r["key"] for r in seen] == ["DB_URL"], seen)

    # job: enqueue as the viewer (a reader), outbox notification recorded
    job = carol.jobs.enqueue("secretReminder", {"secret": secret_id}, key="baseline-reminder")
    receipt = carol.jobs.wait(job["id"], timeout=30)
    expect("job_secretReminder_succeeds_for_reader", receipt["state"] == "SUCCEEDED", receipt)
    refused = None
    try:
        dan.jobs.enqueue("secretReminder", {"secret": secret_id}, key="baseline-dan")
    except Exception as error:  # AipError: enqueue is refused before any run exists
        refused = getattr(error, "code", repr(error))
    expect("job_enqueue_refused_without_project_access", refused is not None, refused)

    # physical delete, then the same name again
    deleted = bob.apply({"apply": "Secret.delete", "target": {"ids": [secret_id]}}, key="baseline-delete")
    expect("member_deletes_secret", deleted.get("ok") is True, deleted)
    live = bob.read({"read": "Secret", "select": ["id"], "filter": [{"field": "id", "op": "eq", "value": secret_id}]})
    expect("deleted_secret_not_listed", live == [], live)
    again = bob.bundle([{"create": "Secret", "values": {"folder": "1", "key": "DB_URL", "owner": "2"}}], key="baseline-regen")
    expect("name_can_be_recreated", again.get("ok") is True, again)

    print(json.dumps({"base": "loopback", "results": report, "failed": failures}, indent=2, ensure_ascii=False))
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main(*sys.argv[1:4]))
