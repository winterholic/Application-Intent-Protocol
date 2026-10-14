# 일반·부분 수정 구현 기록

2026-10-14. 작업 위치 `/tmp/aip-wt-update`, 시작 `origin/main` 9de48e4. 다른 작업자의 UUID Id·DB 채택·검증 복구와 분리한다.

## 설계 관문

1. 원칙 1·2·3·4·7: 메모 편집 같은 CRUD의 값 검사·인가·원자성을 공통 엔진으로 옮긴다.
2. 기존 공개 전이에 `patch title, content`를 선언해 호출자가 해당 필드 값만 지정할 수 있다. 타입·길이·nullable는 resource fields의 선언을 재사용한다.
3. 앱마다 update DTO·endpoint·SQL을 만들 필요가 줄어든다.
4. rows read·allow는 변경 전 잠근 행으로 평가한다. 모르는 값 키·기본키 변경은 거부한다. 참조 변경은 생성과 같은 가시성 검사를 적용한다.
5. 다섯 작성 형식을 동일 AST/facts로 하강한다. TS/Python 호출은 기존 apply에 values·expect를 더한다.
6. 별도 update endpoint나 입력 타입 복제 없이 `patch` 하나로 수정 허용 필드를 선언한다. `expect version`은 호출자가 읽은 version과 현재 행의 같음을 요구하며 `to version = version + 1`과 조합해 낙관적 잠금을 만든다. 일반 조건부 갱신도 `expect title, content`로 같은 방식이다. expect가 없는 선언은 행 잠금 아래 마지막 쓰기가 이긴다.
7. 기존 전이·effect·notify·check·audit·멱등·재시도 transaction 경로를 확장한다. 기존 전이 facts와 생성 계약은 patch/expect가 없으면 보존한다.
8. 사용자 2026-10-14 일반 수정 구현 지시의 범위다. 최종 작성 언어·Id wire 선택은 닫지 않는다.

## 실행 의미

`transition edit { from deleted = false; allow owner = actor; patch title, content; expect version; to version = version + 1; notify owner "note.edited" }`

`apply({apply:"Note.edit",target:{ids:[id]},values:{title:"새 제목"},expect:{version:3}}, {key})`.

values는 비어 있지 않은 객체이며 patch allowlist의 부분집합이다. 누락은 유지, 명시적 null은 nullable만 허용한다. expect 선언은 모든 키를 요구한다. 조건은 인가 뒤 잠근 행에 대해 NULL-safe 비교한다. 불일치면 WIRE.CONFLICT, 전체 rollback. 같은 key 재전송은 기존 확정 결과를 재생한다. conflict는 최신 read 뒤 새 values/expect와 새 key로 요청한다. patch와 고정 to에 같은 필드를 넣거나 patch와 repeat unchanged를 조합하면 정의 오류다. 가려진 필드를 expect로 비교하는 추론 통로는 허용하지 않는다.

## 진행

- 지정 문서·포팅 보고서·최근 work-history 확인. PostgreSQL 17.11 연결 확인.
- 선언 parity 3개: patch 미지원 실패를 관측한 뒤 구현. 다섯 형식 facts·digest 일치, 기본키·중복·고정 to 중복·hidden/Ref expect·repeat·compose 입력 누락 거부.
- 실 PostgreSQL 통합 1개: apply values/expect 미지원 실패를 관측한 뒤 구현. 일반·부분·null·인가·타입/범위·Email/Date/Decimal·동시 충돌·effect/notify·check/bundle rollback 통과.
- 생성 계약 1개: values 타입 누락 실패 뒤 구현. patch/expect 타입·range 변경 지문 민감도 확인.
- 제품 포팅 1개: Python --lang 미지원 실패 뒤 구현. 원본 notes 복사본에 version/patch/expect 추가. 설치 @aip/sdk strict tsc 양성·음성, JWT 실제 서비스, 전체/부분/null·거부·conflict·멱등 재생·응답 유실 복구·계약 불일치·Python 생성 helper 통과. DB version/outbox는 `4|4`.
- 마지막 broad regression/rebase/push 전 checkpoint. 최신 원격에는 검증 복구·rate-limit 후속 변경이 들어와 있으므로 commit 뒤 `git pull --rebase origin main`으로 합친 뒤 전체 검증한다. UUID 작업은 아직 원격 main에 없는 상태였음(조회 시점).
- 지원 한계: compose parameter 입력 슬롯은 확장하지 않는다(bundle.apply 사용). expect Ref/field read 가림 필드는 비교 금지. Python 생성은 공개 apply TypedDict/helper 범위. 실제 운영 Memoir DB·worker sandbox 경로는 이 기능 시험의 대상이 아니다.

## 로컬 회귀 기록

- V1 134, V2 99, V3 30, V5 41 테스트: 실패 0. 각 manifest의 `cargo test --locked --target-dir target`을 실행했다.
- workspace 382 테스트(기존 ignored 4, macOS worker operation 1개는 환경 때문에 제외), PoC CLI 106 테스트(같은 worker operation 제외): 실패 0. 제외 없이 `bash tools/verify.sh`를 먼저 실행했고 해당 worker 기동에서 멈췄다. 직접 `sandbox-exec`도 exit 71 `sandbox_apply: Operation not permitted`라 같은 최신 main의 환경 제약이다. 런타임 격리는 변경하지 않았다.
- V6의 worker 없는 HTTP/전이/원자 묶음/Id/오류 코드 테스트 14개: 실패 0. audit의 worker 포함 통합은 같은 sandbox probe 실패이며, 새 CRUD 시험에서는 worker 없는 실제 PG 감사 before/after를 직접 확인했다.
- 정본 verify.sh의 Node 목록(새 포팅 포함) 125개, Python 24개: 실패 0. npm의 기존 root-owned 캐시 대신 `/tmp` 전용 cache를 쓰는 임시 npm wrapper로 실행했다(제품 코드·HOME 권한은 변경하지 않음).
- PoC와 제품 CLI는 같은 target binary를 쓰므로 순차 실행해야 한다. 처음 겹친 실행의 `RUNTIME_FEATURE_DISABLED`를 확인하고 PoC를 순차 재실행해 위 결과를 얻었다. Rust 통합 harness가 서버 주소를 주는 Node fixture들은 직접 wildcard로 실행할 목록이 아니므로 정본 목록으로 재실행했다.
- root verify의 fmt/clippy 관문은 통과했다. 추가 V5 lib clippy 통과. V1 단독 clippy는 기존 hmap.rs/enrich.rs 경고 2개, V3 단독은 기존 needless_borrow 경고 1개가 있어 별도 기록한다(새 update 함수의 인자 수 경고는 기존 judge와 같은 국소 allow로 정리). V3는 기존 경고만 제외한 `-D warnings -A clippy::needless_borrow`로 통과.
- 생성 문법 카드 21개·wire registry 6개 테스트 통과. plan-docs 검사기는 `/tmp` worktree에서 외부 AriAri 상대 링크 12개를 못 찾으므로 원래 폴더 깊이·읽기 전용 원본 링크를 복원한 임시 문서 복사본에서 다시 검사했다.
- 원격에 후속 Kafka 진단 커밋이 들어와 있으므로 마지막 rebase 후 기능 시험을 다시 실행하고 요청한 `git push origin HEAD:main`으로 올린다. 전체 Linux CI는 push된 commit의 결과를 별도로 확인한다.

## 최종 결과

코드 commit `749527f1f8a222f9b91b4c1831be665e356586fa`를 main에 push했다. 마지막 rebase에서 다른 작업자의 Kafka 진단 변경까지 보존했고 충돌은 없었다. rebase 뒤 실제 PG CRUD와 설치 SDK/Python 포팅 시험을 다시 실행해 모두 통과했다.

[Verify #38512660261](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/38512660261): `completed / success`. Ubuntu·PostgreSQL 17에서 Linux worker 격리·후손 정리와 **전체 `bash tools/verify.sh`**를 제외 없이 통과했다. 마지막 출력은 `AIP core verification passed.`이며 새로운 memoir TS/Python 시험도 포함한다. 로컬 macOS sandbox 제한을 Linux 검증으로 구분해 닫았다. macOS Seatbelt 자체는 이번 환경에서 미실행이다.

다운로드용 포팅 복사본·TS/Python binding·설치 SDK tarball·검증 증거는 이번 세션 outbox의 `memoir-update-copy.zip`에 둔다. 원본 포팅 notes와 기존 DB는 보존했다. 초기 RED 시험이 남긴 본인 시험 schema도 정리했다.
