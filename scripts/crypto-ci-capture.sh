#!/usr/bin/env bash
set -uo pipefail

step=${1:?step name required}
evidence_dir=${CRYPTO_EVIDENCE_DIR:?CRYPTO_EVIDENCE_DIR is required}
mkdir -p "$evidence_dir"
result="$evidence_dir/$step.json"
stdout="$RUNNER_TEMP/crypto-repair-$step.stdout"
stderr="$RUNNER_TEMP/crypto-repair-$step.stderr"

run_command() {
  local label=$1
  shift
  set +e
  "$@" >"$stdout" 2>"$stderr"
  local code=$?
  set -e
  python3 - "$result" "$step" "$label" "$code" "$stdout" "$stderr" "$evidence_dir" <<'PY'
import json, pathlib, re, sys
result, step, label, code, stdout, stderr, evidence_dir = sys.argv[1:]
def sanitize(path):
    text = pathlib.Path(path).read_text(errors="replace")
    text = re.sub(r"k[0-9]+:[A-Za-z0-9+/=_-]+", "kN:[REDACTED]", text)
    return re.sub(r"\b[A-Za-z0-9+/]{40,}={0,2}\b", "[REDACTED_BASE64]", text)
stdout_text, stderr_text = sanitize(stdout), sanitize(stderr)
evidence_dir = pathlib.Path(evidence_dir)
stdout_file, stderr_file = evidence_dir / f"{step}.stdout.txt", evidence_dir / f"{step}.stderr.txt"
stdout_file.write_text(stdout_text)
stderr_file.write_text(stderr_text)
sha = __import__("subprocess").check_output(["git", "rev-parse", "HEAD"], text=True).strip()
excerpt = "\n".join((stdout_text + "\n" + stderr_text).splitlines()[-12:])[-1800:] if int(code) else ""
payload = {
    "step": step,
    "command": label,
    "source_revision": sha,
    "status": "passed" if int(code) == 0 else "failed",
    "exit_code": int(code),
    "stdout_file": str(stdout_file),
    "stderr_file": str(stderr_file),
    "error_excerpt": excerpt,
}
pathlib.Path(result).write_text(json.dumps(payload, ensure_ascii=False, indent=2) + "\n")
print(json.dumps({k: v for k, v in payload.items() if k != "error_excerpt"}, ensure_ascii=False))
if excerpt:
    print(excerpt, file=sys.stderr)
sys.exit(int(code))
PY
}

case "$step" in
  metadata)
    python3 - "$result" <<'PY'
import json, os, pathlib, subprocess, sys
sha = subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()
payload = {"step":"metadata", "status":"recorded", "source_revision":sha,
           "workflow_run_id":os.environ.get("GITHUB_RUN_ID"),
           "workflow_attempt":os.environ.get("GITHUB_RUN_ATTEMPT"),
           "workflow_ref":os.environ.get("GITHUB_REF"),
           "runner_os":os.environ.get("RUNNER_OS")}
pathlib.Path(sys.argv[1]).write_text(json.dumps(payload, ensure_ascii=False, indent=2) + "\n")
print(json.dumps(payload, ensure_ascii=False))
PY
    ;;
  fmt) run_command 'cargo fmt --all -- --check' cargo fmt --all -- --check ;;
  check) run_command 'cargo check --locked -p aip-service -p aip-execution' cargo check --locked -p aip-service -p aip-execution ;;
  service_test) run_command 'cargo test --locked -p aip-service --test encryption_runtime_test' cargo test --locked -p aip-service --test encryption_runtime_test ;;
  execution_test) run_command 'cargo test --locked -p aip-execution --test encryption_sealer_opener --test encryption_rekey' cargo test --locked -p aip-execution --test encryption_sealer_opener --test encryption_rekey ;;
  clippy) run_command 'cargo clippy --locked --all-targets -p aip-service -p aip-execution -p aip-cli -- -D warnings' cargo clippy --locked --all-targets -p aip-service -p aip-execution -p aip-cli -- -D warnings ;;
  cli_build) run_command 'cargo build --locked -p aip-cli' cargo build --locked -p aip-cli ;;
  cli_gate)
    config="$RUNNER_TEMP/crypto-repair-service.json"
    python3 - "$config" <<'PY'
import json, pathlib, sys
config = {
  "schema":"aip_crypto_repair_gate",
  "database_url_env":"AIP_REKEY_GATE_TEST_DATABASE_URL",
  "listen":"127.0.0.1:0",
  "auth":{"issuer":"https://issuer.invalid","audience":"aip-test","jwks":{"kind":"file","path":"unused-jwks.json"}},
  "encryption":{"kind":"spec","spec":"k1:BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=","active":"k1"}
}
pathlib.Path(sys.argv[1]).write_text(json.dumps(config))
PY
    export AIP_REKEY_GATE_TEST_DATABASE_URL='host=127.0.0.1 port=1 dbname=unreachable user=runner connect_timeout=1'
    python3 - "$RUNNER_TEMP" "$result" <<'PY'
import json, pathlib, re, subprocess, sys
root, out = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2])
key = "k1:BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc="
base = {
 "scope_kind":"service", "scope_id":"global", "holder_id":"crypto-ci-test",
 "targets":[{"table":"secret","column":"value","id_column":"id","resource":"Secret","field":"value"}]
}
cases = [("closed_gate_without_legacy_claim", False, "REKEY_OPERATIONAL_GATE_CLOSED"),
         ("closed_gate_with_legacy_claim", True, "REKEY_OPERATIONAL_GATE_CLOSED")]
observed=[]
for name, legacy_claim, expected in cases:
    options = dict(base, inventory_complete=legacy_claim)
    option_path = root / f"crypto-repair-{name}.json"
    option_path.write_text(json.dumps(options))
    proc = subprocess.run(["target/debug/aip", "service", "rekey", "--config", str(root / "crypto-repair-service.json"), "--options", str(option_path)], capture_output=True, text=True)
    text = (proc.stderr + "\n" + proc.stdout).strip()
    try:
        body = json.loads(text.splitlines()[-1])
        code = body.get("code")
        message = str(body.get("message", ""))
    except Exception:
        code, message = None, text[-1000:]
    sanitized = re.sub(r"k[0-9]+:[A-Za-z0-9+/=_-]+", "kN:[REDACTED]", message)
    case = {"case":name,"expected_code":expected,"observed_code":code,"exit_code":proc.returncode,
            "status":"passed" if proc.returncode != 0 and code == expected and key not in text else "failed",
            "message":sanitized[:800]}
    observed.append(case)
payload={"step":"cli_gate","command":"target/debug/aip service rekey --config <runner-temp-config> --options <runner-temp-options>",
         "database_url":"unreachable fixture sentinel on 127.0.0.1:1; gate must reject before connect",
         "status":"passed" if all(c["status"] == "passed" for c in observed) else "failed","cases":observed}
out.write_text(json.dumps(payload, ensure_ascii=False, indent=2) + "\n")
print(json.dumps(payload, ensure_ascii=False))
sys.exit(0 if payload["status"] == "passed" else 1)
PY
    ;;
  cli_gate_skipped)
    python3 - "$result" <<'PY'
import json, pathlib, sys
p={"step":"cli_gate","status":"skipped","reason":"cargo build --locked -p aip-cli did not succeed; CLI checks depend on that build"}
pathlib.Path(sys.argv[1]).write_text(json.dumps(p, ensure_ascii=False, indent=2)+"\n")
print(json.dumps(p, ensure_ascii=False))
PY
    ;;
  *) echo "unknown step: $step" >&2; exit 2 ;;
esac
