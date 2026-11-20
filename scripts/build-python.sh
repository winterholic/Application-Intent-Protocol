#!/usr/bin/env bash
# product/python의 wheel과 sdist를 만든다. 소스 디렉터리에는 아무것도 남기지 않는다(복사본에서 빌드).
#
# 네트워크가 필요 없는 경로:
#   --wheelhouse <dir>  setuptools>=77 wheel이 들어 있는 로컬 디렉터리. pip이 인덱스를 보지 않는다(PIP_NO_INDEX).
#   --no-isolation      현재 Python에 setuptools>=77이 이미 설치돼 있을 때. 격리 환경을 만들지 않는다.
# 둘 다 주지 않으면 python -m build의 기본 동작이며 setuptools를 인덱스에서 내려받는다(네트워크 필요).
# 어느 경로든 `build` 패키지(pip install build)가 현재 Python에 있어야 한다. `wheel` 패키지는 필요 없다.
# 사용: scripts/build-python.sh --out <새 디렉터리> [--wheelhouse <dir> | --no-isolation]
set -euo pipefail

out=""
wheelhouse=""
isolation=1
while [[ $# -gt 0 ]]; do
  case "$1" in
    --out) out="${2:?--out needs a directory}"; shift 2 ;;
    --wheelhouse) wheelhouse="${2:?--wheelhouse needs a directory}"; shift 2 ;;
    --no-isolation) isolation=0; shift ;;
    *) echo "usage: scripts/build-python.sh --out <fresh-dir> [--wheelhouse <dir> | --no-isolation]" >&2; exit 2 ;;
  esac
done
[[ -n "$out" ]] || { echo "--out is required" >&2; exit 2; }
[[ ! -e "$out" ]] || { echo "output directory already exists: $out" >&2; exit 2; }
if [[ -n "$wheelhouse" && $isolation -eq 0 ]]; then
  echo "--wheelhouse and --no-isolation are alternatives" >&2
  exit 2
fi
if [[ -n "$wheelhouse" && ! -d "$wheelhouse" ]]; then
  echo "wheelhouse is not a directory: $wheelhouse" >&2
  exit 2
fi

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
python="${PYTHON:-python3}"
"$python" -c "import build" 2>/dev/null || { echo "the 'build' package is required: $python -m pip install build" >&2; exit 1; }

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
cp -R "$root/product/python" "$work/src"
rm -rf "$work"/src/*.egg-info "$work/src/build" "$work/src/dist"
find "$work/src" -name __pycache__ -type d -prune -exec rm -rf {} +

if [[ -n "$wheelhouse" ]]; then
  PIP_NO_INDEX=1 PIP_FIND_LINKS="$(cd "$wheelhouse" && pwd)" "$python" -m build --outdir "$out" "$work/src"
elif [[ $isolation -eq 0 ]]; then
  "$python" -c "import setuptools, sys; sys.exit(0 if int(setuptools.__version__.split('.')[0]) >= 77 else 1)" \
    || { echo "--no-isolation needs setuptools>=77 in $python" >&2; exit 1; }
  "$python" -m build --no-isolation --outdir "$out" "$work/src"
else
  "$python" -m build --outdir "$out" "$work/src"
fi
