# UUID·외부 PostgreSQL 채택 진행 기록 (2026-10-14)

작업 worktree: `/tmp/aip-wt-uuid`, 기준 main `9de48e4`. 사용자 승인: 기본 숫자 Id·owned schema 유지, UUID·외부 운영 DB 경로 추가. 다른 작업자의 변경을 덮지 않고 detached worktree에서 작은 커밋을 rebase/push한다.

읽음: 사용자 지정 포팅 보고서·memoir-notes README/포팅물, README·PRINCIPLES·DECISIONS Id/adopt 관련 항목, V13 결과, migrate README/ADOPTION, service README, SDK, capability-gap, security audit. 과거 기억에서도 같은 사용자 결정을 확인. memory remember는 도구 승인 정책 때문에 거부되어 저장 못함(기존 기억에 결정 있음).

설계: `id: Uuid` → typed `Id<Resource>` + resource `idKind: uuid`; Ref/X.Id는 대상 종류를 따라간다. canonical 소문자 하이픈 UUID 문자열, native PG uuid, DB default gen_random_uuid(). 숫자 V13 그대로. 자연키는 종류별 파싱·캐스트·출력 어댑터 확장 지점으로 열되 미구현. 기존 앱의 임의 UUID 컬럼은 별도 후속 범위.

현재: V1 실패 테스트에서 UNKNOWN_TYPE Uuid 재현; V3 실제 PG 테스트에서 UUID 생성 결과 i64 decode 실패 재현 후 EntityId(Number/Uuid)로 전이·생성·compose 내부 변경 중. V1 uuid syntax/ordering 구현, V2 파싱/DDL, V3 enum 적용. 아직 미완성·미커밋·미검증. 테스트 문법 수정 필요(read/filter, execute 호출) 및 V6 durable Vec 타입 변경 필요. audit target_id를 text로 바꿨으므로 기존 bigint 감사 schema 호환 경계 검토 필요.

외부 채택 예정: 별도 `service introspect`/`adopt-external`, mapping JSON + deny-all 정의 초안. 별도 AIP schema에 security_invoker updatable view(테이블/컬럼 aliases + xmin)와 metadata/infra만 만든다. 기존 테이블에 DDL 안 함, defaults/FK/check 그대로. catalog snapshot 기반 타입/구조 검증, drift는 시작·ready 거부. 제한 DML role 검증 필수. prototype adopt 유지. 정책/서비스 설정/SDK 생성은 기존 경로 재사용. UUID actor 신원은 아직 설계 검토 중(현 Caller/auth handle i64 고정).

다음: UUID 엔진/SDK 계약 descriptor·검증 테스트 → 외부 catalog/mapping/채택 검증 → 실 serve+설치 SDK TS/Python 테스트 → 회귀 tools/verify.sh와 제외 suite → docs 결정/연결표 → 작은 커밋/rebase/push. 새 테스트 schema만 사용, .env/비밀 안 읽음.

## 후속 구현(진행 중, 검증 아직 미완)

- V3 전체 테스트 로그 `/tmp/aip-uuid-v3.log`: EntityId custom Debug로 기존 숫자 회귀를 유지하고 전체 V3 실행 종료0. V3 원형 helper/tuple assertion만 enum 타입에 맞게 조정. 신원 handle은 기존 i64 유지하되 UUID business actor를 별도 aip_actor_identities에 매핑, session public actor_resource_id는 원래 UUID. AuthenticatorImpl.with_resource_actor가 실제 business actor 존재를 토큰 조회/재검증 때 검사. 정책 Ctx.actor()는 handle→UUID 조회, 업무 FK 타입 UUID 그대로.
- UUID audit는 UUID 포함 배포만 text target_id DDL variant; 기존 숫자 배포 bigint 유지. CLI audit 조회·필터 UUID 대응 아직 남음. outbox source/recipient UUID 지원 아직 남음.
- `default database` 타입 suffix AST/parser/sema 도입. introspect가 실제 default 컬럼에 표시. 필수 create 필드 제외, 외부 create는 실제 INSERT 후 정책 평가(기본값/생성값 존중, 같은 transaction 내 실패 rollback). owned init는 이 modifier 거부. field 기본값은 사실 범위 type/ref 계약과 분리.
- 신규 `spike-v2-read::external` catalog snapshot/mapping, `aip-migrate::external` introspection/plan/apply/deployment_facts. external_mapping config·CLI introspect/adopt-external. 새 AIP schema에 security_invoker updatable alias view+xmin+metadata만 생성. 원본 SELECT만 가진 adopter role로 테스트 중. snapshot fingerprint는 원본의 제약·기본값·FK·trigger·함수 등 포함. view와 metadata도 기존 marker catalog preflight. 외부 mapping baseline은 저장된 facts에서 읽음(드리프트를 자동 재기준화하지 않음). `/read`·`/apply` 등 배포 fence 뒤 catalog drift 검사 추가. migrate/init/prototype adopt에 external_mapping이 있으면 DDL 거부. mapping 누락 설정으로 external schema migrate 우회되는지 parent migrate 단독 API 방어 보강 필요.
- TypeScript 생성 Id<Resource>는 UUID resource를 conditional str로 처리; scalar descriptor Uuid. Python `gen --out .py` NewType/TypedDict 생성. SDK TS UUID scalar/session/write 결과 검사 추가; Python session UUID 허용. numeric wire에 UUID 응답을 글로벌 validResultId로 무조건 허용한 상태여서 종류별 결과 검증 강화 필요.
- 통합 fixture `product/tests/uuid-external.test.mjs`: 매 실행 새 DB/role을 만든 후 finally 삭제. .env·비밀 사용 안 함. SDK build → offline pack/install → 생성 binding tsc → 실제 JWT service. external UUID users/notes·default/FK/unique·제한 DML role·SDK read/create/apply/replay/Python/cursor/drift 테스트 작성. 구 CLI `unrecognized subcommand introspect` 재현. 외부 UUID actor metadata 생성 실패 재현 후 handle 구현. 현재 test script의 sort(direction→dir)/cursor(after:{id}) 문법 보정해 실행 중(session 39514). 앞선 테스트 원형이 SDK tarball 아닌 디렉터리 빌드를 가정해 pack 누락→추가, .npm cache 밖 쓰기 실패→임시 cache로 해결. 현재 cargo build 통과.

남은 필수: 통합 test 통과시키고 owned UUID SDK 두 wire smoke 추가; ready/worker 경계 drift 적용 확인; FK/default/nullability/type 음성 대조; UUID Ref/compose/effects/audit/outbox 경계 테스트 및 일반 숫자 회귀; Core PoC IR은 아직 UUID 변경 없음(제품 execution facts만 적용); 안전한 자연키 extension 결정 기록; 문서/인프라 지원 표; 전체 tools/verify.sh fmt/clippy/locked(독립 spike Cargo.lock serde 반영 필요)·product smoke; 작은 commit·git pull --rebase origin main·push. 미커밋 상태이고 원본 checkout 건드리지 않음.

## 최신 검증·커밋(2026-10-14)

`node --test product/tests/uuid-external.test.mjs`: decimal/safe × external/owned 4구성이 통과(pass 1 fail 0, 3.39s). external UUID actor/Ref, 설치 TS SDK·Python, native UUID default·PG 제약·outbox notify·UUID audit·cursor·멱등 replay/mismatch·다른 사용자 거부·live/startup drift를 확인. owned UUID+설치 SDK create/read/apply도 통과. 시험DB/role은 finally에서 삭제. 숫자 resource에 잘못된 UUID 성공응답을 보내면 pending을 해제하지 않는 SDK 음성 대조도 변경 전 실패→변경 후 통과를 확인.

세 커밋: 43a06e5(정의), 7ac00fa(실행/SDK), 87da7f0(외부 채택/서비스). `cargo clippy -p aip-auth -p aip-migrate -p aip-service --all-targets --locked -- -D warnings` 종료0. 이제 concurrent origin/main rebase, 전체 회귀, 최종 문서가 필요. 전체 verify 미실행.

정정/현상태: Python read()는 envelope가 아니라 list를 반환. numeric audit schema는 bigint 그대로, UUID 혼합 schema만 target_id text. outbox source/recipient도 UUID 혼합 schema만 text, relay는 양쪽을 text로 읽는다. Python 타입은 gen --out .py. PoC Core IR은 원래 Type::Uuid가 있어 변경 없음, 제품 Id<Resource>+idKind를 추가. external marker에 대한 일반DDL migration 우회도 API에서 거부. IdKind 변경은 Unsupported. /ready도 외부 snapshot 확인. UUID 종류별 성공응답 검사 추가.

남은 것: external 같은 AIP schema의 정책 업데이트는 현재 미지원(fresh AIP schema만); 기존 numeric infra에 UUID 리소스를 새로 추가할 때 audit text 업그레이드 운영 처리가 필요. background durable guard drift 추가 여부 검토. 문서·외부 연결표·rebase 후 새 일반update 입력과 UUID/default 통합 확인. 현재 root docs 커밋 전.
