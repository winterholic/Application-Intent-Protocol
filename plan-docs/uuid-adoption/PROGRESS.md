# UUID·기존 PostgreSQL 채택 진행 상태

2026-10-14, `/tmp/aip-wt-uuid` detached worktree. 사용자 승인과 범위는 DESIGN.md, 검증은 RESULTS.md, 외부 인프라 목록은 INFRASTRUCTURE-CONNECTIONS.md에 있다. 이 기록은 이전 중간 상태를 대체한다. 과거 상태/실패는 git 이력에 남아 있다.

## 현재

- UUID 기본키와 Ref/X.Id·제품 IR·TS/Python 생성·native SQL·필터/정렬/커서·SDK/pending·캐시·멱등·신원·audit/outbox를 연결했다. 숫자 wire는 유지한다.
- introspect→deny-all 정의/매핑→별도 AIP schema view/metadata→제한 DML serve·SDK→drift 거부를 실 PG에서 확인했다. 원본에 DDL하지 않는다.
- 같은 매핑의 정책 재배포·plan 재시도, UUID actor 자기 참조, numeric 앱에 UUID resource 추가/Ref backfill과 audit 업그레이드도 검증했다.
- 다른 작업자의 patch/expect와 Python 생성 기능을 rebase로 보존했다. Python의 기존 `gen --lang python` 경로에 UUID 타입을 통합했다.
- 기본 원칙/결정 D-A31·D-A32와 명령/권한/제약, 전체 외부 연결 표를 갱신했다.
- workspace(환경상 worker 1개 제외), PoC, V1/V2/V3/V5, V6 library, SDK 127개, Python 24개와 실 DB 4구성 통과. local 전체 verify는 macOS sandbox/pro세스 관리 권한 때문에 일부 worker 경로가 미검증이다. 상세 숫자와 한계는 RESULTS.md.

## 커밋과 다음 단계

`a3ad7d3` 정의, `073f30d` runtime/SDK, `f659595` 외부 서비스, `ffe924f` 중간 기록, `76d49a3` numeric/update/Python/migration/재배포 통합, `d85a898` durable drift, `5e3b633` verify 통합. 이 hash는 다음 rebase에서 바뀔 수 있다.

남은 단계는 문서 커밋→`git pull --rebase origin main`→`git push origin HEAD:main`→자동 Linux Verify 확인·최종 결과 기록이다. push는 사용자 명시 승인 범위다. 원본 checkout은 수정하지 않았다. `.env`·비밀을 읽지 않았다. memory remember는 승인 정책으로 거부됐지만 동일 사용자 결정은 기존 기억에 있고 정본 문서에도 저장했다.
