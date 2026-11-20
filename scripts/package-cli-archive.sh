#!/usr/bin/env bash
# product/tools/package.mjs가 만든 묶음에서 플랫폼별 CLI 아카이브 aip-<version>-<target>.tar.gz를 만들고,
# 풀어서 aip --version이 Cargo 버전과 같은지, service check가 통과하는지 확인한다.
# 사용: scripts/package-cli-archive.sh <bundle-dir> <target-triple> <out-dir>
set -euo pipefail

bundle="${1:?bundle dir}"
target="${2:?target triple}"
out="${3:?out dir}"
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
version="$(node "$root/product/tools/version.mjs" version)"
name="aip-$version-$target"

mkdir -p "$out"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

stage="$work/$name"
mkdir -p "$stage/bin" "$stage/extensions"
cp "$bundle/bin/aip" "$stage/bin/aip"
cp "$bundle/config.example.json" "$bundle/example.aip" "$bundle/README.md" "$stage/"
cp "$bundle/extensions/event.mjs" "$bundle/extensions/event.py" "$stage/extensions/"
cp "$root/LICENSE" "$root/product/INSTALL.md" "$root/COMPATIBILITY.md" "$stage/"
chmod 0755 "$stage/bin/aip"

tar -C "$work" -czf "$out/$name.tar.gz" "$name"

# 풀어서 실제로 동작하는지 확인한다.
verify="$work/verify"
mkdir "$verify"
tar -C "$verify" -xzf "$out/$name.tar.gz"
reported="$("$verify/$name/bin/aip" --version)"
if [[ "$reported" != "aip $version" ]]; then
  echo "aip --version이 'aip $version'이 아닙니다: $reported" >&2
  exit 1
fi
"$verify/$name/bin/aip" service check "$verify/$name/example.aip" >/dev/null
echo "{\"ok\":true,\"archive\":\"$name.tar.gz\",\"version\":\"$version\",\"target\":\"$target\"}"
