# F13 CLI 대조: 원본 FlaskBB CLI → AIP service CLI (v0.1)

원본: `upstream/flaskbb/cli/{main,users,translations}.py` + `flaskbb db`(alembic).
AIP: `aip service <cmd>` (product path). 실행 확인은 격리 PG가 필요하므로
DB-backed 항목은 미실행이며, 선언 검증(`check`/`gen`)만 통과 상태다.

| 원본 명령 | 의미 | AIP 대응 | 상태 |
|---|---|---|---|
| `flaskbb install` | schema 생성+기본 그룹/설정 seed | `service init` + 선언 seed 스크립트 | 선언 있음, 실행 미실행 |
| `flaskbb populate --test-data/--bulk-data` | fixture/대량 seed | 테스트 harness의 SQL seed (fixture 전용) | tests 내 실행, 격리 schema |
| `flaskbb db` (alembic) | 마이그레이션 | `service migrate --plan/--apply` | 선언 있음, 실행 미실행 |
| `flaskbb upgrade` | 코드/데이터 업그레이드 | `service migrate` + 배포 artifact | 실행 미실행 |
| `flaskbb users new/change/delete` | 사용자 생성/수정/삭제 | `service principal bind` + `User.adminUpdateUser`/`moderatorEditUser`; User 물리 삭제 전이 없음 | 부분 (삭제 미선언) |
| `flaskbb celery` | 비동기 worker 기동 | serve 내장 durable jobs + `service jobs` | 선언 있음, 실행 미실행 |
| `flaskbb shell` | 앱 컨텍스트 shell | `service audit`/`service jobs` 조회 | 읽기 전용 대응, 실행 미실행 |
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

- clean checkout에서의 `init → migrate → seed → serve` 전 과정 재현 (DB 필요).
- `users delete`의 AIP 전이 선언 여부 (원본은 물리 삭제; tombstone 정책 결정 필요).
- translations/i18n은 F16 범위와 함께 미정.
