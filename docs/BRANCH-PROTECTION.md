# `main` 브랜치 보호 권장 설정

2026-11-21 기준 `main`은 보호되어 있지 않다(`gh api repos/winterholic/Application-Intent-Protocol/branches/main/protection` → 404). 빨간 체크 상태로 병합된 PR이 있었다(#1, #2, #6). 이 문서는 **제안**이며 설정 변경은 저장소 소유자가 직접 한다.

## 권장 규칙 (Settings → Branches → Branch protection rule, `main`)

| 설정 | 값 | 이유 |
|---|---|---|
| Require a pull request before merging | 켬 (승인 0~1명) | 개인 저장소라 승인 수는 선택. PR 경로를 강제해 체크를 거치게 한다 |
| Require status checks to pass | 켬 | 아래 필수 체크 |
| Require branches to be up to date | 켬 | 병합 결과 SHA에서 체크가 돌게 한다 |
| Do not allow bypassing the above settings | 켬 | 소유자 우회 병합 방지 |
| Block force pushes / Restrict deletions | 켬 | 이력 보존 |
| Require linear history | 선택 | 기존 이력 방식에 맞춘다 |

## 필수 체크 후보

경로 필터가 없는 workflow의 job만 필수로 지정할 수 있다. 경로 필터 workflow를 필수로 지정하면 해당 경로를 건드리지 않은 PR이 영원히 "대기" 상태가 된다.

| 필수 지정 | 대상 |
|---|---|
| PR마다 지정 | `Verify`, `App packages`, `External brokers`, `Object storage`, `Linux cgroup v2` (경로 필터 없음) |
| PR에서는 지정하지 않음 | `database-contracts`, `worker-jobs-linux`, `flaskbb-*`, `deploy-checks`, `dependencies`, `AIP 1.0 release gate` (경로 필터) |
| 출시 SHA에서만 | `release-candidate`의 `required (all workflows green on this SHA)` |

`release-candidate`는 `release/**` 브랜치와 `v*` 태그, 수동 실행에서 돌며 위 경로 필터 workflow를 전부 호출한다. 릴리스 브랜치로 향하는 PR의 필수 체크로 이 job을 지정하면 경로 필터 때문에 안 도는 검사가 없다.

## 태그 보호

Settings → Tags에서 `v*` 패턴의 tag protection(또는 ruleset)을 켜서 태그 생성·삭제를 소유자로 제한한다.

## 병합 방식

squash 병합을 권장한다(체크 SHA와 병합 SHA가 달라지는 문제를 줄이고 이력을 단순하게 한다). 병합 직후 `main` push 실행이 0건이면(PR #11의 `d327b264`가 그랬다) `gh workflow run <workflow> --ref main`으로 누락된 workflow를 수동 실행한다.
