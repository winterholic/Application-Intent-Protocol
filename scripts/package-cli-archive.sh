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
mkdir -p "$stage/bin" "$stage/extensions" "$stage/tools"
cp "$bundle/bin/aip" "$stage/bin/aip"
cp "$bundle/config.example.json" "$bundle/example.aip" "$stage/"
# $bundle/README.md는 sdk/, SHA256SUMS를 안내하는 번들용이라 쓰지 않고, 아카이브 내용에 맞춘 전용 README를 넣는다.
cp "$root/product/CLI-ARCHIVE-README.md" "$stage/README.md"
cp "$bundle/extensions/event.mjs" "$bundle/extensions/event.py" "$stage/extensions/"
cp "$bundle/tools/dev-jwt.mjs" "$stage/tools/"
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
# 개발용 토큰 도구가 checkout 없이 동작하는지 확인한다.
(cd "$verify" && node "$name/tools/dev-jwt.mjs" --dir ./dev-idp --subject user-42 | awk -F. 'NF==3 { found=1 } END { exit !found }')
test -f "$verify/dev-idp/jwks.json"
echo "{\"ok\":true,\"archive\":\"$name.tar.gz\",\"version\":\"$version\",\"target\":\"$target\"}"
