#!/usr/bin/env bash
# Apply the admin console overlay (WBS 7.2.9 / 7.2.10) to a CLEAN checkout of the pinned admin UI `new-facereview-admin-front`.
# The original checkout is never edited: pass a fresh clone, as the member overlay script expects.
set -euo pipefail

if [[ $# -lt 1 || $# -gt 2 ]]; then
  echo "usage: $0 /path/to/clean/new-facereview-admin-front-checkout [definition.aip]" >&2
  exit 2
fi

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
ui_root="$1"
definition="${2:-$repo_root/product/tests/fixtures/facereview-mysql/facereview.aip}"
if [[ ! -f "$definition" ]]; then
  echo "AIP definition input is not a file: $definition" >&2
  exit 2
fi
expected_revision="ab0386c878a5ba5c7c079a262b987c10d211d22d"
actual_revision="$(git -C "$ui_root" rev-parse HEAD)"
if [[ "$actual_revision" != "$expected_revision" ]]; then
  echo "pinned admin UI revision mismatch: expected $expected_revision, got $actual_revision" >&2
  exit 1
fi
if [[ -n "$(git -C "$ui_root" status --porcelain)" ]]; then
  echo "admin UI checkout must be clean before applying the overlay" >&2
  exit 1
fi

shared="$repo_root/porting/facereview/ui-overlay/src/api"
glue="$repo_root/porting/facereview/ui-overlay/admin/src/api"
mkdir -p "$ui_root/src/api"
# Pure adapters and the transport shared with the member overlay; they import nothing from a UI tree.
for file in adminAdapter.ts adminReadAdapter.ts authAdapter.ts index.ts; do
  cp "$shared/$file" "$ui_root/src/api/$file"
done
# Files that replace the axios modules of the admin UI and import its auth store.
for file in aipClient.ts admin.ts auth.ts HeaderToken.ts; do
  cp "$glue/$file" "$ui_root/src/api/$file"
done

mkdir -p "$ui_root/src/aip"
"${AIP_CLI_BIN:-aip}" service gen --lang ts --out "$ui_root/src/aip/facereview.ts" \
  --sdk-import @aip/sdk \
  "$definition"

echo "admin overlay applied to pinned UI $actual_revision at $ui_root using definition $definition"
