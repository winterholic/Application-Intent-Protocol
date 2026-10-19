# F13 CLI 대조: 원본 FlaskBB CLI → AIP service CLI (v0.1)

원본: `upstream/flaskbb/cli/{main,users,translations}.py` + `flaskbb db`(alembic).
AIP: `aip service <cmd>` (product path). 2026-10-18 후속 실행에서 `cli.test.mjs`가
격리 PG에서 init·seed·migrate·principal·audit를 실행했다(아래 표 상태 갱신, 증거
`evidence/run-752-cli.log`). jobs는 `mail-jobs.test.mjs`가 실행한다.

| 원본 명령 | 의미 | AIP 대응 | 상태 |
|---|---|---|---|
| `flaskbb install` | schema 생성+기본 그룹/설정 seed | `service init` + 선언 seed 스크립트 | 실행됨: `service init` + `seed/install.py`(upstream fixture 그룹·설정·admin·welcome), 재설치는 거부 |
| `flaskbb populate --test-data/--bulk-data` | fixture/대량 seed | 테스트 harness의 SQL seed (fixture 전용) | tests 내 실행, 격리 schema |
| `flaskbb db` (alembic) | 마이그레이션 | `service migrate`(계획) / `--apply <digest>` | 실행됨: 무변경 계획, Safe 필드 추가 계획, 미승인 digest 거부, 적용 뒤 행 보존 |
| `flaskbb upgrade` | `--all`: `alembic.upgrade()` 후 fixture 갱신, `--fixture NAME`: 설정 fixture 갱신(`--force`는 덮어쓰기) (`upstream/flaskbb/cli/main.py:232-263`, 갱신 본체 `utils/populate.py:87-145`) | 마이그레이션 절반은 `service migrate`(계획)/`--apply <digest>`. fixture 갱신 절반은 대응 없음: `Setting`·`SettingsGroup`에 쓰기 전이가 없고 `seed/install.py`는 설치 전용 | 마이그레이션 절반 실행됨(`cli.test.mjs`, `cli-upgrade.test.mjs`: Safe 필드 추가 적용 뒤 settings 행 불변). fixture 절반은 제품 갭 G-15, `cli-upgrade.test.mjs`가 현재 상태를 고정 |
| `flaskbb users new/update` | 사용자 생성/수정 | `service principal bind/revoke` + `User.adminUpdateUser`/`moderatorEditUser` | bind·revoke 실행됨 |
| `flaskbb users delete -u NAME [-f]` | 정확한 username 조회 → 없으면 오류 → `-f` 없으면 확인 → `User.delete()`(`session.delete`+commit) (`upstream/flaskbb/cli/users.py:67-89`, `user/models.py:465-468`) | `service` CLI에는 전이 실행 명령이 없다. admin 신원을 `principal bind`한 호출자가 `User` read(`username.prefix` 뒤 정확 일치 선별)로 id를 찾고 `User.purge`(G-11)를 적용한다. 확인 질문은 호출자 몫 | 실행됨: `cli-users-delete.test.mjs`(조회·없는 이름·접두 불일치·삭제·재실행·감사). 차이: (1) 원본 CLI는 권한 검사가 없고 AIP는 admin 활성 계정·자기 제외를 요구, (2) 참조 처리 차이는 G-11 그대로, (3) `principal bind`된 계정(revoke해도 행이 남음)은 `aip_principals.actor_id` FK 때문에 삭제 불가이며 오류도 재시도 가능한 미확정 쓰기로 보인다(G-14) |
| `flaskbb celery` | 비동기 worker 기동 | serve 내장 durable jobs + `service jobs list/retry` | 기존 실행 근거 있음. 연결 전 실패의 같은 run retry는 Linux CI 재검증 대기; 미확정 효과는 계속 거부(G-2) |
| `flaskbb shell` | 앱 컨텍스트 shell | `service audit list`/`service jobs list` 조회 | 실행됨. audit는 전이만 기록, create는 기록 없음 |
| `flaskbb urls` | route 목록 | `service gen` 바인딩 + INVENTORY route map | `gen` 실행됨 (지문 기록) |
| `flaskbb makeconfig` | 설정 생성 | service.json + env 문서화 | 문서만 |
| translations new/update/compile | i18n 카탈로그 | 제품 범위 밖 (F16과 함께 미선언) | 갭 |
| 검색 reindex | Whoosh 색인 재생성 | 대응 없음: `contains` 필터는 색인 없이 항상 일관 (F15 행 참조) | 설계상 불필요, 순위검색은 갭 |

## 비-DB 실행 증거 (이 세션)

- `aip service check porting/flaskbb/app.aip` → `ok:true`
  (executionDigest `6ffbbab6...`, offset/cursor 예산·last-post update·mail job 포함).
- `aip service gen porting/flaskbb/app.aip --out /tmp/…` → `ok:true`,
  20개 read descriptor (fingerprint `873e35e6…`, idWire `decimal-string-v13`).
- `service check` executionDigest `6ffbbab6…` (offset/cursor·last-post·mail job 포함).
- `aip service --help`의 init/migrate/serve/principal/jobs/audit는 DB 없이
  호출하지 않았으므로 실행 미실행.

## 열린 항목

- clean checkout 배포 artifact(패키징)에서의 재현. 격리 schema에서의 `init → seed → serve → migrate`는 `cli.test.mjs`로 실행됨.
- `users delete`의 AIP 전이 선언 여부 (원본은 물리 삭제; tombstone 정책 결정 필요).
- translations/i18n은 F16 범위와 함께 미정.
