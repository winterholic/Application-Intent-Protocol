#!/usr/bin/env bash
# aip 작업 중 쌓이는 재생성 가능한 산출물을 정리한다. 기본은 dry-run이고 --apply를 줘야 지운다.
#
# 대상 (모두 aip 소유 경로만, 다른 프로젝트·모델 폴더는 건드리지 않는다)
#   1. .claude/worktrees/agent-* 와 <tmp>/aip-* 에 등록된 git worktree
#   2. worktree가 아닌 <tmp>/aip-* 디렉터리 (git 정보 없음)
#   3. <repo>/target 과 <repo>/spikes/*/target
#
# 절대 지우지 않는 것: 병합 안 된 커밋, 잠긴 worktree, 열린 파일·실행 중 프로세스가 있는 경로,
# 최근 수정된 경로, 판단이 애매한 경로. 사유는 SKIP 로그로 남긴다.
set -euo pipefail

# cron은 PATH가 좁아서 lsof(/usr/sbin)를 못 찾는다
export PATH="/usr/sbin:/usr/bin:/bin:/opt/homebrew/bin:/usr/local/bin:$PATH"

apply=0
worktree_hours="${AIP_CLEANUP_WORKTREE_HOURS:-72}"
target_days="${AIP_CLEANUP_TARGET_DAYS:-7}"
tmp_dir="${AIP_CLEANUP_TMP_DIR:-/private/tmp}"

usage() {
    cat <<'EOF'
usage: scripts/cleanup-disk.sh [--apply] [--worktree-hours N] [--target-days M] [--tmp-dir DIR]

  --apply             실제로 삭제한다. 없으면 목록과 크기만 출력한다 (dry-run).
  --worktree-hours N  worktree와 tmp 폴더가 N시간 넘게 수정되지 않아야 지운다. 기본 72.
  --target-days M     target이 M일 넘게 수정되지 않아야 지운다. 기본 7.
  --tmp-dir DIR       tmp 폴더 위치. 기본 /private/tmp.

환경변수 AIP_CLEANUP_WORKTREE_HOURS, AIP_CLEANUP_TARGET_DAYS, AIP_CLEANUP_TMP_DIR로도 조정할 수 있다.
EOF
}

die() {
    echo "error: $*" >&2
    exit 2
}

while [ $# -gt 0 ]; do
    case "$1" in
        --apply) apply=1 ;;
        --dry-run) apply=0 ;;
        --worktree-hours) worktree_hours="${2:?--worktree-hours needs a value}"; shift ;;
        --target-days) target_days="${2:?--target-days needs a value}"; shift ;;
        --tmp-dir) tmp_dir="${2:?--tmp-dir needs a value}"; shift ;;
        -h | --help) usage; exit 0 ;;
        *) usage >&2; die "unknown option: $1" ;;
    esac
    shift
done

for v in "$worktree_hours" "$target_days"; do
    case "$v" in '' | *[!0-9]*) die "hours/days must be a non-negative integer: '$v'" ;; esac
done
[ -d "$tmp_dir" ] || die "tmp dir not found: $tmp_dir"
# git과 lsof는 심볼릭 링크를 푼 경로(/private/tmp)로 보고하므로 같은 형태로 맞춘다
tmp_dir="$(cd "$tmp_dir" && pwd -P)"

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_hint="$(cd "$script_dir/.." && pwd)"
# worktree 안에서 실행해도 항상 메인 worktree를 기준으로 삼는다
main_repo="$(git -C "$repo_hint" worktree list --porcelain | awk '/^worktree /{sub(/^worktree /, ""); print; exit}')"
[ -n "$main_repo" ] && [ -d "$main_repo/.git" ] || die "cannot locate main worktree from $repo_hint"

# origin/main은 마지막 fetch 기준이다. 오래됐으면 병합을 못 알아볼 뿐 잘못 지우지는 않는다.
merge_refs=""
for ref in refs/heads/main refs/remotes/origin/main; do
    if git -C "$main_repo" rev-parse --verify -q "$ref" >/dev/null; then
        merge_refs="$merge_refs $ref"
    fi
done
[ -n "$merge_refs" ] || die "neither main nor origin/main exists in $main_repo"

lock_dir="${TMPDIR:-/tmp}/cleanup-disk-aip.lock"
if ! mkdir "$lock_dir" 2>/dev/null; then
    old_pid="$(cat "$lock_dir/pid" 2>/dev/null || true)"
    if [ -n "$old_pid" ] && kill -0 "$old_pid" 2>/dev/null; then
        die "another cleanup is running (pid $old_pid)"
    fi
    rm -rf "$lock_dir"
    mkdir "$lock_dir"
fi
echo $$ >"$lock_dir/pid"

open_list="$(mktemp)"
ps_list="$(mktemp)"
registered="$(mktemp)"
trap 'rm -f "$open_list" "$ps_list" "$registered"; rm -rf "$lock_dir"' EXIT

# 열린 파일(cwd 포함)과 실행 중 명령줄을 한 번만 수집한다. 트리마다 lsof +D를 돌리면 target에서 너무 느리다.
lsof -Fn >"$open_list" 2>/dev/null || true
[ -s "$open_list" ] || die "lsof returned nothing; refusing to judge in-use state"
ps -axo command >"$ps_list"

git -C "$main_repo" worktree list --porcelain | awk '/^worktree /{sub(/^worktree /, ""); print}' >"$registered"

total_kb=0
skipped=0
removed=0

log() { echo "$(date '+%Y-%m-%d %H:%M:%S') $*"; }
skip() {
    skipped=$((skipped + 1))
    log "SKIP   $1  ($2)"
}
human() { awk -v k="$1" 'BEGIN{ printf "%.1fG", k / 1048576 }'; }
size_kb() { du -sk "$1" 2>/dev/null | awk '{print $1}'; }

# 경로 자체이거나 그 하위 경로가 열려 있으면 사용 중이다
in_use() {
    local p="$1"
    if awk -v p="$p" 'BEGIN{n = length(p)}
        substr($0, 1, 1) == "n" { s = substr($0, 2); if (s == p || substr(s, 1, n + 1) == p "/") { f = 1; exit } }
        END{ exit !f }' "$open_list"; then
        return 0
    fi
    grep -qF -- "$p" "$ps_list" && return 0
    case "$p" in
        /private/tmp/*) grep -qF -- "${p#/private}" "$ps_list" && return 0 ;;
    esac
    return 1
}

# 최근 수정 여부. .git, node_modules, target은 빼고 소스 변경만 본다.
recently_modified() {
    [ -n "$(find "$1" \( -name .git -o -name node_modules -o -name target \) -prune -o -type f -mmin "-$2" -print -quit 2>/dev/null)" ]
}

is_merged() {
    local head="$1" ref
    for ref in $merge_refs; do
        git -C "$main_repo" merge-base --is-ancestor "$head" "$ref" 2>/dev/null && return 0
    done
    return 1
}

remove_dir() {
    local d="$1"
    case "$d" in '' | *..*) return 1 ;; esac
    [ -d "$d" ] && [ ! -L "$d" ] || return 1
    rm -rf -- "$d"
}

account() {
    # $1=kb $2=label $3=path
    total_kb=$((total_kb + $1))
    removed=$((removed + 1))
    if [ "$apply" -eq 1 ]; then
        log "DELETE $(human "$1")  $3  ($2)"
    else
        log "WOULD  $(human "$1")  $3  ($2)"
    fi
}

worktree_minutes=$((worktree_hours * 60))
target_minutes=$((target_days * 1440))

log "mode=$([ "$apply" -eq 1 ] && echo apply || echo dry-run) worktree_hours=$worktree_hours target_days=$target_days tmp_dir=$tmp_dir repo=$main_repo"
log "disk before: $(df -h "$main_repo" | awk 'NR==2{print "used " $3 ", avail " $4 " (" $5 ")"}')"

# ---- 1. git worktree ----
log "== 1. git worktrees"
while IFS=$'\t' read -r -u 3 wt_path wt_ref wt_locked; do
    [ "$wt_path" = "$main_repo" ] && continue
    case "$wt_path" in
        "$main_repo"/.claude/worktrees/agent-* | "$tmp_dir"/aip-*) ;;
        *) continue ;;
    esac
    if [ "$wt_locked" = "1" ]; then skip "$wt_path" "잠긴 worktree"; continue; fi
    if [ ! -d "$wt_path" ]; then skip "$wt_path" "경로 없음, git worktree prune 대상"; continue; fi
    # 빈 필드가 탭 IFS에서 합쳐지지 않도록 브랜치 없음(detached)은 "-"로 넘어온다
    head="$(git -C "$wt_path" rev-parse HEAD 2>/dev/null)" || { skip "$wt_path" "HEAD 확인 실패"; continue; }
    if ! is_merged "$head"; then skip "$wt_path" "main/origin 에 병합 안 됨 $([ "$wt_ref" = "-" ] && echo detached || echo "${wt_ref#refs/heads/}")@${head:0:7}"; continue; fi
    # status가 인덱스를 갱신하면 gitdir 수정 시각이 바뀌어 스스로 recent 판정을 만들므로 optional lock을 끈다
    status_out="$(git -C "$wt_path" --no-optional-locks status --porcelain 2>/dev/null)" || { skip "$wt_path" "git status 실패"; continue; }
    dirty="$(printf '%s\n' "$status_out" | grep -vE '^$|(^|[ /])(node_modules|target)(/|$)' || true)"
    if [ -n "$dirty" ]; then skip "$wt_path" "미커밋 변경 $(printf '%s\n' "$dirty" | wc -l | tr -d ' ')건"; continue; fi
    gitdir="$(git -C "$wt_path" rev-parse --absolute-git-dir 2>/dev/null || true)"
    if recently_modified "$wt_path" "$worktree_minutes" \
        || { [ -n "$gitdir" ] && [ -n "$(find "$gitdir/HEAD" "$gitdir/index" -mmin "-$worktree_minutes" -print -quit 2>/dev/null)" ]; } \
        || { [ -d "$wt_path/target" ] && [ -n "$(find "$wt_path/target" -maxdepth 2 -mmin "-$worktree_minutes" -print -quit 2>/dev/null)" ]; }; then
        skip "$wt_path" "${worktree_hours}시간 안에 수정됨"
        continue
    fi
    if in_use "$wt_path"; then skip "$wt_path" "사용 중 (열린 파일 또는 프로세스)"; continue; fi

    kb="$(size_kb "$wt_path")"
    branch="${wt_ref#refs/heads/}"
    if [ "$apply" -eq 1 ]; then
        # 위에서 node_modules/target 외 변경이 없음을 확인했으므로 --force는 그 둘 때문이다. 잠금은 -f 한 번으로는 못 푼다.
        if git -C "$main_repo" worktree remove --force "$wt_path"; then
            account "$kb" "병합된 worktree" "$wt_path"
            if [ "$wt_ref" != "-" ] && [ "$branch" != "main" ] && [ "$branch" != "master" ]; then
                git -C "$main_repo" branch -d "$branch" >/dev/null 2>&1 \
                    && log "BRANCH deleted $branch" \
                    || skip "branch $branch" "git branch -d 거부, 유지"
            fi
        else
            skip "$wt_path" "git worktree remove 실패"
        fi
    else
        account "$kb" "병합된 worktree" "$wt_path"
        if [ "$wt_ref" != "-" ]; then log "WOULD  git branch -d $branch"; fi
    fi
done 3< <(git -C "$main_repo" worktree list --porcelain | awk 'BEGIN{RS=""; FS="\n"}
    { p = ""; b = "-"; l = 0
      for (i = 1; i <= NF; i++) {
          if ($i ~ /^worktree /) p = substr($i, 10)
          else if ($i ~ /^branch /) b = substr($i, 8)
          else if ($i ~ /^locked/) l = 1
      }
      printf "%s\t%s\t%d\n", p, b, l }')

if [ "$apply" -eq 1 ]; then
    git -C "$main_repo" worktree prune
    log "git worktree prune done"
else
    git -C "$main_repo" worktree prune --dry-run -v | sed 's/^/                    prune: /' || true
fi

# ---- 2. worktree가 아닌 tmp/aip-* ----
log "== 2. $tmp_dir/aip-* (git 정보 없는 폴더)"
for d in "$tmp_dir"/aip-*; do
    [ -e "$d" ] || continue
    [ -d "$d" ] && [ ! -L "$d" ] || continue
    if grep -qxF -- "$d" "$registered"; then continue; fi
    if [ -e "$d/.git" ]; then skip "$d" "git 정보 있음, worktree 아님"; continue; fi
    if [ -n "$(find "$d" -mmin "-$worktree_minutes" -print -quit 2>/dev/null)" ]; then
        skip "$d" "${worktree_hours}시간 안에 수정됨"
        continue
    fi
    if in_use "$d"; then skip "$d" "사용 중 (열린 파일 또는 프로세스)"; continue; fi
    kb="$(size_kb "$d")"
    if [ "$apply" -eq 1 ]; then
        if remove_dir "$d"; then account "$kb" "git 없는 tmp 폴더" "$d"; else skip "$d" "삭제 실패"; fi
    else
        account "$kb" "git 없는 tmp 폴더" "$d"
    fi
done

# ---- 3. target ----
log "== 3. cargo target (미사용 ${target_days}일 초과)"
for t in "$main_repo/target" "$main_repo"/spikes/*/target; do
    [ -d "$t" ] && [ ! -L "$t" ] || continue
    if ! git -C "$main_repo" check-ignore -q "$t"; then skip "$t" "git ignore 대상이 아님"; continue; fi
    if [ -n "$(find "$t" -mmin "-$target_minutes" -print -quit 2>/dev/null)" ]; then
        skip "$t" "${target_days}일 안에 사용됨"
        continue
    fi
    if in_use "$t"; then skip "$t" "사용 중 (열린 파일 또는 프로세스)"; continue; fi
    kb="$(size_kb "$t")"
    if [ "$apply" -eq 1 ]; then
        if remove_dir "$t"; then account "$kb" "오래된 target" "$t"; else skip "$t" "삭제 실패"; fi
    else
        account "$kb" "오래된 target" "$t"
    fi
done

log "== summary: $([ "$apply" -eq 1 ] && echo deleted || echo 'would delete') $removed items, $(human "$total_kb"); skipped $skipped"
log "disk after:  $(df -h "$main_repo" | awk 'NR==2{print "used " $3 ", avail " $4 " (" $5 ")"}')"
