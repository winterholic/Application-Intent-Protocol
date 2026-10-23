#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 1 || $# -gt 2 ]]; then
  echo "usage: $0 /path/to/clean/facereview-front-joowon-checkout [definition.aip]" >&2
  exit 2
fi

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
ui_root="$1"
definition="${2:-$repo_root/product/tests/fixtures/facereview-mysql/facereview.aip}"
if [[ ! -f "$definition" ]]; then
  echo "AIP definition input is not a file: $definition" >&2
  exit 2
fi
expected_revision="2e7c021fa5abebd72278d841b8066a33d45cfbb3"
actual_revision="$(git -C "$ui_root" rev-parse HEAD)"
if [[ "$actual_revision" != "$expected_revision" ]]; then
  echo "pinned UI revision mismatch: expected $expected_revision, got $actual_revision" >&2
  exit 1
fi
if [[ -n "$(git -C "$ui_root" status --porcelain)" ]]; then
  echo "UI checkout must be clean before applying the overlay" >&2
  exit 1
fi

overlay="$repo_root/porting/facereview/ui-overlay/src/api"
for file in aipClient.ts auth.ts authAdapter.ts index.ts readAdapter.ts request.ts watch.ts writeAdapter.ts youtube.ts; do
  mkdir -p "$ui_root/src/api"
  cp "$overlay/$file" "$ui_root/src/api/$file"
done

mkdir -p "$ui_root/src/aip"
"${AIP_CLI_BIN:-aip}" service gen --lang ts --out "$ui_root/src/aip/facereview.ts" \
  --sdk-import @aip/sdk \
  "$definition"

echo "overlay applied to pinned UI $actual_revision at $ui_root using definition $definition"
