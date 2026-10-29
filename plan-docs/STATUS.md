# 최신 재개 지점 (2026-10-30, durable rekey 재시작 커서 보강)

로컬 `main` SHA `26de75059c048690abb7d78101d8cbf94dfc4fc3`에서 lease의 phase/pass/cursor, old·active DEK id/version, 남은 old 셀, 실패 수를 DB에 저장한다. 세대가 바뀌면 커서를 초기화한다. 각 행 CAS와 커서 저장은 같은 transaction이고, lease 갱신·해제·양보는 holder/epoch/두 키 세대가 일치하는 경우로 fence했다. 재시작 후 cursor 1부터 이어가는 Postgres 통합 테스트를 추가했고 `worker-jobs-linux`의 Postgres 17 job에 연결했다.

로컬 증거: `cargo fmt --all -- --check`, 비DB `encryption_rekey` 3건, `--no-run`, `clippy -p aip-execution --all-targets --locked --offline -- -D warnings`, `cargo check --workspace --locked --offline`, `git diff --check` 통과. 후속 SHA `b4ccf764f05ed248b443217157199c2f8135ceed`에서 회전 중 양 KEK 보유가 필요한 fixture를 보완했다. 재시작 DB 테스트는 파일만 빌드하고 로컬에서 실행하지 않았다. design 13 Proposed/runtime gate 유지.

`f37b23bafd959f12e2e484e4de4c072f37e0ccd7` FaceReview 묶음은 origin/main에 반영됐다. 7개 check success, Verify core는 기록 시점에 in progress였다. rekey 커밋은 아직 origin에 push하지 않았다. 다음 main push는 해당 Verify가 끝난 뒤 진행해 유용한 실행을 취소하지 않는다.

---

# 이전 재개 지점 (2026-10-29, Durable Rekey 구현 및 검증)

`crates/aip-execution::rekey`에 `RekeyContext`, `rekey_target_column`, `rekey_scope` 및 `RekeyLease`를 구현했다. `READABLE` 구 키 세대를 keyset pagination(`LIMIT 100`)으로 순회하며 원자적 CAS(`WHERE col = <old_frame>`)로 `ACTIVE` 키 세대로 재암호화한다. 레이스 행 재스캔(multi-pass)과 잔여 구 셀 전수 검증(`final 0`)을 거쳐 `DekStore::retire_dek_in`으로 안전하게 `RETIRED` 상태로 전이한다. infrastructure 스키마의 `aip_rekey_leases` 테이블과 epoch 검증으로 동시 rekey 작업자 및 stale writer를 차단한다.

검증: `cargo clippy -p aip-execution --all-targets -- -D warnings`, `cargo fmt --all --check`, 단위 테스트 `encryption_rekey` 3건 통과. 실DB 5종(봉인, 읽기, 셀 회귀, adopt 2종) 및 실DB rekey는 샌드박스 차단으로 미실행(우회 없음). Git push는 헤드리스 인증 프롬프트 차단으로 미실행(호스트 인증 환경 필요).

---

# 이전 재개 지점 (2026-10-28, 검증 이관 연결·게이트 실행)

`adopt::apply_verified`가 승인 일치 뒤·변경 전 같은 트랜잭션에서 암호화 전 열의 전수 AEAD 검증을 수행하도록 연결했다. 키셋 페이지 순회로 평문은 버리고 열·셀 개수만 결과에 싣는다. 위조 셀은 마커 제거 전 실패로 닫고, owner는 거부한다. 기본 creation·이관·초기화·마이그레이션 게이트는 그대로다. `ReadCellOpener`에 트랜잭션 내 열기를 추가했다.

검증: `cargo check`·`clippy -D warnings`·`fmt --check`, `adopt_cells` 링크, 게이트 단위 `adopt_cells` 1개·`encryption_gate` 4개·`activation_gate` 1개 실행 통과. 실DB 이관 2종은 이 명령 샌드박스 차단으로 미실행(우회 없음). DB 접속 가능 세션이 밀린 실DB 5종부터 실행한다. Git push는 기존 keyring 차단으로 미실행이며 같은 전환을 재시도하지 않는다.

---

# 이전 재개 지점 (2026-10-28, 읽기 복호화 연결·정적 검증)

인가 SQL 실행 뒤 root·1:1·1:N 각 층의 프레임을 여는 `execute_opened`와 `aip-execution::read_opener::StoreOpener`를 연결했다. 정책 소거 셀은 NULL로 남아 열지 않고 숨은 키는 응답에서 지운다. 혼합 scope·owner·위조 셀은 읽기를 실패로 닫으며, opener 없이는 기존 출력이 유지된다. gate·Proposed 상태 유지.

검증은 정적 범위다. `cargo check`·`clippy --all-targets -- -D warnings`·`fmt --check`(실행·읽기·쓰기), `encrypted_opened` 링크, wire_codes 9개 통과. 새 `encrypted_opened` 왕복 테스트는 작성했으나 이 명령 샌드박스에서 실DB 실행이 차단됐다(전기 턴과 동일, 우회 없음). DB 접속 가능 세션이 `encrypted_sealed`·`encrypted_opened`·신규 셀 회귀부터 실행한다. Git push는 기존 keyring 차단으로 미실행이며 같은 전환을 재시도하지 않는다.

---

# 이전 재개 지점 (2026-10-28, 봉인 writer 연결·정적 검증)

native writer(create_row/update)가 인가·평문 scalar 검증·행 ID/scope 확정 뒤 `Knobs.seal` 경계로 DB `seal_in`을 사용하도록 연결했다. `aip-execution::write_sealer::StoreSealer`가 별도 budget 연결 예약과 트랜잭션 내 봉인을 수행하고, 다중 행 갱신은 행별 CASE로 프레임을 나눈다. expect/`to`/effect의 암호화 대입, 행간 값 복사 봉인, 명시적 id·scope 리터럴 누락, lookup:eq·owner는 실패로 닫는다. sealer 없이는 기존 prototype(`v2:` 통과)이 유지되고 listener/migrate/adopt gate와 Proposed 상태는 그대로다. 같은 트랜잭션의 두 번째 예산 예약이 멈추지 않도록 DEK 잠금을 KEY SHARE/NO KEY UPDATE로 낮추고 회귀를 추가했다.

검증은 정적 범위다. `cargo check`·`clippy --all-targets -- -D warnings`·`fmt --check`, 쓰기·실행 `--no-run` 링크, spike-v1 `encrypted_types` 10개·wire_codes 9개 통과. 새 `encrypted_sealed` 통합 테스트와 `encryption_cells` 신규 회귀는 작성했으나 이 명령 샌드박스에서 실DB 실행이 차단됐다(docker 거부·loopback TCP 거부·initdb 사용자 조회 거부, 우회 없음). DB 접속 가능 세션이 두 테스트부터 실행한다. Git push는 기존 keyring 차단으로 미실행이며 같은 전환을 재시도하지 않는다.

---

# 이전 재개 지점 (2026-10-28, DB 암호화 경계·공통 저널 방문자 구현)

`f37b23b`에서 이어 `DekStore`의 DB 키 조회/암복호화·독립 commit 사용량 예약·epoch/key 승격을 구현했다. `encrypted_journal`의 공통 방문자를 `/apply`·`/status`·확장 요청 및 durable 효과 지문에 연결했다. `where` 배열 누락, compose 리소스/상수 문맥 누락, 실패 시 평문 fallback, 구 지문 키 부재를 mismatch로 축약하는 경로를 고쳤다. 제품 resource writer/read는 아직 이 DB 암호화 API를 사용하지 않으며 listener/migrate/adopt readiness gate와 Proposed 상태는 유지한다. 전체 포팅/1.0 완료가 아니다.

검증·환경 범위는 [제품 검증](../product/VERIFICATION.md)의 2026-10-28 암호화 증분을 따른다. 다음은 native writer가 인가·평문 scalar 검증·행 ID/scope 확정 뒤 DB `seal_in`을 사용하도록 연결하고, read/traverse와 migration/adopt의 인증 검증 및 durable rekey를 같은 경계로 연결하는 일이다. 고정 키 map·row/scope 기본값 0·요청을 미리 암호화한 뒤 평문 validator로 넘기는 기존 내부 경로를 제품 구현으로 재사용하지 않는다. Git push는 기존 Codex/macOS keyring exit 161 차단으로 미실행이며, 해결된 환경 근거 없이 같은 전환을 재시도하지 않는다.

---

# 이전 재개 지점 (2026-10-28, 1.0 작업 재개·공통 실행부 로컬 구현·검증)

기존 6개 로컬 커밋(85d4fcf~463b526)과 미커밋 live snapshot-negative 변경을 보존했다. 기존 사용자 지시대로 전체 소스/inventory·공통 갭을 기준으로 작업하며, 단일 여정 성공을 다른 독립 작업의 관문으로 삼지 않는다. 필수 WBS 범위와 model.h5 권리/visitor 미검증 상태는 유지한다.

현재 명령 환경(Codex/macOS)의 `gh auth switch --hostname github.com --user winterholic`은 `failed to move active token in keyring: exit status 161`로 실패했다. 비밀 저장소 우회·동일 전환 재시도 없이 push 미실행으로 남기고 로컬 구현은 계속한다. 다른 환경의 Git 기능 전체가 불가능하다는 판정이 아니다.

live snapshot-negative 검사는 이번 Run 소유 MySQL 8.4(loopback 35493, tmpfs)에서 실제 1 passed/0 failed로 실행했고 컨테이너 제거까지 확인했다. 이전 미커밋 문안의 “커밋·target 제거·정확 바이트 측정 완료”는 당시 코드/장부로 확인되지 않아 철회한다. 이번 실행의 Python runner/evidence/write-deployment 71개, Rust test 컴파일, 비DB scope 1개, fmt·shell syntax·diff 검사는 통과했다. 독립 E2E 11개 묶음의 실패를 수집한 뒤 최종 nonzero로 반환하는 runner를 연결했고 진단 모드 MongoDB fixture 누락도 수정했다. 전체 live 앱/UI 11개 실행은 미검증이며 공통 암호화·Infisical·FlaskBB와 최종 릴리스 범위는 계속 미완료다. 빌드는 기존 target을 재사용한다.

---

# 이전 재개 지점 (2026-10-27, 스냅샷 scope 음성회귀 고정·tutorial 부재검증·main push·CI 확인 대기)

48926c5 독립리뷰 지적(무조건 summary 제외 arm)을 닫았다. 확인 결과 arm 자체는 명시적 `exclude_summary` 게이트로 이미 전환되어 있었고(48926c5 diff에 무조건 arm 존재·현 트리 4-tuple 게이트, 전 호출처 대조: password/email만 true·signup/tutorial/search false·withdraw 별도 헬퍼), 빠져 있던 것은 게이트를 고정하는 회귀였다. `business_snapshot_excluding`의 테이블별 scope 결정을 순수 `snapshot_statement`로 추출(SQL 바이트 동일)하고 DB 없이 도는 `snapshot_scope_keeps_unintended_summary_inserts_visible` 음성회귀로 고정했다. 48926c5 형태로 임시 되돌린 RED에서 signup/tutorial 전체조회 단언이 실패함을 시연한 뒤 복원 GREEN. tutorial 판정에 signup과 같은 `summary_row_absent_for_member` 0-count 긍정검증을 추가했다(/my 미방문 여정이므로 부재가 정상). 무관행보존 조건은 전 여정 그대로다. 제품·gate 변경 없음.

로컬: 신규 Rust scope 테스트 RED(되돌림)→GREEN(복원) 직접 실행, `cargo fmt --check` OK, auth `--no-run --locked --offline` OK, `cargo clippy -p aip-cli --features mysql --all-targets -- -Dwarnings` exit0, `git diff --check` OK. Python evidence 63종+write-deployment 감사는 상위 별도 실행 결과 유효하므로 재사용하고 변경분(Rust)만 새로 확인했다. Cargo는 PG* unset+형식검증용 dummy DSN(미접속). 빌드 캐시 target/ 검증 후 제거·회수. 로컬 liveDB/browser 미실행. 로컬 main에 커밋, push는 known blocker로 재시도 없이 유지·원격 fa9afef. 쓰기 권한 세션이 `git push origin HEAD:main` 후 원격 SHA 대조부터 이어간다. CI 결과·withdraw 첫 artifact·tutorial 부재 count·account 2phase·admin 후단은 미검증이다. 관련25경로행 유지. WBS90/25·25경로전수미완·model.h5권리/visitor보류 유지. PMS·WBS6·Infisical/FlaskBB는 상위 계속(MCP 미접근·미검증).

---

# 이전 재개 지점 (2026-10-27, EditPage 확정탈퇴 여정 구현·로컬검증·main push·CI 확인 대기)

confirmedwithdraw 한 묶음을 현재 main에서 실제 구현했다(테스트·여정 코드만, 제품·gate 완화 없음). 원본 계약 대조: EditPage 탈퇴하기→alertdialog(정말 탈퇴하시겠어요?)→확인→DELETE /v2/mypage/withdraw→toast(탈퇴가 완료되었어요)→logout→/main(joowon 2e7c021), 백엔드는 본인 전이·5개 테이블 물리삭제·댓글 soft-delete·is_deleted=1(new-facereview 7948b7e), AIP `User.withdraw` 선언·overlay writeAdapter withdraw()가 동일 효과를 선언·호출한다. 새 `facereview-ui-withdraw-browser.py`는 disposable 회원 1명으로 login→/my→/edit→modal→confirm→toast→logout→/main signed-out을 수행하고 단일 self-target /apply·logout·UI 4boolean을 수집한다. 새 `member_withdraw_evidence` gate(login/withdraw/logout/ui, summary 호출은 gate 제외·사유 명시)와 단위테스트, Rust caller(전용 회원+관계행 8종 시드·타기기 세션·스크립트 실행·DB 판정), `business_snapshot_except_member`(전이 영향 8테이블 회원행 제외·bookmark/dna 생존은 전수 동등으로 증명)를 추가했다. DB 판정은 삭제 5종 0·댓글 1건 flip·bookmark/dna/안정컬럼 보존·summary 1행·0합계·lock1·checkpoint·reissue 401·active 0·타기기 세션 사망·login 401·메일 무변화·무관행/타세션 보존이다.

로컬: evidence unittest 63 OK(CI 플래그, 기존62+신규1), 배선 probe OK, py_compile 4종 OK, `cargo fmt --check` OK, auth `--no-run --locked --offline` 26.76s OK, `cargo clippy -p aip-cli --features mysql --all-targets -- -Dwarnings` exit0, `git diff --check` OK. Cargo는 PG* unset+형식검증용 dummy DSN(미접속). 빌드 캐시 target/ 검증 후 제거·회수. 로컬 liveDB/browser 미실행. 로컬 main에 커밋, push는 이 환경 차단 유지(상위 확정: Codex default daeseon-dev 403·winterholic 키체인 전환 실패·scoped helper 미제공·연결 앱 push 불가, 동일 denial 재시도 금지)·원격 fa9afef 유지. 쓰기 권한 세션이 `git push origin HEAD:main` 후 원격 SHA 대조부터 이어간다. 적층 커밋 CI 결과·withdraw 첫 artifact의 toast/modal/PNG·summary 행· tutorial live-store·account 2phase·admin 후단은 미검증이다. 관련25경로행 유지(성공 artifact 직접검사 후 withdraw 행만 갱신). WBS90/25·25경로전수미완·model.h5권리/visitor보류 유지. PMS 집계·WBS6 scope·Infisical/FlaskBB 대조는 상위 세션 계속(MCP 미접근·미검증). 참고: 이 Muse 경로의 파일 쓰기는 `bearer =` 토큰을 마스킹하므로 식별자에 bearer 사용 금지(우회 아님·명명 회피).

---

# 이전 재개 지점 (2026-10-27, 쓰기배포 enable_writes 회귀가드·로컬검증·main push·CI 확인 대기)

48926c5의 근본원인을 교훈이 아니라 테스트로 고정했다. 8개 mysql_* 확장 배포와 7개 browser 여정을 호출처 기준으로 전수 대조했다: 쓰기확장 /apply 호출(인라인 `"request": {"extension"}` 또는 `row["write_extension"]` 태그 여정)은 auth(password/email 경유)·trending_job·watch·external_effects·worker_write_e2e 5곳에만 있고 5곳 모두 `enable_writes: true`다. 플래그 없는 3곳(recommend·pool_job·category_fill_job)은 읽기 `/extension`·job 전용이라 추가하지 않았다. 일반 전이 /apply와 읽기 goldens는 규칙의 명시적 negative다. worker_write_e2e는 `apply` 헬퍼 안에 요청을 만들어 마커가 못 보므로 파일명으로 pin하고 한계를 문서화했다.

새 `product/tests/test_facereview_write_deployments.py` 8종(규칙 6+전수감사 1+실제 auth에서 플래그 제거 시뮬레이션 1). 제품·gate·배포 코드 변경 없음.

로컬: evidence unittest 62 OK(CI 플래그, 기존54+신규8), py_compile 6종 OK, `cargo fmt --check` OK, auth `--no-run --locked --offline` 28.74s OK, `cargo clippy -p aip-cli --features mysql --all-targets -- -Dwarnings` 35.73s exit0, `git diff --check` OK. Cargo는 PG* unset+형식검증용 dummy DSN(빌드스크립트는 형식만 검사·미접속). 빌드 캐시 target/ 3.0G 검증 후 제거·회수. 로컬 liveDB/browser 미실행. 로컬 main에 커밋, push는 이 환경 차단 유지(`gh auth status` 미로그인·키체인 계정은 직전 403 daeseon-dev, 동일 denial 재시도 금지)·원격 fa9afef 유지. 쓰기 권한 세션이 `git push origin HEAD:main` 후 원격 SHA 대조부터 이어간다. 적층된 로컬 커밋들의 CI 결과·새 artifact myEmotionSummary ok/body·감정그래프 PNG·body_failure_reason 값·tutorial live-store·account 2phase·confirmedwithdraw·admin 후단은 미검증이다. 관련25경로행 유지. WBS90/25·25경로전수미완·model.h5권리/visitor보류 유지. PMS 집계 불일치(total101 vs list_tasks 132)·WBS6 보존 5건 scope·Infisical/FlaskBB 대조는 상위 세션이 PMS description으로 확정한다(MCP 미접근·미검증).

---

# 이전 재개 지점 (2026-10-27, auth E2E enable_writes·감정요약 긍정검증·main push·CI 확인 대기)

User.myEmotionSummary WIRE.NOT_FOUND의 공통 원인을 코드로 확정했다. `/apply` 쓰기 확장은 `workers.enable_writes` 없으면 명시적 설정 gate(`spike-v6-transport/src/lib.rs:1438-1440`)에서 HTTP200·ok:false·WIRE.NOT_FOUND로 거부된다. 회원 auth E2E(`mysql_facereview_auth.rs`)만 이 설정을 빠뜨렸고 trending/admin/watch/external_effects 배포는 전부 `enable_writes:true`다. 같은 정의·같은 확장이 trending CI에서 성공하므로 제품 결함이 아니라 이 배포의 설정 누락이다. `enable_writes:true` 1줄로 password 4건·email 3건의 호출이 실제 실행되며, fingerprint는 jobs가 이미 all-extensions로 찍어 변경 없고(`mysql_service.rs:528-534`), `validate_with_writes`가 요구하는 4개 쓰기 모듈(slice14e/14k/14l/14g)도 worker dir에 전부 있다.

실행 부작용을 좁혔다. 확장은 첫 호출에 회원 행을 만들고 1회만 commit(lock 0→1)하며, fixture에 시청 세션이 없어 이후 호출은 rewrite 없이 base를 반환한다. 11테이블 스냅샷에 `user_emotion_summary`가 포함되므로 여정 회원의 행만 제외하는 arm을 추가했고(타회원 행은 양쪽 유지), 제외분을 긍정검증으로 메웠다. password DB: 회원 행 1건·5종 합계 전부 0·lock_version 1·checkpoint 시각 NOT NULL+빈 세션 표시. email DB: 동일 행 그대로 1건 유지(중복·rewrite 없음). profile/reissue/like/comment/search/logout/account는 스냅샷 비교가 깨지지 않음을 호출처별로 대조했다(MyPage 방문은 profile/logout뿐이며 checkpoint 이후 무쓰기, search·signup 스냅샷은 사전 존재 행만 포함). browser evidence에는 `emotion_summary_served` gate(태그된 호출 중 1건 이상 ok)를 password/email 판정에 추가했다. 원본 대조 근거: 세션 없음→0 합계는 원본 `get_emotion_summary`와 동일한 기대값이며(trending 19사례 동등성), 가짜 0이 아니다.

로컬: evidence unittest 54 OK(CI 플래그 포함, 신규 gate 테스트 1종), `py_compile` 5종 OK, `cargo fmt --check` OK, `cargo test -p aip-cli --features mysql --test mysql_facereview_auth --no-run --locked --offline` 19.74s OK, `cargo clippy -p aip-cli --features mysql --all-targets -- -Dwarnings` 43.68s exit0, `git diff --check` OK. 빌드 캐시 target/ 3.1G는 검증 후 제거·회수했다. 로컬 liveDB/browser 미실행. 로컬 main에 커밋했으나 push가 다시 `403 denied to daeseon-dev`로 차단됐다(원격 main은 fa9afef 유지, 이 환경에서 재시도 금지). 쓰기 권한 세션이 `git push origin HEAD:main` 후 원격 SHA 대조부터 이어간다. 새 CI artifact의 myEmotionSummary ok/body·감정그래프 PNG·body_failure_reason 값·tutorial live-store·account 2phase·confirmedwithdraw·admin 후단은 미검증이다. 관련25경로행 유지. WBS90/25·25경로전수미완·model.h5권리/visitor보류 유지.

---

# 이전 재개 지점 (2026-10-27, password/email/account body 실패원인 배선·main push·CI 확인 대기)

Verify40076012954(751ad8d)의 password 전체body gate 실패(line232)는 수집기가 `json.loads(response.body())` 예외를 버려 원인 불명이었다. 같은 삼킴이 password/email/account 3개 스크립트에 공통으로 있고 profile/admin/admin-reissue/member는 이미 `body_failure_reason`을 기록함을 확인했다. 3개 스크립트의 except에만 profile 패턴(`json_decode_error` vs `body_failure_reason(error)`)을 배선했고, 전체body gate·판정 범위·원본UI/overlay 제품코드·DB보존조건은 변경하지 않았다. 새 CI artifact의 해당 행에 `body_failure_reason`이 남아야 실제 예외 종류(target_closed/timeout/resource_evicted/playwright_error/json_decode_error)를 확정할 수 있다.

로컬: 3종 `py_compile` OK, 배선 probe(3종 wiring+gate strict, profile 참조 일치) OK, evidence unittest 53 OK(CI 플래그 `-W error::ResourceWarning` 포함), `git diff --check` OK, 변경 3파일만. Rust/overlay/어댑터 변경 없음. 로컬 main에 커밋 `85d4fcf` 완료했으나 push가 `403 Permission to winterholic/Application-Intent-Protocol.git denied to daeseon-dev`로 차단됐다(원격 main은 fa9afef 유지, 읽기 fetch/ls-remote는 정상). 쓰기 권한 세션이 `git push origin HEAD:main` 후 원격 SHA 대조부터 이어간다. CI 결과·다음 artifact의 failure_reason 값·tutorial live-store 검증·account 2phase·confirmedwithdraw·admin 후단은 미검증이다. WBS90/25·25경로전수미완·model.h5권리/visitor보류·myEmotionSummary/graph 별도잔여 유지.

---

# 이전 재개 지점 (2026-10-24 17:46 KST, Verify40076012954 비밀번호 body 수집 gate 실패)

일반 Verify `40076012954`, SHA `751ad8d13483c951fbfb026ea12e8a4c0eec015d`, attempt1은 completed/failure(core종료 08:22:04UTC)다. Broad11·원본UI준비13/14는 success, 첫 실패는 실HTTP/worker/MySQL step15다. Safe artifact `12554215645` (`facereview-ui-browser-40076012954-1`)를 현재 Run outbox `/Users/winterholic/.ttakkari/data/outbox/f5290b4f-874f-49e9-8f1a-133da3f9fd35/verify-40076012954`에 다운로드했다. Locator는 stage=FaceReview member authentication E2E test, command=tee /tmp/aip-e2e-auth-run-"$tag".txt, exit101이다.

`password/password-browser-result.json`은 journey_stage=login_new/failure_line232/failure_path=/my/failure_type=unrecognized다. 원본 password UI6boolean·member_password6gate/verified는 true지만 최종 all-body assertion에서 실패했다. API28건 모두 HTTP200·finished28/failed0/unfinished0/pageerror0이며, 응답목록 21번째 `/apply` POST `User.myEmotionSummary` 한 건만 body_captured=false다. 나머지 같은 호출 3건은 body_captured=true/ok=false/WIRE.NOT_FOUND다. PNG5개를 직접 검사해 새password 폼·signed-out·재login/reload된 본인 profile을 확인했다(첫 PNG는 fade중, 감정그래프는 skeleton). Password DB는 database_status=success/모든boolean true/active1 revoked2/challenge0/mail1/11업무테이블·타계정session보존, browser_status=failure다. 따라서 전체 password browser 성공으로 세지 않는다.

동일 SHA의 수집 함수는 pending.pop·finished.add 이후 `json.loads(response.body())` 예외를 삼킨다. 실제 함수 AST를 브라우저 없이 실행한 synthetic probe에서 body읽기 예외와 invalid JSON이 모두 finished1/body_captured=false/pending0을 남겼고 valid WIRE.NOT_FOUND JSON은 body_captured=true였다. 이는 finished와 body수집의 차이를 입증하며, CI의 실제 예외 종류나 navigation 관련 원인은 미확인이다. `safe_browser_failure_type`에 예외 객체 대신 문자열을 넘겨 unrecognized가 되는 코드도 확인했으나 불변 지시에 따라 고치지 않았다.

Profile/reissue·like/logout·comment3phase/search의 browser/DB JSON은 기존 성공 범위를 재확인했다. Add33/remove37/comment각23/search22는 failed0/unfinished0/pageerror0다. 이번 artifact에는 email/account-complete/account-skip/admin이 없고 Rust caller는 password browser assertion에서 후단을 중단한다. 따라서 751ad8d의 tutorial live-store 수정은 새 UI·DB에서 검증되지 않았다. CheckEmail/signup/tutorial 25경로행은 byte동일하게 유지하고 confirmedwithdraw도 진행하지 않았다. 제품overlay/Rust/password·email·member·account scripts/원본UI/정책은 변경하지 않았다. 재실행·dispatch·cancel·후속예약 없이 실패 근거만 기록한다. 재현: 현재 outbox의 `python3 inspect-verify-40076012954.py` → CI 실패행/DB/미도달 phase와 synthetic probe 출력. WBS90/25·25경로전수미완·model.h5권리/visitor보류 및 User.myEmotionSummary/graph 별도잔여 유지.

---

# 이전 재개 지점 (2026-10-24, 첫계정 가입 DB 성공·튜토리얼 reload 판정 수정)

일반 Verify `40072496940`, SHA `ef828ac7a1dbb9b442fe4a97c1faef4e19f09031`, attempt1은 completed/failure(core종료 2026-10-24 07:22:58UTC)다. Broad11·UI준비13/14는 success, 최초 실패는 실HTTP/worker/MySQL step15다. Safe artifact `12552860245`를 현재Run outbox `/Users/winterholic/.ttakkari/data/outbox/a461ad09-aff0-4da4-b4f5-d65e56ee8ac4/verify-40072496940`에 다운로드했다. Locator는 stage=FaceReview member authentication E2E test, command=tee /tmp/aip-e2e-auth-run-"$tag".txt, exit101이다. Raw GH log/cache는 접근하지 않았다.

첫 account-complete의 signup JSON2개·PNG2개를 직접 검사했다. 신규false→기존true checkEmail 각1회·signup POST201 1회/정확payload·성공body/전gate·verified 및3finished/failed0/unfinished0/pageerror0다. DB도 browser/database success, 전boolean true: 서버UUID/scrypt/name·GENERAL:0:0:0/animal,eating 정확2장르/active1 revoked0/신규user·genre 외11업무테이블·타계정session 보존이다. 직전 false였던 selected_genres_match·unrelated_business_rows_preserved가 이번에는 true다. ENUM문자정렬·snapshot exec통일 이후 첫계정에서 재발하지 않았으나 과거 실행의 실제 행보존을 소급 보증하지 않는다.

Tutorial complete는 initial/second/final-step PNG3개와 account-failure PNG를 직접 검사했다. Step1/2/3·단일 User.completeTutorial/정확본인target·성공body/auth/contract·main 복귀는 관측됐고 DB도 전boolean true(GENERAL:0:1:0/target tutorial·updated_at 외열·장르포함11테이블·타계정session 보존/active2 revoked0)다. 그러나 browser는 reload_main, line214, failure_path=/main에서 실패했다. member_tutorial evidence는 비었고 lifecycle17requests/14finished/failed0/unfinished3/pageerror0다. account-skip과 후단admin은 없으며 complete의 최종 reload PNG도 없으므로 전체 가입/튜토리얼 성공이나25경로 완료로 세지 않는다.

판정기 결함을 좁혔다. 고정 UI 2e7c021의 authStore.partialize는 user_tutorial을 localStorage에 저장하지 않는다. App은 reload 뒤 getUserName→setUserInfo로 메모리 Zustand store만 갱신한다. 기존 wait_for_function은 localStorage.user_tutorial===1을 요구했다. 이 실제 predicate를 Node에서 실행해 persisted tutorial이 없고 메모리 tutorial1인 정상상태도 false인 RED를 재현했다. 검사를 원본 App이 재조회한 live store의 signed-in·정확target·tutorial1로 변경했다(상태쓰기·추가login·API mock 없음). 잘못된target/미완료/sign-out은 계속 false다. CI의 오류클래스는 safe JSON에 unrecognized라 특정하지 않는다. PNG3개가 fade중 어두워 tutorial 영역의 유한애니메이션 종료후 캡처하도록 보완했다. 원본UI/overlay제품코드·DB보존조건·단일mutation·lifecycle 기준은 불변이다.

앞단 password28/email20·profile/reissue/like/logout/comment3phase/search JSON은 기존 성공 범위대로 재확인했다. Password/email 전gate·DBboolean true, add33/remove37/comment각23/search22는 failed0/unfinished0/pageerror0다. User.myEmotionSummary/WIRE.NOT_FOUND·감정그래프 skeleton은 별도잔여이며 admin은 이번실행 미검증이다.

로컬: reload predicate 회귀 AssertionError RED→GREEN, 전체 evidence unittest53 OK, Python AST·embedded JS syntax·git diff --check 통과. Rust/overlay제품코드는 이번에 변경하지 않았다. 실DB·새 browser 검증은 다음 일반 Verify 전 미실행이며 webapp-testing 기준으로 기존 Linux CI만 사용한다. Clone/localDB/container/localbrowser/subagent 신규생성 없음. 관련25경로행은 유지하고, 두 account phase의 전JSON/PNG·회귀 성공 뒤에만 갱신→EditPage confirmedwithdraw로 이어간다. WBS90/25·25경로전수미완·model.h5권리/visitor보류 유지.

---

# 이전 재개 지점 (2026-10-24, 가입 UI 성공·가입 DB 판정 실패 조사)

일반 Verify `40068996762`, SHA `68cdf255d738c7f7a977c9ae9bdbf8979cb4f8b9`, attempt1은 completed/failure(core종료 2026-10-24 06:27:30UTC)다. Broad11·UI준비13/14는 success, 최초 실패는 실HTTP/worker/MySQL step15다. Safe artifact `12551196219`를 현재Run outbox `verify-40068996762`에 다운로드했다. Locator는 stage=FaceReview member authentication E2E test, command=tee /tmp/aip-e2e-auth-run-"$tag".txt, exit101이다. raw GH log cache는 접근하지 않았다.

`account-complete/signup-browser-result.json`과 PNG2개를 직접 검사했다. 원본AuthPage 신규email→비밀번호확인→이름/먹방·동물선택→가입후빈로그인폼복귀→기존email분기는 completed/member_signup 전gate·verified=true다. Check-email은 POST200 신규false/기존true 각1건, signup은 POST201 1건이며 정확payload·완료성공body/3finished/failed0/unfinished0/pageerror0다. 그러나 signup DB는 failure: UUID/scrypt/name·초기flags/active1 revoked0/타계정session보존은 true지만 selected_genres_match와 unrelated_business_rows_preserved가 false다. Account-complete tutorial 파일과 account-skip 폴더는 없으며, 후단 admin도 실행되지 않았다. 성공 조건이 충족되지 않았으므로 checkEmail/signup/tutorial 25경로행은 갱신하지 않았다.

코드 기반 원인 두 가지를 좁혔다. (1) 장르열은 MySQL ENUM으로 eating이 animal보다 먼저 선언된다. ORDER BY genre는 선언순 정렬인데 판정기는 animal,eating을 요구했다. 기존 profile 검증과 같이 ORDER BY CAST(genre AS CHAR)로 바꿨다. (2) signup 전 snapshot은 query(text), 후에는 user/genre만 exec(binary)라 raw Value Debug 비교가 타입표현 차이를 잡는다. 로컬의 설치된 mysql_common0.37.3 실제 RowDeserializer를 오프라인 실행해 동일 SQL정수1이 text=[Bytes("1")], binary=[Int(1)]인 것을 재현했다. Helper의 무필터 조회도 exec로 통일했다. 모든행·모든열·순서 및 제외대상은 그대로며, 보존조건을 줄이지 않았다. 이 원인을 CI 실패의 최종 원인으로 확정하려면 새 CI DB결과가 필요하다. 당시 실제 DB가 보존됐다고 소급 주장하지 않는다.

앞단 password(28finished)/email(20finished)는 전gate·DBboolean true, profile/reissue/like/logout/comment3phase/search도 각 JSON 범위 성공을 재확인했다. Add33/remove37/comment각23/search22는 failed0/unfinished0/pageerror0이다. Password/email의 User.myEmotionSummary/WIRE.NOT_FOUND는 별도 잔여이며 graph skeleton을 성공으로 세지 않는다. 후단admin은 이번 artifact에 없으므로 미검증이다.

로컬검증: evidence unittest52 OK, cargo fmt/diff OK. PG*unset·explicitloopback55432/aip_test_run DSN에서 MySQL migrate/service/CLI all-target clippy -Dwarnings5.92s exit0, auth test no-run --locked --offline compile6.59s exit0. 컴파일은 실제 DB검증이 아니다. 기존 password/email/member/account browser scripts와 원본UI·overlay 제품코드·정책은 불변이다. Clone/DB/container/localbrowser 신규생성 없음. 새 일반 Verify 결과를 기다린다. 성공시 두 account phase의 JSON/PNG 전체와 기존회귀를 직접검사한뒤 관련25행만 갱신→EditPage confirmedwithdraw로 이어간다. WBS90/25·25경로전수미완·model.h5권리/visitor보류 유지.

---

# 이전 재개 지점 (2026-10-24, 이메일 인증 UI·DB 성공·가입/튜토리얼 검증 추가)

일반 Verify `40065518651`, SHA `22089a4e0def6cd9461420bf72c3a7990a8c46cf`, attempt1은 completed/success(core종료 2026-10-24 05:48:35UTC)다. Broad11·원본UI준비13/14·실HTTP/worker/MySQL15·safeartifact16·cleanup18이 success다. Artifact `12549937284`를 현재Run outbox `verify-40065518651`로 다운로드해 email JSON2개·PNG4개를 직접 검사했다. MyPage 미인증banner→코드modal→인증badge/banner제거→reload유지의 UI5boolean, member_email3gate·verified 모두 true다. 두 인증경로는 POST각1건/HTTP200/ok/body_captured/payload_matches/authorization_present true. API20건 모두finished/failed0/unfinished0/pageerror0. Code/password/token 원문은 산출물에 기록하지 않는다.

Email DB의 browser/database success 및 모든boolean true를 확인했다. Flags GENERAL:0:1:1, target인증flag·updated_at외모든열보존, challenge0/mail1회·수신계정/template, active2/revoked2, target user제외11업무테이블과타계정session fullsnapshot보존을 실행SHA caller와대조했다. 25경로표의 email verification/verify 두행을 성공근거로갱신했다. 가입·튜토리얼·탈퇴 성공으로 확대하지 않는다.

Password는 전gate·DBbooleantrue/28finished/failed0/unfinished0/pageerror0이다. 기존profile/reissue(DBsuccess,USEDrefresh+1,expired2→reissue1→retry2), like1→0/타회원보존/logoutrevoked+1, 댓글3phase동일ID·softdelete·무관행보존, search4gate·11테이블보존과 admin기존checks/기간5/permission4/reissue2회도 JSON범위대로재확인했다. Add33/remove37/댓글각23/search22 lifecycle도완료됐다. 마지막locator는admin stage/command·exit pending이며실패가아니다. 개별cargo stdout은미열람이다.

이번계측에서 password의4건/email의3건 `/apply` HTTP200·okfalse가 전부 write_extension=`User.myEmotionSummary`/error_code=`WIRE.NOT_FOUND`로 확인돼 호출대상은 확정됐다. 실패근본원인은 아직미검증이며 이메일인증과별도잔여로둔다. PNG의감정그래프 skeleton·일시적인0표시를 감정통계검증성공으로 세지않는다. 과거실행의호출대상을소급확정하지않는다.

다음 bounded증분은 원본AuthPage signup/checkEmail과 TutorialPage 완료·건너뛰기다. 서로다른2개disposable계정을 실제원본가입UI로만만들고(preseed없음), 각signup직후DB를검사한후fresh browser에서login→tutorial을실행한다. Check-email은 신규false→가입후existingtrue 분기/정확body를검사하고 signup은POST201각1건·정확name/genre/password boolean, 로그인폼복귀와genre선택PNG를남긴다. DB는server생성UUID·scrypt·GENERAL:0:0:0·장르animal/eating정확2행·active1/revoked0 및신규user/genre행외11테이블·타계정session보존을요구한다.

Tutorial은complete경로의step1/2/3와완료버튼 또는별도계정의skip버튼→단일User.completeTutorial/정확본인target·성공body·인증/contract→main/reload후실제profile재조회로user_tutorial1을요구한다. DB는GENERAL:0:1:0/target tutorialflag·updated_at외모든열·genre/타계정보존·active2/revoked0을별도JSON으로기록한다. 원본UI/정책불변. 새account UI·DB는CI전미검증이고 EditPage confirmedwithdraw는다음잔여다.

로컬실행: signup/tutorial판정기stub AssertionError RED2→GREEN, 전체evidence unittest52OK·adapter node64pass·AST/fmt/diffOK. Rustauth no-run --locked --offline compile3.81s exit0, MySQL migrate/service/CLI all-targetclippy -Dwarnings2.80s exit0. PG*unset/전용55432/testDSN 계약준수; 로컬liveDB/browser미실행. 기존password/email/member제품코드·판정범위는불변이고snapshot helper의새genre제외옵션은signup비교에만적용한다. WBS90/25·25경로전수미완·model.h5권리/visitor보류유지. 동일refVerify진행중push/dispatch/cancel·장시간대기금지,45분timeout+3분후속예약유지.

---

# 이전 재개 지점 (2026-10-24, 비밀번호 UI·DB 성공·MyPage 이메일 인증 검증 추가)

일반 Verify `40062036395` attempt1, SHA `08314567afc200377ae2d366209284f359827119`은 completed/success(core종료 2026-10-24 04:45:09UTC)다. Broad11·UI준비13/14·실HTTP/worker/MySQL15·artifact16·cleanup18 모두 success. Safe artifact `12548545988`를 현재 Run outbox `verify-40062036395`에 다운로드했고 password JSON2개·PNG4개를 직접 검사했다. `member_password` 전체6gate/verified 및 UI6boolean true, `/auth/password` POST1건·정확payload·완료된성공body, logout body/refreshcookie/clientclear, old/new login 성공과 새비밀번호 재login/reload를 확인했다. API27건 모두 finished/failed0/unfinished0·pageerror0. `/session`은 전체3회(최초login1/새login1/reload1)이며 재로그인 경합에서 보이던 중복2회가 없다. 첫 PNG는 fade 도중 어둡지만 코드 후 폼·로그아웃·최종profile PNG를 확인했고 비밀번호 입력·code·reset_token은 저장하지 않았다.

Password DB는 browser/database success 및 모든 boolean true다. Hash변경/scrypt, oldpassword거절, 타기기폐기, revoked2·active1/challenge0, mail1회·수신계정/template, flags GENERAL:0:1:0 보존 및 target user만 제외한11개업무테이블 fullsnapshot·다른계정session fullsnapshot 보존을 실행SHA caller와 대조했다. 사용자25경로 표는 비밀번호 여정의 send-code/code-exchange/password 세 행만 갱신했다. 이메일 verify/signup/tutorial/withdraw 화면 성공을 이 결과로 주장하지 않는다.

기존회귀 JSON도 직접검사했다. Profile/reissue browser·DB success/USEDrefresh+1 및 expired2→reissue1→retry2, like add/remove·DB1→0/타회원count보존·logout revoked+1, 댓글3phase·동일ID/softdelete/무관행보존, search4gate/11테이블보존이 유지됐다. Add33/remove37/comment각23/search22 lifecycle은 failed0/unfinished0이다. Admin 기존 dashboard/기간5·role/승인/거절/삭제/permission4 및 reissue2회를 JSON 범위대로 확인했다. Locator의 마지막 admin stage/command·exit pending은 실패가 아니고 개별cargo stdout은 미열람이다.

비밀번호 JSON의 `/apply` HTTP200/okfalse는3건이며 전부 `WIRE.NOT_FOUND`다. MyPage→getEmotionSummary→SDK writeExtension(User.myEmotionSummary)→/apply라는 source 경로와 일치하지만 현재 artifact에는 호출명이 없어 동일 호출이라고 확정하지 않는다. 다음 password/email JSON에 허용된 해당 호출명만 기록한다. 감정그래프 skeleton이나 이 오류를 성공으로 포함하지 않으며 별도 원인조사 잔여다.

다음 bounded 증분은 원본 MyPage 미인증banner→이메일인증모달→메일code입력→badge/banner제거→reload유지다. 방금 비밀번호를 바꾼 동일 disposable계정의 fresh browser를 쓰며 기존 run-owned in-memory메일sink를 재사용한다. DB는 인증flag0→1/GENERAL:0:1:1, target user의 인증flag·updated_at 외 모든열보존, challenge0/mail1회, active2/revoked2, 무관한11개업무테이블·타계정session fullsnapshot 불변을 별도 JSON으로 판정한다. 원본UI/제품정책 불변. Email UI·DB는 새 CI 전까지 미검증이며 signup/tutorial완료·skip/confirmedwithdraw는 다음 잔여다.

로컬: 새 email 판정기 stub의 AssertionError RED→GREEN, 전체evidence unittest50 OK/overlay node64 pass, AST/fmt/diff OK, MySQL migrate/service/CLI all-targetclippy6.50s exit0 및 Rust auth --no-run --locked --offline compile6.29s exit0. 로컬liveDB/browser는 미실행이다. WBS90/25·25경로전수미완·model.h5권리/visitor보류 유지. 같은ref Verify진행중 push/dispatch/cancel·장시간대기금지 및45분timeout+3분후속예약을 유지한다.

---

# 이전 재개 지점 (2026-10-24 10:00 KST, 비밀번호 CI broad gate 실패·로컬 lint 수정 및 최초실패 계측 보강)

일반 Verify `40043525384` attempt1, SHA `3678af624974325fb1ac082104426f869f06b20d`은 completed/failure(00:14:06UTC 종료)다. 최초 실패는 step11 broad compiler/runtime/product/worker/transport/SDK이고 React 준비13·admin준비14·실제HTTP/DB/browser15는 skipped다. Safe artifact `12539998150`에는 verify-failure.txt만 있고 stage=broad gate/command=bash tools/verify.sh/exit101이다. Annotations도 exit101 외 첫 Rust 오류를 특정하지 않는다. 비밀번호 browser/DB는 도달하지 않았으므로 관련25경로행은 pending을 유지한다. 거부된 raw GH log cache 경로를 우회하지 않았다.

로컬에서 변경된 MySQL auth test의 clippy를 좁혀 실행해 exclude_user.is_some() 뒤 unwrap()이 clippy::unnecessary_unwrap/-D warnings로 exit101 되는 결함을 직접 재현했다. match (table, exclude_user)로 바꿔 같은 SQL·parameters·snapshot 의미를 유지했다. 이후 CI와 같은 MySQL migrate/service/CLI all-target clippy는 explicit loopback55432/test DSN·PG*unset·--locked --offline에서 exit0/3.38s다. 원격 CI의 첫 Rust 오류 원문이 없으므로 이 결함을 과거 CI의 직접 원인으로 확정하지 않고 유력한 코드 기반 설명으로 둔다.

추가 계측 결함도 독립 재현했다. nested shell의 실제 inner failure locator를 부모 bash command/outer stage가 덮어쓰는 회귀 시험이 FAIL이었다. pending stage/start marker는 새 gate를 시작하게 두고, 이미 numeric failure가 기록된 경우 부모의 후속 non-pending write가 덮지 않게 했다. 자식 첫실패 보존과 다음stage reset을 검사했고 전체 evidence unittest49 OK다. tools/verify.sh의 DBguard/readiness/fmt/workspace/auth-service/PoC/MySQL clippy 앞에 단계 marker를 추가했다. 검사 명령·순서·범위·제품코드는 바꾸지 않았다. shell syntax/fmt/diff OK; 로컬 liveDB/browser는 미실행이다.

새 일반 CI로 비밀번호 원본 UI·DB 증거를 확보한다. 동일ref 진행중 push/dispatch/cancel·장시간대기·맹목retry 금지,45분timeout+3분 후속예약을 유지한다. 성공 뒤에만 해당25경로행 갱신 및 이메일검증/signup/tutorial/confirmedwithdraw 잔여로 진행한다. 컨텍스트54%의 연속성을 사용해 이번 조사·작은수정을 마친 뒤, 상태·변경·검증을 인계에 저장하고 새컨텍스트로 결과확인을 이어간다. WBS90/25·25경로전수미완·model.h5/visitor보류 유지.

---

# 이전 재개 지점 (2026-10-24 09:10 KST, 검색 UI·DB 성공·비밀번호 원본 UI 검증 추가)

일반 Verify `40038229142` attempt1, 코드 `f436a06173d2207020f5e334d8caa5f3bc40e3f4`은 completed/success(2026-10-23 23:45:33 UTC 종료)다. Safe artifact `12539367749`를 이번 Run outbox에 다운로드해 검색 found/empty × submit/reload JSON·PNG4개를 직접 검사했다. 전체와 4 gate 모두 verified=true, 각 단일 Video.homeSearch 응답 HTTP200/ok/body 완료·정확input boolean·contract 일치, found length1/fixture=true 및 empty0/false다. 검색 lifecycle22건 모두 finished/failed0/unfinished0, page_error0. 별도 search-db JSON은 browser/database success, business_table_count11/all_business_rows_preserved=true. 실행 SHA의 fullsnapshot caller와 대조하고 25경로 search행을 갱신했다.

댓글3phase는 각23finished/verified 및 DB modified0→1→1/deleted0→0→1/동일ID·행+1·무관한댓글 전체보존을 재확인했다. Profile/reissue browser·DB success와 USED+1, like1→0 및 타회원count보존, logout revoked+1, 관리자 기존 checks와 2회 reissue도 범위대로 재확인했다. Broad11/full15/artifact16/cleanup18 success와 마지막 administrator console E2E locator를 확인했으며 개별 cargo stdout은 미열람이다.

다음 계정 여정 중 PasswordChangePage를 별도 browser script로 추가했다. disposable 전용계정의 원본 로그인→MyPage 비밀번호 변경→인증코드 발송→code입력→새비밀번호→원본 자동logout→새비밀번호 재login→reload profile을 검사한다. 기존 mailer sink의 run-owned loopback capability로 해당 fixture code만 메모리에서 전달하고 code/reset_token/password/hash 원문은 산출물에 기록하지 않는다. DB는 hash변경/old password 거절, 다른기기 세션 폐기와 logout 포함 revoked2/새login active1, challenge소진, mail1회·수신계정, target flags 및 무관한11개업무테이블 모든행/다른계정session 보존을 별도로 판정한다. 새 CI 전까지 실제 password browser/DB는 미검증이다.

로컬: 판정기 stub RED→GREEN47(정상 body/정확payload/단일요청/인증/실제 cookie logout/렌더 부정대조), mypageAdapter2 pass, in-memory mailer capability/타수신자격리 실HTTP시험1 pass(0.01s), browser AST·기존member/search script byte동일·fmt/diff OK. Rust auth no-run compile2.50s exit0; 첫 compile은 새 mailer Vec 타입추론 E0282여서 Vec<Value> 명시 후 통과했다. Live DB/브라우저는 로컬 미실행. 프로젝트정책·원본UI/checkout은 불변이다.

원본 호출처 대조: MyPage 미인증 배너/코드모달→email verification/verify, AuthPage→check-email/signup 및 login, TutorialPage 완료/skip→completeTutorial, EditPage 확인모달→withdraw가 실제 연결된다. 이 source 확인을 browser coverage로 세지 않는다. 비밀번호 성공 후 별도 이메일검증/signup/tutorial/confirmed withdrawal 잔여를 이어간다. 동일ref CI 진행중 push/dispatch/cancel·장시간 대기 금지;45분 timeout+여유3분으로 후속예약한다. WBS90/25·25경로전수미완·model.h5권리/visitor보류 유지.

---

# 이전 재개 지점 (2026-10-24 08:10 KST, 댓글 UI·DB 성공 확인·검색 검증 추가)

일반 Verify `40032647647` attempt1, 코드 `5741949eac1f13cd7e027a71b513b4a5b3922708`은 completed/success(2026-10-23 22:40:56 UTC 종료)다. `gh run view --json headSha,status,conclusion,attempt,jobs`로 broad step11·full HTTP/worker/MySQL step15·artifact16·cleanup18 success를 확인했다. Safe artifact `12537050658`을 현재 Run outbox에 다시 다운로드했고 comment-create/edit/delete JSON·reload PNG를 직접 검사했다. 세 단계 모두 completed/page_error_count0/member_comment.verified=true, 단일 mutation1/HTTP200/ok=true/body_captured=true, fixture target/content boolean과 contract 일치다. 각23 requests/responses/finished, failed0/unfinished0이다. PNG는 생성 댓글→수정 내용과 (수정됨)→삭제 후 기존 두 댓글만 표시한다.

각 `comment-db-result.json`의 browser/database success, same_comment_identity·target_content_and_flags_match·target_row_present·unrelated_rows_preserved=true, total_rows_added=1을 확인했다. 같은 ID의 is_modified0→1→1/is_deleted0→0→1이며 생성시각과 soft-delete 행보존·무관한 행 전체 snapshot 비교는 실행 SHA Rust caller와 대조했다. 25경로 표의 comments browser 행을 성공 근거로 갱신했다. 타회원 전체 행 보존은 댓글 범위이고 like는 기존 count 보존 범위다.

기존 회귀도 직접 검사했다: profile/reissue browser·DB success, USED refresh+1, add/remove like verified와 rows1/0·타회원 count 보존, bookmark completed/원본 reload PNG, logout verified·revoked+1. Add33/remove37 lifecycle failed0/unfinished0. Admin JSON은 dashboard/기간5/역할변경/승인·거절/삭제/권한거절4개를 기록하고 별도 reissue는 두 번 모두 reissue1·새 token 저장이다. Locator는 마지막 administrator console E2E stage이며 command/exit=pending은 실패가 아니다. 개별 cargo stdout은 미열람이다.

다음 검색 phase는 원본 Header 검색 버튼/입력/Enter→원본 SearchResultsSection을 사용한다. fixture 제목 검색1건·고유 absent query0건을 각각 제출/새로고침해 총4개 상태를 검사한다. 정확한 `Video.homeSearch` input(page1,size20,title,emotions empty)을 boolean으로 분류하고 단일 완료 body/contract/목록 길이·fixture 포함/렌더를 요구한다. raw 검색어를 네트워크 JSON에 저장하지 않는다. 검색에는 player 진입·업무 mutation이 없으며 종료되지 않은 API도 실패 처리한다. Rust는 로그인 세션 metadata와 별개로 11개 업무 테이블 모든 행·열의 검색 전후 snapshot 동일성을 검사한다. 원본 UI/policy/overlay 제품코드는 불변이다.

로컬: 새 helper stub에서 의도한 FAIL3→GREEN46, 기존 watch/comment/bookmark/like/logout branch AST 동일, readAdapter14 pass, browser AST·fmt·diff OK, PG* unset와 전용55432/aip_test_run DSN의 Rust auth `--no-run --locked --offline` compile6.27s exit0(실DB 아님). 실제 검색 browser/DB는 새 CI 전 미검증. 성공 후 계정잔여(check-email/signup/tutorial/email/password/confirmed withdrawal)의 원본 화면·기존 source overlay·메일 sink·DB/session 부수효과를 대조해 bounded 여정으로 이어간다. WBS90/25·25경로 전수미완·model.h5권리/visitor보류와 과거399638/399880 원인미확정 유지.

---

# 이전 재개 지점 (2026-10-24 07:17 KST, 댓글 UI 3단계 검증 추가·CI 판정 대기)

원본 사용자 UI 고정 SHA `2e7c021fa5abebd72278d841b8066a33d45cfbb3`의 CommentSection/CommentItem, SDK write adapter, Comment 정의 및 DB defaults를 대조했다. 원본 checkout과 앱 정책은 변경하지 않았다. 원본 등록 버튼→수정 메뉴/저장→삭제 메뉴/확인창을 각각 fresh browser phase로 실행하며, 정확한 SDK 단일 mutation·fixture 대상/내용·완료된 성공 body와 reload 후 렌더를 함께 요구한다. 수정됨 표시와 삭제 후 비노출도 검사한다.

Rust caller는 create/edit/delete 각 phase 직후 같은 comment ID, 회원/영상/내용, is_modified 0→1→1, is_deleted 0→0→1, 총 행 +1 유지(soft delete), 생성시각 보존을 검사한다. 무관한 모든 댓글 행의 ID·내용·플래그·시각 전체 snapshot을 비교해 타회원 보존을 별도로 판정한다. 단계별 browser JSON/PNG와 comment-db-result.json을 남긴다. 기존 bookmark/like/logout 분기의 AST는 동일하다.

로컬 검증: 새 판정기 RED 2 failures→GREEN, 중복된 다른 댓글 mutation negative-control도 RED→GREEN; evidence unittest43 OK, writeAdapter node test6 pass, browser AST/fmt/diff OK. Rust MySQL auth `--no-run --locked --offline` compile exit0(4.36s)이며 실DB 실행이 아니다. 최초 compile은 지정한 기본 포트5432를 프로젝트 guard가 거절했고, PG* unset 및 전용55432/test DB 계약으로 바로잡았다. 로컬 브라우저는 실행하지 않았고 실제 댓글 browser/DB 성공은 새 Linux 일반 Verify의 safe artifact로 판정한다.

진행 중 동일 ref CI에 push/dispatch/cancel 금지; workflow timeout45분 뒤 재개 예약으로 확인하며 장시간 대기하지 않는다. 성공이면 사용자25경로 comments행을 근거로 갱신하고 다음 search/계정 여정으로 진행한다. 현재 댓글UI는 미검증, WBS90/25·25경로 전수미완·model.h5권리/visitor보류 및 과거399638/399880 원인미확정 유지.

---

# 이전 재개 지점 (2026-10-24 07:05 KST, 좋아요 add/remove UI·DB 검증 성공)

코드 `1e7fda7dca0c4971e11d8324d4aee4c0502500c3`의 일반 Verify `40025235103` attempt1은 completed/success다. 직접 다운로드한 safe artifact `12534242371`에서 add/remove 모두 completed/page_error_count0/member_like.verified=true, mutation_response_count1/server_accepted=true, reload active true/false를 확인했다. 별도 like DB JSON의 browser/database status는 모두 success, 해당 회원 row는 add1/remove0이고 타회원 like count도 보존됐다. Rust caller의 초기0 assertion과 연결해 0→1→0을 확인한다. PNG에서도 활성/비활성 버튼·합계2→1을 직접 확인했다. 이번 lifecycle은 add33/remove37 requests·responses·finished 동일, failed0/unfinished0이다(이전 실행의 extension2 unfinished와 구분).

Profile/reissue browser·DB success와 USED refresh+1, logout browser·DB success 및 revoked session+1을 직접 다시 확인했다. Broad11·full E2E15·artifact16·cleanup18 success이며 locator는 마지막 관리자 E2E stage다. 사용자 API25경로 표의 like 행을 갱신했다. 과거 CI 원인을 소급 확정하지 않으며 WBS90/25,25경로 전수 미완,model.h5권리·visitor 보류는 유지한다.

다음은 원본 댓글 쓰기/수정/삭제의 UI→SDK→DB 검증이다. 현재 미커밋 제품 코드·활성 CI는 없고, 이 결과의 문서만 [skip ci]로 기록한다. 이번 단계는 조회/기록만 수행했으며 이전 41개 로컬 시험을 재실행했다고 주장하지 않는다. 컨텍스트 사용61%와 단계 전환을 고려해 기존 STATUS/VERIFICATION을 정본으로 두고 새 컨텍스트로 이어간다. 댓글 다음 잔여는 검색·계정 여정이며 단순 API 성공을 화면 검증으로 확대하지 않는다.

---

# 이전 재개 지점 (2026-10-24 06:10 KST, 로그아웃 UI/DB 검증 성공·좋아요 여정 준비)

코드 `a87748d469e4f58fc07fd7396ed0821e812ba060`의 일반 Verify `40017780097` attempt1은 completed/success다. 직접 받은 artifact `12530975676`에서 remove browser의 member_logout.verified=true, logout 응답1/서버수락/인증상태 초기화 true, `remove/logout-db-result.json` browser_status/database_status=success 및 revoked_session_rows_added=1을 확인했다. Profile/reissue browser·DB도 success, locator는 마지막 관리자 E2E stage다. visitor landing 정책은 판정하지 않는다. 사용자 API 경로 25개 표의 logout 행을 이 근거로 갱신했다.

다음 좋아요 add/remove는 원본 버튼과 SDK VideoLike 생성/삭제 성공 body, 새로고침 후 aria-pressed, MySQL member row 0→1→0 및 다른 회원 like count 보존을 검사하도록 추가했다. 아직 새 CI 전이므로 브라우저/DB 성공으로 세지 않는다. 댓글 쓰기·검색·계정 여정 등 잔여는 별도 미검증이다. WBS90/25·25경로 전수 미완·model.h5/visitor 보류 유지. 로컬 evidence unittest41/AST/fmt/diff는 통과했고 Rust MySQL auth test의 offline --no-run compile도 5.75s exit0이다(실DB 실행 아님).

---

# 이전 재개 지점 (2026-10-24 05:10 KST, 전체 E2E 복구 확인·회원 로그아웃 여정 추가)

코드 `50a012c9e844cfef3e5560838a61fc8bdd0add59`의 자동 Verify `40010266672` attempt1은 completed/success(19:41:55 UTC 종료)다. API에서 broad step11, full HTTP/worker/MySQL step15, artifact step16 및 fixture cleanup step18 success를 직접 확인했다. 이 push의 workflow는 `run-ci`를 선택한다. safe artifact `12526429724`의 locator stage는 마지막 `administrator console E2E test`다. 고정 script의 `set -euo pipefail`과 각 suite의 test-discovery/1-pass grep을 대조했으며 recommendation/watch/external effects/pool·trending·category jobs/admin lists/admin console까지 이어지는 일반 경로가 복구됐다. 개별 cargo stdout은 미열람이며 raw GH log cache 경로는 사용하지 않았다.

직접 받은 JSON에서 profile/member reissue browser·DB success, USED refresh +1, 만료 2→reissue 1→retry 2를 확인했다. Admin JSON은 dashboard·기간5개·역할/비활성화·승인/거절·영상/댓글 삭제·4개 권한거절을 기록하고, 별도 admin reissue는 2회 모두 재발급 1회/새 token 저장을 기록한다. 북마크 add/remove completed·page error0이며 remove의 `/extension` 2개 unfinished는 여전히 한계다. `verify-failure.txt`의 pending 값은 실패 증거가 아니다. 과거399880 scope종료예외/399638attempt2 원인은 소급 단정하지 않는다.

다음 잔여인 회원 로그아웃을 원본 버튼(헤더 마이페이지→로그아웃)으로 수행하는 E2E를 추가했다. 실제 성공 body와 인증 상태 초기화 boolean, 별도 MySQL revoked session +1을 요구한다. guest landing 정책/화면은 판정하지 않는다. 새 검증은 아직 CI 미실행이며 성공으로 세지 않는다. 로컬 evidence unittest40 OK, AST/fmt/diff OK, MySQL auth test `--no-run --locked --offline` 컴파일 성공(실DB 실행 아님). 최초 compile은 PGHOST 프로젝트 preflight에서 중단됐고 요구대로 PG*를 unset한 재실행은 exit0이다. 이번 변경을 push한 뒤 자동 Verify의 logout JSON과 DB 결과를 확인하고, 사용자25route의 comment/like/search 및 계정 여정 잔여로 이어간다. WBS90/25·model.h5권리·visitor 보류 유지.

---

# 이전 재개 지점 (2026-10-24 04:15 KST, 회원 UI/DB 성공 확인·일반 E2E 범위 복구)

코드 `d29c014146fa557acf6983208e0aa6f988af4e98`의 자동 Verify `40003138384` attempt 1은 completed/success다. 직접 다운로드한 artifact `12524337062`에서 profile browser/DB success, 필수 영역 4개 visually_ready=true, member reissue browser/DB success, 만료 거절 2회→reissue 1회→USED refresh row +1, 실제 `send`/`fetchWithRefresh`/`postWithToken` frames를 확인했다. 북마크 추가·해제도 completed, page error 0이다. 해제 여정 종료 시 `/extension` 2건 unfinished는 별도 남으며 모든 요청 종료를 주장하지 않는다. 과거 CI 실패의 구체 원인은 소급 확정하지 않는다.

다만 workflow 대조에서 `d29c014`가 일반 push/PR에도 무조건 `run-ci-member`를 호출한 회귀를 발견했다. broad gate는 통과했지만 recommendation/watch/effects/jobs/admin의 전체 MySQL E2E까지 통과한 실행으로 확대하지 않는다. 일반 실행은 `run-ci`, 명시적 `member_ui_diagnostic=true`만 `run-ci-member`를 선택하도록 복구했다. 실제 workflow shell의 빈 입력/false/true를 실행하는 회귀 시험이 수정 전 2개 실패, 수정 후 전체 evidence unittest 39개 및 YAML syntax/diff check 통과다. 복구 코드 `50a012c`의 전체 CI Verify `40010266672`는 success로 종료됐다(아래 최신 확인 참조). PMS 90%/25%, 사용자 25 route 미완, model.h5·visitor 보류는 유지한다. 상세는 product/VERIFICATION.md 마지막 절이다.

---

# 이전 재개 지점 (2026-10-24, reissue 종료 경계 확인·focused 계약 분리)

Verify `39988060660` attempt1, SHA `17d00728820c5852b566c4cf8991779b28e7cf36`는 step10 broad gate success, step14 HTTP/worker/MySQL/browser failure, artifact preservation success다. Safe artifact를 직접 내려받아 검사했다. `member-reissue-browser-result.json`은 `journey_stage=completed`, expiry refusal 2건, `/auth/reissue` 1건, retry 2건, page error 0; 별도 `reissue-db-result.json`은 `database_status=success`, USED refresh row +1, rotation observed=true다. 다만 DB 결과의 `browser_status=failure`라 프로세스 성공으로 합산하지 않는다. 같은 run의 별도 bookmark `add-browser-result.json`은 `bookmark_settle` AssertionError, lifecycle rows 0이다.

reissue branch는 report를 완료로 쓴 뒤 `sync_playwright()` 범위를 빠져나오고 공통 `finally`가 browser를 닫았다. `profile` branch는 반대로 드라이버 scope 안에서 context/browser를 닫고 참조를 비운다. 완료 JSON과 exit failure의 불일치가 scope 종료 예외로 생길 수 있다는 것은 코드로부터 도출한 가설이며, 브라우저 없이 만든 lifecycle negative-control과 순서 단위검사로 경계를 확인했다. 실제 CI에서 그 종료 예외가 발생했다고 증명되지는 않았다. 수정은 reissue의 성공 반환 전에 같은 드라이버 scope 안에서 context/browser를 정상 종료하고, 종료 예외를 실패로 보존한다. authAdapter 정책은 바꾸지 않았다.

Focused Verify `39994997512` attempt1, 같은 SHA는 API job/step 결과상 step11 CLI build, 12–13 pinned UI 준비 success, step14 real HTTP/DB test failure다. Artifact API `total_count=0`; 업로드 단계는 `No files were found`라 browser/reissue 실행 여부는 판별할 수 없다. Workflow는 native Mongo/Redis fixtures와 broad gate를 skip하면서 `run-ci` 전체를 불렀고, 뒤의 recommendation/watch/effects/admin test들도 실행하며 Mongo/Redis URL을 요구한다. URL은 fixture를 시작하지 않은 focused run에서 준비되지 않은 loopback service를 가리킬 수 있다. 이것은 workflow/script에서 확인한 실행계약 불일치이며, 399949의 첫 실패 명령이 그 단계였다는 주장은 미확인이다.

이 수정은 `run-ci-member`로 MySQL worker-write와 member auth/UI 검증만 돌리고 종료하게 분리한다. 기존 `run-ci` full suite와 Mongo/Redis fixtures 및 일반 workflow gate는 유지한다. Safe locator directory는 workflow 시작 직후 만들고, 실패 시 stage·체크아웃 shell의 literal command·exit status만 남긴다. stdout/stderr, 환경값, token, cookie, DB URL은 쓰지 않는다. 주요 build/UI preparation/test 경계마다 stage를 갱신한다. 이전 focused 실패 artifact는 없으므로 첫 원인 command는 이 변경 전에는 확인 불가로 남긴다.

Local evidence at that worker run: browser evidence unittest 38/38, profile script AST parse, Ruby YAML syntax parse, `bash -n` on safe locator/E2E/broad verification scripts, `git diff --check` all pass. Browser/DB E2E and macOS browser were not run locally. Subsequently pushed as `d29c014`; Verify `40003138384` and its limited E2E scope are recorded above. WBS 7.2.9 `doing/90%`, 7.2.10 `doing/25%`; route 25 incomplete, model.h5/visitor deferred. Engine auto/general unchanged. Current tool catalog has no `account_usage` or routing-receipt lookup; the provided 03:16 KST usage snapshot is not independently refreshed here.

---

첫 진단 commit `91ca346314e1ac7bd42493acbb06f5fae10eda19`의 Verify `39973060321` attempt 1을 artifact까지 확인했다. step10 compiler/runtime/product/worker/transport/SDK와 pinned UI 준비 step11–12는 success, step13 browser/worker/MySQL는 failure, artifact 저장·fixture cleanup step14–16은 success다. profile journey는 `/my`, API body settled=true에서 저장 후 즉시 캡처한 화면의 body text length 133을 남겼다. 그 캡처 순간 profile 영역 opacity/effective opacity는 0–0.21, `fadeInUp` running이고 이후 animation 종료 측정에서 필수 영역 4개는 모두 opacity 1·visually_ready=true다. 실제 before-animation screenshot도 어둡게 비어 있다. 이는 screenshot이 500ms fade animation 중에 찍히는 것이 빈 화면의 직접 원인이라는 강한 측정 증거다. 최종 after-animation screenshot은 넓은 페이지의 무관한 1.4초 animation을 금지한 잘못된 assertion(line 469) 전에 생성되지 않아 CI로 한 번 더 확인한다.

같은 Verify에서 profile DB persistence는 success(저장 이름 및 장르 2개 일치, 다른 회원 marker 보존)였고 screenshot/browser는 failure다. member reissue journey는 같은 fixture를 공유하는 profile 여정이 표시 이름을 변경했는데도 기존 원래 이름을 기대해 line 339 heading assertion에서 멈췄다. 이 시점 `api_requests=[]`, `/auth/reissue` 관측 없음, DB USED refresh delta 0은 reissue 제품 결함의 증거가 아니라 여정 선행조건 불일치다. 새 변경은 reissue 시작 뒤 API body 수집과 networkidle을 먼저 기다리고, profile 저장된 이름을 검증하며, assertion 전에 안전 trace를 저장한다. trace에는 failure type/assertion/line, content type/envelope, Authorization 존재 boolean, allowlist SDK/auth frames, response body/networkidle 타임라인을 기록하고 알려지지 않은 path는 `other`로 만든다. token/header/body 원문은 저장하지 않는다.

profile gating은 관련 `fadeInUp` animation 완료와 실제 4개 영역 visibility만 본다. final screenshot을 측정 직후 보존하며 무관한 animation 상태는 성공 조건으로 쓰지 않는다. capture 시점 API body는 이미 완료되고 body finish는 `networkidle`보다 앞섰으므로, 이 관측은 API/networkidle 경쟁조건이 아닌 fade 시점 차이를 지지한다. 최종 screenshot 확인과 reissue transport 분기는 수정 SHA CI 전까지 미확정이다.

새 수정의 로컬 검사: browser evidence unittest 32/32, authAdapter/mypageAdapter Node tests 12/12, profile script AST parse, `cargo fmt --all --check`, `git diff --check` 모두 통과했다. Rust test compile은 직전 SHA에서 explicit local test DSN의 `--no-run --locked` 성공으로 확인했으며 live DB/browser는 아님. configured browser에는 `about:blank`뿐이라 local UI는 미실행이다. Verify `39973060321` attempt1 원인 산출물은 이 Run outbox `aip-run-39973060321-attempt-1/facereview-ui-browser-39973060321-1/`에 있다. attempt1 log cache 거부 경로는 여전히 재시도하지 않았다.

Verify `39982476627` attempt1, SHA `27db2af55eb65b71cdac65248d0dce04ee11ffa2`는 step10–12 success, step13 failure, artifact·fixture cleanup step14–16 success다. profile browser/DB와 member reissue browser/DB 결과는 모두 각각 success했다. Member UI의 실제 edit→save after TTL에서 `/apply`와 concurrent `/extension`이 HTTP 200 JSON `WIRE.TOKEN_EXPIRED`로 두 번 거절됐고 Authorization 존재=true, body 수집=true, 두 body 모두 `networkidle` 전 완료. 이어 `/auth/reissue` HTTP 200 JSON `ok=true` 1회, MySQL USED refresh row +1, token digest 변경, 후속 UI/read/write 성공 및 `/my` 화면 복귀를 확인했다. 이 자연 UI write에서는 member auth reissue가 동작한다. 다만 새 injected fetch/XHR hook이 실행되지 않아 `fetch_hook_installed=false`, frames 0; 실제 function stack은 미측정이다. 옛 attempt2에서 reissue 0이었던 구체 차이는 아직 설명 못한다. 이전 테스트의 이름 불일치와 cached read 문제는 evidence와 맞지만 옛 artifact에 auth header/content type이 없으므로 옛 원인으로 확정하지 않는다. 동일 run의 step13 전체 failure는 target reissue 뒤 add-bookmark `bookmark_settle` 별도 failure이며, 화면은 bookmark-saved 표시지만 lifecycle evidence가 비었다. WBS 범위 밖 수정은 보류한다.

다음 재개점은 browser init script의 arrow function을 실제로 호출해 stack allowlist/fetch-XHR observation을 확보하는 새 SHA Verify다. 만료 branch 입력과 DB token rotation은 측정됐지만 `fetchWithRefresh`/SDK function frames는 아직 없다. authAdapter/SDK source policy는 수정하지 않는다. `c64dd28` run의 step10 success로 locator는 생성되지 않았으나 trap/wiring은 local control로 확인했다. WBS 7.2.9 `doing/90%`, 7.2.10 `doing/25%`; 사용자 25 route 미완료, model.h5/visitor 보류는 유지한다.

---

# 직전 재개 지점 (2026-10-23, Verify step10 재실행 통과·member browser reissue 실패; profile 미실행)

같은 SHA `066187ad9d607b29e0770e1fadd07f12581c6644`의 Verify `39963850568` attempt 2는 `completed/failure`다. 앞선 step10 전체 compiler/runtime/product/worker/transport/SDK 검증은 19분 실행 후 success, pinned member UI 및 admin 준비 step11–12도 success였다. step13의 실제 HTTP/worker/MySQL/browser journey에서 실패했고 artifact 보존과 run-owned native fixture cleanup step14–16은 success다. CI run은 fixture 정리를 마쳤다.

attempt2 browser artifact `profile-browser-result.json`은 로그인·session·User/Profile genre reads와 확장 요청에서 HTTP 200을 기록했고, TTL 이후 화면 경로에서 `/apply`와 `/extension`이 HTTP 200 `WIRE.TOKEN_EXPIRED`로 거절된 것을 기록한다. 해당 expiry 구간에 `/auth/reissue` 요청은 0건, `journey_stage=member_token_expired`, `failure_type=AssertionError`, page error 0, `profile_ui={}`다. 코드 실행 순서상 거절은 발견됐으나 reissue 1회 조건에서 멈춘 것으로 판단된다(artifact에는 예외 문구가 없어 이 branch attribution은 추론). profile 저장·computed style·animation 계측은 이 journey가 먼저 중단되어 전혀 실행되지 않았고 DB post-save assertion 및 screenshot도 없다. reissue를 호출하지 않은 adapter 판단의 더 구체적 이유는 artifact에 응답 header가 없어 미확인이다. 고정 원본의 fadeInUp opacity 코드는 관측 사실일 뿐 공백 원인 가설은 계속 미확정이다.

attempt1 step10 실패 원문은 `gh run view ... --log-failed` 시 Codex nested command sandbox가 거부한 동일한 GH CLI cache path 때문에 미확인으로 남는다. 해당 경로를 다른 방식으로 재시도하지 않았다. safe artifact 내려받기 `gh run download 39963850568 --name facereview-ui-browser-39963850568-2 --dir <이 Run outbox>/aip-run-39963850568-attempt-2-browser`는 성공했다. step10 원인을 attempt1의 exit 101이나 Ubuntu 공지로 단정하지 않는다.

로컬에서는 browser evidence unittest 28개, authAdapter/mypageAdapter Node test 12개, profile script AST parse, 영향받은 Rust `aip-cli` MySQL auth clippy를 통과했다. step10의 broad Node/Python/DB 관련 subset을 Codex nested sandbox에서 실행할 때는 PGHOST preflight, uv cache 쓰기 거부 및 로컬 5433 connection refused가 각각 선행 제약으로 나타났다. 이들은 CI step10의 원인 증거가 아니다. 이 Run이 만든 isolated Cargo target 1.3 GiB는 제거했다. Codex engine usage/실제 routing receipt를 조회할 tool은 현재 실행에서 제공되지 않았다.

WBS 7.2.9 `doing/90%`, 7.2.10 `doing/25%`와 분모(관리자 feature API 13: 읽기 7/쓰기 6, shared auth 4, 화면 5, 사용자 고유 route 25)는 변경하지 않는다. model.h5 권리·visitor 정책은 계속 보류, 사용자 route 25 전수는 미완료다.

---

# 직전 재개 지점 (2026-10-23, member profile 렌더 진단·만료 reissue 여정 준비, Verify failed / UI·DB 미검증)

코드 SHA `066187ad9d607b29e0770e1fadd07f12581c6644`의 Verify `39963850568` attempt 1은 `completed/failure`다. step 10 `Verify compiler, runtime, product, worker, transport and SDK`가 실패했고 11~13은 skip되어 실제 pinned member UI·전용 DB·브라우저 여정은 실행되지 않았다. 실패 job의 원문은 Codex nested command sandbox에서 `gh run view 39963850568 --log-failed`가 `/Users/winterholic/.cache/gh/run-log-39963850568-1792763124.zip: operation not permitted`로 캐시 생성 단계에서 거부되어 미확인이다. 이는 해당 Codex 명령 실행의 캐시 권한 범위만 말하며 CI 실패 원인을 판정하지 않는다. 같은 로그 경로는 재시도하지 않았다.

이번 커밋은 profile save 전후의 computed style/ancestor opacity, animation·transition·Web Animations API, route·visibility·API 완료를 안전하게 기록하고 실제 profile animation `finished`를 기다리는 진단을 추가했다. 별도 20초 TTL 서비스에 같은 test-owned MySQL 계약으로 만료→원본 버튼 클릭→`WIRE.TOKEN_EXPIRED`→reissue 1회→화면 복귀를 검증하도록 browser journey를 추가했다. **둘 다 CI step 13이 skip되어 미실행**이다. 고정 원본 `mypage.scss`/`_motion.scss`에 profile `fadeInUp` opacity 0→1 animation이 있다는 것은 관측된 코드 사실이고, 이것이 빈 화면의 원인이라는 설명은 아직 추론뿐이다. 기존 DB 이름/장르 성공과 DOM text length 133 헤더 전용 screenshot은 유지하되 원인 판정으로 승격하지 않는다. 필요한 최소 제품 수정은 아직 하지 않았다.

로컬 확인: `PYTHONDONTWRITEBYTECODE=1 python3 -m unittest product.tests.test_facereview_ui_browser_evidence` → 28 tests OK; `node --test porting/facereview/ui-overlay/tests/authAdapter.test.ts` → 10/10 pass; `python3 -c 'import ast; ast.parse(...)'` profile browser script parse OK; `cargo fmt --all --check && git diff --check` → exit 0; Rust MySQL auth test `--no-run --locked` → compile success (DB/browser 실행 아님). 원본 체크아웃은 읽기만 했다. 결과·관측 경계는 [제품 검증](../product/VERIFICATION.md#2026-10-23-member-profile-렌더-및-token-reissue-브라우저-진단-코드-sha-066187ad9d607b29e0770e1fadd07f12581c6644)에 있다.

Verify 재개 전 해결할 항목: step 10 실패 원문을 허용된 실행 경로로 확인하고 원인을 고친 뒤, **같은 SHA의 CI 종료 후** 실제 UI/DB/browser step을 수행한다. 중간 대기나 403이 발생하면 기획에서 schedule_action하도록 reset을 보고한다. WBS 7.2.9 `doing/90%`, 7.2.10 `doing/25%` 및 분모(관리자 feature API 13: 읽기 7/쓰기 6, shared auth 4, 화면 5, 사용자 고유 route 25)는 유지한다. model.h5 권리와 visitor 정책 보류. 사용자 25 route 전수는 미완료다.

---

# 이전 재개 지점 (2026-10-23, WBS 7.2.9/7.2.10 관리자 ADMIN 해제·기간·token reissue, 코드 SHA `b5cd719` Verify green)

코드 SHA `b5cd719d76e6af9d94f889c4899ab3ddea260e16`의 Verify `39957123455`는 attempt 1에서 `success`다(같은 SHA의 나머지 6개 workflow도 success). 이전 미검증 3개를 원본 `ab0386c`의 UI(`ADMIN 해제` 버튼, 5개 기간 버튼, 401→`/v2/auth/reissue` 인터셉터)에 맞춰 실제 UI·실서버·전용 DB·브라우저로 닫았다. (1) SUPER_ADMIN의 ADMIN→GENERAL: DB `ADMIN|0`에서 시작해 종료 `GENERAL|0`, 일반 ADMIN에게는 역할 버튼 없음. (2) 대시보드 기간 30일·3달·1년·3년·7일: 고정 가입일 시드로 버킷 합 3/4/5/6/2와 차트 점 수를 단정. (3) token reissue: 서버가 만료를 HTTP 401이 아닌 `200 + WIRE.TOKEN_EXPIRED`로 답해 공유 `authAdapter.ts`가 reissue를 한 번도 하지 않는 **실제 결함**을 브라우저로 재현(수정 전 `TOKEN_EXPIRED` 2건, reissue 0건, 로딩 화면 정지)하고 시험 선행(5 fail)으로 고쳤다. 수정 뒤 TTL 20초 서비스에서 만료 2회 모두 거절 1, reissue 정확히 1, 이후 요청 ok, 새 token 저장을 CI Linux가 확인했다. 상세와 한계는 [제품 검증](../product/VERIFICATION.md#후속-admingeneral-7d-외-기간-토큰-reissue-코드-sha-b5cd719d76e6af9d94f889c4899ab3ddea260e16).

아직 미검증: 거절된 reissue(refresh token 만료) 뒤 로그인 이동의 브라우저 여정(단위 시험까지만), member 화면의 만료 reissue 재현, 원본 오류 문구 동등성, 사용자 25 route 전체, profile after-save 빈 화면(별도 미해결), model.h5·visitor 정책(보류). PMS 7.2.9 `doing/90%`, 7.2.10 `doing/25%` 유지. 분모는 관리자 feature API 13개(읽기 7/쓰기 6), shared auth 4개, 화면 5개, 사용자 고유 경로 25개로 별도다.

---

# 이전 재개 지점 (2026-10-23, 관리자 에러 경계 live 검증, 코드 SHA `0acf327` Verify green)

코드 SHA `0acf32756a0e0cca38be9a83668022a686cadb52`의 Verify `39947955420`은 attempt 1에서 `success`다(같은 SHA의 나머지 6개 workflow도 success). 직전 `1e528aa`의 Verify `39939106930` 실패는 원문 확보로 확정했다: step 13 브라우저 여정의 `closed_request_refusal`에서 시험이 `WIRE.EFFECT_TARGET_MISSING`을 기대했으나 서버는 `WIRE.INVALID_STATE`를 반환했다(artifact와 서버 로그 일치). 시험 기대값 오류였고 정책·권한·flaky가 아니다. 기대값 정렬과 직전 Run들의 보존된 작업 트리 변경(세션 SDK client, job 캐시 무효화, null metrics 타입)을 한 커밋 `0acf327`로 push했다.

증거: 로컬(Claude 엔진 Mac, run-owned 컨테이너)에서 live 서버·DB 시험 5.34s 통과, 브라우저 포함 19.83s 통과(null metrics mounted render 확인), CI Linux에서 같은 여정과 ADMIN의 ADMIN/SUPER_ADMIN 대상 FORBIDDEN 4건, 종료 요청·삭제 영상 재결정 거부 3종이 통과했다. 이전 `SERVER_CONFIG`는 Codex nested sandbox 범위의 한계로 분리했고 이 환경에서는 재현되지 않았다. 상세는 [제품 검증](../product/VERIFICATION.md#2026-10-23-관리자-에러-경계-live-검증-코드-sha-0acf32756a0e0cca38be9a83668022a686cadb52).

미검증·보류: ADMIN→GENERAL 방향, 기간 7d 외, 토큰 reissue, 원본 오류 문구 동등성, 사용자 25 route 전체, profile after-save 빈 화면(별도 미해결), model.h5·visitor 정책. PMS 7.2.9 `doing/90%`, 7.2.10 `doing/25%` 유지. 관리자 feature API 13개(읽기 7/쓰기 6), shared auth 4개, feature 화면 5개, 사용자 고유 경로 25개는 서로 별도 분모다.

---

# 이전 재개 지점 (2026-10-23, WBS 7.2.9/7.2.10 관리자 권한·상태 검증 후속, 코드 SHA `1e528aa` Verify failed; 위 `0acf327`로 원인 확정·해소)

코드 SHA `1e528aa4a2c47ca02a453c1c4f1f7c0075e5380f`는 origin/main과 일치하고 작업 트리가 clean이다. Verify `39939106930`은 public Actions 요약에서 실패, core job exit code 101로 확인했다. 같은 SHA에서 step 10~12는 success였고 step 13은 당시 진행 중이었다. 이후 API는 rate limit 403, public job 로그는 로그인 필요라 정확한 실패 step·원문·브라우저 artifact 내용은 미확인이다. 실패 원인을 권한 assertion이나 flaky로 분류하지 않았고 새 코드 push/재실행도 하지 않았다.

정책 대조는 고정 원본 `new-facereview-admin-front` `ab0386c878a5ba5c7c079a262b987c10d211d22d`의 API exports/calls와 현재 AIP 선언을 따라갔다. 원본 정의는 SUPER_ADMIN의 GENERAL→ADMIN 및 ADMIN→GENERAL을 제공하고 ADMIN은 GENERAL만 비활성화하며 ADMIN/SUPER_ADMIN 대상은 허용하지 않는다. 기존 AIP 선언도 같은 정책이다. 삭제 영상 재삭제와 이미 종료된 요청 재결정은 원본이 거절하는 경계로 분류했고, 새 slice는 전용 fixture·실서버·브라우저·DB 상태 단언으로 닫으려 했으나 local service startup이 generic `SERVER_CONFIG`로 실패했고 CI도 최종 실패해 이 동작들의 live/browser/DB 증거는 아직 없다. 정책을 임의 변경하지 않았다. 상세는 [제품 검증 후속](../product/VERIFICATION.md#2026-10-23-관리자-권한경계-null-metrics-및-원본-api-분모-정정-코드-sha-1e528aa)을 참조한다.

원본 API inventory를 다시 셌다: 관리자 feature API는 13개(읽기 7, 쓰기 6), shared auth API는 4개이며 `/auth/me`는 이 shared auth 쪽이다. 따라서 관리자 읽기 분모는 7이고 `/auth/me`를 8번째로 합산한 예전 해석은 정정한다. 5개 feature 화면·13개 관리자 API·사용자 25개 고유 경로는 별도 분모다. Linux 원본 contract가 host metrics를 null로 낼 수 있어 대시보드 overlay는 수치를 만들지 않고 부분 상태를 표시하도록 추가했으나, 이번 새 branch의 browser CI 실행 결과는 미확인이다. 관리자 token reissue와 7d 외 기간도 미검증이다.

실행 근거: `node --test porting/facereview/ui-overlay/tests/adminDashboardStatus.test.ts porting/facereview/ui-overlay/tests/adminAdapter.test.ts porting/facereview/ui-overlay/tests/adminReadAdapter.test.ts` 24/24; browser evidence unittest 25 OK; SDK aggregate/jobs 회귀 21/21; Rust 실서비스 통합 시험은 `--no-run` compile 성공. local ignored live test 실행은 서버가 `SERVER_CONFIG`로 기동 실패해 정책 probe 전 중단됐다. macOS browser preflight blocked라 local browser 미실행. run-owned Docker fixtures는 제거했고 `docker ps -a --filter label=aip.admin-ui.run=20261023-admin-auth`는 비어 있었다. `/private/tmp/aip-adminui-20261023-*` 및 `/private/tmp/aip-sdk-0.1.0.tgz`는 삭제 시 safety guard가 거부했고 그대로 남아 있다(다른 방식으로 재시도하지 않음). `aip-p730-*`, 다른 작업의 5173은 불변. 다음 재개점은 CI failure 원문을 접근 가능한 경로로 확보하는 것, local fixture의 `SERVER_CONFIG` 실패 원인 규명, permission fixture/browser/DB assertion 결과 수집이다. profile after-save 빈 화면 원인은 별도 미확정으로 남긴다. WBS 진행률은 7.2.9 doing/90%, 7.2.10 doing/25% 그대로다. model 권리와 visitor policy는 계속 보류한다.

---

# 이전 최신 재개 지점 (2026-10-23, WBS 7.2.9/7.2.10 관리자 콘솔 SDK 전환, 코드 SHA `e4f2efe` Verify green)

코드 SHA `e4f2efe97a7f712c3c8438abfdb8f5eb4e43a4d3`(origin/main 일치)의 Verify `39932769334`는 `success`(step 10~14 success)다. 고정 관리자 원본 `new-facereview-admin-front` `ab0386c`의 data access를 생성 TS SDK와 서버 정책으로 전환했다. 이전 "checkout 없음" 보고는 사실이 아니었고 로컬 `~/development/projects/facereview/new-facereview-admin-front`가 고정 commit과 같다(원본은 수정하지 않고 임시 clone에 overlay 적용).

구현: 읽기 7개와 `/auth/me`(`adminReadAdapter.ts`), 승인 durable job adapter(`adminAdapter.ts`), 기존 쓰기 5개 유지, 적용 스크립트 `porting/facereview/apply-admin-ui-overlay.sh`, 실서비스 시험 `mysql_facereview_admin_ui.rs`, 별도 브라우저 여정 `facereview-ui-admin-browser.py`, `verify.yml`의 관리자 UI 준비·build 단계. SDK 결함 2건을 실제 콘솔로 발견해 SDK에서 고쳤다: 독립 `aggregate()` 호출 부재(와 `filter: []` 거절), job 입출력의 `Uuid` 스칼라 미지원. 상세와 원본 대조표는 [제품 검증](../product/VERIFICATION.md) 끝의 관리자 콘솔 절이다.

검증: 로컬 macOS에서 서버 정책(일반 회원, 비인증은 정확한 code로 거절, 관리자는 대조군 통과), 고정 admin UI `tsc -b`/build, 브라우저 여정(쓰기 5개, 승인 job, DB·Mongo 단언). CI Linux에서 같은 여정과 대시보드 렌더(`rendered`, 오류 0, 요청 82개 모두 ok)까지 통과. 미검증: 기간 선택 7d 외, 토큰 reissue, `ADMIN 지정` 방향, ADMIN 계정의 GENERAL 한정 비활성화 브라우저 시험, 삭제된 영상 재삭제 같은 원본 오류 경계, 성공 메시지 문구의 원본 대조. profile after-save 화면 갭은 이번 일과 별도로 열려 있다. 이전 dirty clone 보존 patch(Run `a14a5dd2…`의 `aip-ui-dirty-clone-preservation/`)는 가드 거부와 빈 artifact 목록으로 확인하지 못했다(미확인).

정리: repo `target` 3.0G를 `cargo clean`으로 회수(4.6G에서 1.6G), 이 Run의 컨테이너 3개와 `/tmp/aip-admin-run`(overlay 적용 clone은 저장소 파일과 `cmp`로 동일함을 확인한 뒤)을 삭제했다. 소유가 불명한 `aip-p730-*`, `aip-fixture-*-p730` 컨테이너와 다른 `/private/tmp/aip-*` 항목, 5173(다른 프로젝트 dev server)은 건드리지 않았다. 다음 순서: 7.2.10의 남은 사용자 화면과 위 미검증 항목, 7.2.6의 model.h5·visitor 정책은 계속 보류.

---

# 최신 재개 지점 (2026-10-23 09:05 UTC, browser journey 수정 SHA `d6718c4` Verify green)

코드 SHA `d6718c40a09952939ad90336c2c4595fefc063bb`(origin/main 일치)의 Verify `39922363131`은 attempt 2에서 `success`다. 직전 조사의 최소 수정안을 member와 profile에 한 커밋으로 적용했다. member: 클릭 전에 `.video-information-container .title`에 fixture 제목이 보일 때까지 기다리고, 클릭 뒤 북마크 `/read`와 `/apply`가 `requestfinished`(본문 수집 포함)에 이를 때까지 기다린 다음 reload하고 `aria-pressed`를 단정한다. profile: 본문을 `requestfinished`에서 읽고, 저장 `/apply`의 본문 수집을 기다린 뒤 목적지 페이지의 읽기와 `networkidle`까지 기다려 screenshot을 찍고 화면 텍스트 길이를 기록한다. 단정 완화, mock, 숨은 버튼 조작, 제품 코드 변경은 없다.

로컬 검증: `python3 -m unittest product.tests.test_facereview_ui_browser_evidence` 22/22. 가짜 UI/API(실제 Chromium, Playwright 1.64)에서 수정 전 스크립트는 `Video.view`가 느리면 `bookmark_action`에서 미눌림으로 실패(CI attempt 2 재현), 낙관적 flip 뒤 느린 `/read`는 reload에 끊겨 `bookmark_reloaded`에서 실패했고 수정본은 둘 다 통과했다. 수정본은 add/remove/profile을 한 가짜 API에서 연속 통과했다.

CI: attempt 1은 step 12의 앞선 시험 `crates/aip-cli/tests/mysql_worker_write_e2e.rs:433`(`http_apply_runs_real_node_and_python_workers_in_one_mysql_transaction`)이 `retryJob`의 응답 `WIRE.DB_UNAVAILABLE`(`retry: later`)로 실패해 브라우저 시험이 실행되지 못했다. 같은 SHA로 실패 job만 재실행(attempt 2)하자 step 10~13이 통과했다. attempt 2 브라우저 증거: add/remove 모두 `journey_stage=completed`, 북마크 `/read`는 앱 수신과 `match`, settle이 reload 시작 전(`bookmark_requests_settled` < `reload_start`), `add-bookmarked.png`에 제목, `저장됨`, 조회수 11이 보인다. profile의 MySQL 이름·장르 assertion은 통과했고 모든 body가 수집됐다. 다만 `profile-after-save.png`는 `after_save_visible_text_length=133`인데도 화면은 헤더만 보이는 빈 본문이다. DOM에는 텍스트가 있으므로 이전 가설(로딩 중 캡처)은 틀렸고, 렌더링 지연(예: 투명도 전환 애니메이션)은 미확인 후보다. 이 after 화면은 계속 증거로 세지 않는다. 다음 조치 후보는 heading의 computed opacity와 viewport screenshot 시점을 기록하는 관측 보강이다. remove의 `unfinished` 1건은 reload 뒤 새 페이지의 `/extension`이 종료 시점에 진행 중이던 것으로 bookmark 요청이 아니다.

간헐 실패 후보(분석만, 수정 안 함): (1) Verify `39915678255` attempt 1, `spikes/spike-v3-write/tests/absence_serializable.rs:78`, 기대 `["OK","WIRE.FORBIDDEN"]`에 `["OK","WIRE.CONFLICT"]`가 나옴(재시도 소진 추정, 같은 SHA 재실행에서 통과). (2) Verify `39922363131` attempt 1, `mysql_worker_write_e2e.rs:433`, `WIRE.DB_UNAVAILABLE retry later`(일시 DB 불가용 추정, 재실행에서 통과). 둘 다 이번 변경(Python 시험 파일)과 무관하고 각 attempt 1 원문은 outbox `ci-39915678255-attempt1/`, `ci-39922363131-attempt1/`에 보존했다. 재발하면 해당 run의 원문을 보존하고 원인 분석만 한다.

GitHub artifact `12486124697`, `12490352539`는 원시 증거라 삭제하지 않았다. `/private/tmp`의 이전 ZIP·압축 해제 폴더·UI clone은 삭제했고, 소유가 불명한 `aip-p730-*` 컨테이너와 캐시 디렉터리는 건드리지 않았다. 관리자 UI slice(적용 스크립트, 읽기 8개, 승인 job adapter)는 Verify green 조건을 충족했으나 시작하지 않았다. 관리자 UI는 별도 저장소 `new-facereview-admin-front`(고정 ab0386c)이고 이 환경에 checkout이 없어, 읽기 8개의 화면 쪽 응답 모양을 확인하려면 먼저 그 저장소를 고정 revision으로 가져와야 한다(미확인: 접근 권한). 다음 재개점은 그 checkout 위치 확인과 `adminAdapter.ts`의 읽기 8개·승인 job 어댑터 단위 시험이다. 이후 push는 한 번에 묶고 Verify 완료까지 기다린다. model.h5와 visitor policy는 계속 보류한다.

---

# 이전 재개 지점 (2026-10-23, FaceReview browser `/read` 측정기 조사, 계측 SHA `5a42b20`)

이전 `invalid_json`은 서버 non-JSON의 증거가 아니었다. `facereview-ui-member-browser.py`는 `response` 이벤트 안에서 `response.json()`을 호출하고 모든 Exception을 `invalid_json`으로 기록했다. 합성 서버와 같은 Playwright 1.64/Chromium 1248에서 reload나 close와 겹친 응답이 `Network.getResponseBody: No resource with given identifier`(resource_evicted), `TargetClosedError`, 요청이 종료 이벤트 없이 사라지는 경우로 실패했고 모두 같은 라벨이 됐다. 진짜 HTML 본문은 별도 분류(`json_decode_error`)로 재현했다.

계측 SHA `5a42b204eaac4a58bd89778d0ba12aa99a1488dd`는 본문을 `requestfinished`에서 읽고, JSONDecodeError와 body 취득 실패를 reason code로 나누며, 요청 lifecycle·reload/close timeline·앱 쪽 fetch 수신(길이, sha256 16자, 허용 shape)을 기록한다. Verify `39915678255`는 attempt 1이 step 10의 Rust 동시성 시험 `spike-v3-write/tests/absence_serializable.rs:78`(기대 `FORBIDDEN`, 실제 `CONFLICT`; 재시도 소진으로 추정)에서 실패했다. 이 시험은 직전 SHA 4개 run에서 통과했고 이번 변경은 Python 시험 파일뿐이다. 같은 SHA로 실패 job만 재실행(attempt 2)했고 step 10·11은 통과, step 12에서 실패했다.

attempt 2의 새 증거: 요청 15개 모두 종료, 본문 전부 취득, VideoBookmark `/read` 2건은 JSON object이며 앱 수신과 길이·해시가 `match`다. `invalid_json`은 재현되지 않았다. 실패는 add 단계 `bookmark_action`의 `aria-pressed=true` 단정이다. 서버는 북마크 생성 `/apply`까지 200으로 처리했지만 UI는 `저장`(미눌림)이고 제목과 조회수 영역이 비어 있다. 핀된 UI 코드상 `getVideoDetail`은 `Video.watchDetail` 뒤 `Video.view` apply가 끝나야 resolve되고, `bookmarkMutation.onMutate`는 `cancelQueries`로 진행 중인 videoDetail을 버리며 낙관적 갱신은 기존 데이터가 있을 때만 일어난다. 클릭이 videoDetail 완료 전에 오면 낙관적 flip이 없고 조회도 버려진다(코드 읽기와 화면에 근거한 추론, 클릭-조회 타이밍의 직접 로그는 없음).

원인 판단: (1) 측정기가 body 실패를 서버 오류처럼 기록했고 sync handler가 다음 Playwright 호출 때에야 실행돼 row가 늦거나 사라졌다(확인). (2) 테스트 흐름이 낙관적 `aria-pressed` flip을 완료 신호로 쓰고 곧바로 reload한다. 그래서 `39910753805`처럼 진행 중인 read/apply가 reload에 끊기는 경우와, 이번처럼 클릭이 videoDetail 로딩보다 앞서 flip이 없는 경우가 클릭 타이밍에 따라 갈린다(추론, `39910753805`의 body 실패 사유는 당시 계측이 없어 미확정). 서버가 non-JSON을 반환했다는 증거는 없다.

최소 수정안(미적용, 이번 단계는 관측 보강에 한정): 클릭 전에 `.video-information-container .title`에 fixture 제목이 보일 때까지 기다린다(`Video.view` 완료 뒤). 클릭 뒤에는 reload 전에 북마크 `/read`와 `/apply`의 종료 이벤트(lifecycle 기준)를 기다리고, 그 뒤에만 reload와 `aria-pressed` 단정을 한다. 단정 완화, API mock, 숨은 버튼 조작은 쓰지 않는다. profile 스크립트도 같은 패턴(`response.json()`을 handler 안에서 호출, 이동 직후 screenshot)이며 after 화면이 두 run 연속 비어 있어 같은 수정 대상이다. MySQL 이름·장르 assertion은 통과했지만 after 화면은 증거로 세지 않는다.

로컬 검증: `PYTHONDONTWRITEBYTECODE=1 python3 -m unittest product.tests.test_facereview_ui_browser_evidence` 22/22. 가짜 UI/API로 `main()` 전체를 실제 Chromium에서 실행해 add·remove 정상(app 수신 `match`), 서버 HTML 응답(`json_decode_error`), reload 겹침(200 row + `not_finished_before_report` + 앱 수신 없음)을 구분함을 확인했다. 재현 도구는 outbox `aip-measurement-repro/`, CI 원시 증거는 `ci-39915678255-attempt1/`·`attempt2/`에 있다. GitHub artifact `12486124697`은 원시 증거라 삭제하지 않았다. 관리자 UI slice, model.h5, visitor policy는 계속 보류한다. 다음 재개점은 위 최소 수정안을 한 번에 적용하고 로컬 가짜 스택으로 검증한 뒤, 한 번 push하고 Verify 완료까지 기다리는 것이다.

---

# 이전 재개 지점 (2026-10-23 06:44 UTC, 두 번째 수정 Verify 39910753805 실패)

Verify `39910753805`(SHA `bd3ef4abc6bdc678f939134e78fd4bea08e94f80`)는 세 번째 대기 시도 중 `failure`로 끝났다. step 12(`Verify real HTTP to Node and Python workers to MySQL`)에서 `mysql_facereview_auth.rs:611`이 React add-bookmark browser script의 실패를 받아 panic했다. 명령 `gh run view 39910753805 --log-failed`는 앞선 실행에서 Codex Mac nested worker의 gh log cache 쓰기가 `operation not permitted`였으므로 반복하지 않았고, `gh api repos/winterholic/Application-Intent-Protocol/actions/runs/39910753805/logs` 원문을 조사했다.

이 두 번째 수정은 safe evidence에 top-level response shape를 추가했다. 실패 요청은 `VideoBookmark` POST `/read`, HTTP 200, contract fingerprint 일치, 유효한 query key·field였지만 response JSON 파싱이 예외를 내 `response_shape: {"json_kind":"invalid_json"}`으로 기록됐다. caller frame은 pinned React `authAdapter.ts`의 41행과 43행이다. raw server request log는 `route=read`, `status=200`만 보여 주고 관련 `WIRE.*` code는 없다. 응답 원문/값을 artifact에 저장하지 않았으므로 실제 body가 무엇인지, 서버가 어떤 경로로 non-JSON을 반환했는지는 미확정이다. 따라서 `/read` 성공으로 간주하거나 원인을 추정해 gate를 완화하지 않는다. 앞선 수정에서 `youtube.com/iframe_api`를 inert script로 바꾼 뒤 이 artifact에서 player fixture 호출/오류에 해당하는 증거는 없다.

browser JSON은 `journey_stage=completed`, page error 0이며 `add-bookmarked.png`에는 bookmark `저장됨`이 표시된다. 그러나 strict `/read` 검증 실패로 Rust caller의 후속 bookmark DB 0→1 검증은 실행되지 않았다. Profile JSON과 앞선 MySQL profile row assertion은 통과했으며 저장 이름 `AIP 프로필 수정회원`, 장르 `animal,eating`, `write_bundle_ok=true`다. profile before screenshot은 편집 UI를 보이고 after screenshot은 로고만 남은 빈 본문이어서 after 결과 화면 근거는 아니다.

artifact `12483616670`(191,662 bytes)의 JSON 및 screenshot을 확인하고 삭제했다. artifact 재조회는 `total_count=0`이다. 로컬 재검증: `PYTHONDONTWRITEBYTECODE=1 python3 -m unittest product.tests.test_facereview_ui_browser_evidence` 17/17, Python `py_compile`, `git diff --check` 성공. run 완료 후 문서만 `[skip ci]`로 기록한다. 두 번째 수정 시도도 실패했으므로 요청된 반복 한도에 도달했다. 이번 SHA는 이미 push됐고 더 이상의 수정을 하지 않는다. Verify가 green이 아니므로 관리자 UI slice는 시작하지 않는다. 다음 재개점은 allowlist 기반 safe evidence에 한정된 원인과 실제 response body/server 처리의 미확정 범위를 바탕으로 원천 경로를 조사하는 것이다. 그 다음 push를 한다면 Verify 완료까지 기다려야 하며, 관리 UI slice의 선행 조건도 green Verify다.

---

# 이전 재개 지점 (2026-10-23 04:56 UTC, Verify 39902365530 대기 · 관리자 쓰기 adapter 조각)

Verify `39901115868`(SHA `6b1a8d4`)은 `cancelled`로 확정했다. step 10이 04:50:37 UTC에 취소되고 browser step 11–12는 skipped였다. 원인은 `verify.yml`의 `cancel-in-progress: true`이고 다음 push `93890fa`가 같은 ref의 새 run을 시작했다(추론, 취소 actor 미조회). 따라서 profile/reset/withdraw의 browser 증거와 회수할 screenshot은 없다. 현재 `39902365530`(SHA `93890fa`)은 04:55 UTC 조회 때 `in_progress`(결과 없음)였다. **이 run이 끝나기 전에는 새 SHA를 push하지 않는다.** 연속 push가 매번 Verify를 browser step 전에 취소했다. 끝난 뒤에는 `mysql_facereview_watch`의 빈 timeline 단정과 profile browser 결과를 한 번 읽고, 실패면 raw step output으로 원인을 조사한다. 2시간 polling은 하지 않는다.

로컬 Rust 전체 lib의 PG 21건 실패는 시험 DB 부재(`127.0.0.1:54321` listener 없음)였다. run 소유 `postgres:17`을 띄운 뒤 `98 passed; 0 failed`다(첫 실행의 Timeout 3건은 재실행에서 통과, 원인은 추론). 컨테이너와 이 Run의 cargo target 1.3G는 회수했다. 새 slice는 `porting/facereview/ui-overlay/src/api/adminAdapter.ts`다. 관리자 쓰기 5개를 선언된 transition에 연결하고 단위 시험 4/4를 통과했다. admin UI checkout 적용 스크립트, 읽기 8개, 승인 job adapter, admin build/browser는 미구현·미실행이다. 상세는 [제품 검증](../product/VERIFICATION.md) 끝의 「Verify 상태 정정…」이다. model.h5 추론과 visitor policy는 계속 보류한다.

---

# 이전 재개 지점 (2026-10-23 04:36 UTC, WBS 7.2.10 회원 계정 화면 overlay CI 진행 중)

프로필 쓰기 뒤 독립된 PasswordChangePage의 mail challenge→code→reset token→비밀번호 변경 API 3단계를 인증된 AIP adapter로 연결했다. `/auth/email/verification`, `/auth/password/verification`, `/auth/password`, `/auth/email/verify`를 기존 `authTransport.fetch`의 Bearer·401 refresh 경로에 연결하고 overlay 적용 스크립트가 `mypage.ts`·`mypageAdapter.ts`를 복사하도록 했다. 이어 pinned `/edit`의 기존 별도 확인 탈퇴 버튼을 생성 SDK `User.withdraw` transition에 연결해 signed-in member id만 서버에 전달한다. `useLogout`은 transition 뒤 API가 실패해도 local auth를 지우고 이동한다.

탈퇴 TDD 전 테스트는 `api.withdraw is not a function`으로 실패했다. 수정 후 전체 `node --test porting/facereview/ui-overlay/tests/*.test.ts` 26/26; `PYTHONDONTWRITEBYTECODE=1 python3 -m unittest product.tests.test_facereview_ui_browser_evidence` 15/15; `cargo fmt --all -- --check`, `git diff --check` 통과. Local pinned UI build/browser/password DB flow는 실행하지 않았다. Code SHA `6b1a8d4f70688caa9a61ed32bba6095c86fae7f9`는 main에 push했다. 최신 Verify `39901115868`은 04:36:37 UTC 조회 때 `in_progress`, container initialization step 2였다. 이전 profile SHA run `39900453676`은 04:34:56에 core step 10에서 취소되고 browser steps 11–12는 skipped; password SHA run `39900970945`는 04:36:33에 isolation step 7에서 취소되고 이후 browser steps는 skipped였다. 둘 다 브라우저 증거가 아니며 현재 Verify의 브라우저 journey도 profile 중심이라 password/withdraw 화면 journey는 별도 미실행으로 남는다.

PMS 7.2.10 description, `product/VERIFICATION.md` coverage 및 overlay README를 3개 SHA별 근거와 잔여 범위로 갱신했다. progress는 doing/25% 그대로다. 최종 docs-only push 뒤 main/origin SHA는 `6863eac5fac292c8dd664fa8e6e4793bf53f1b5e`다. 다음은 Verify `39901115868`의 완료 상태와 artifact를 한 차례 확인한다. 추가 계정/화면 여정도 필요한 CI fixture가 준비되기 전까지 실행 완료로 세지 않는다. 2시간 polling은 하지 않는다.

---

# 최신 재개 지점 (2026-10-23 04:30 UTC, WBS 7.2.10 프로필 쓰기 CI 진행 중)

회원 프로필 쓰기 조각을 코드 SHA `d29f1d372505e26fa8bec60062f0e6c036a771a3`로 main에 push했다. `porting/facereview/ui-overlay/src/api/writeAdapter.ts`에서 서명된 회원의 User patch와 본인 UserFavoriteGenre remove/create를 생성 SDK `bundle()` 하나로 묶고 `/my/edit`의 `updateProfile`에 연결했다. 과거 브라우저 코드가 정의에 없는 `user.eq`를 요청하던 기대를 제거하고, 다른 회원의 고정 genre 행을 심어 서버의 `rows read when user = actor` 경계를 확인하도록 fixture를 바꿨다. `product/VERIFICATION.md`에는 사용자 25 고유 경로와 관리자 5 화면/13 API 목록을 분리하고 API·SDK와 브라우저 근거를 나눠 기록했다. 이메일/비밀번호/탈퇴 5개 UI 경로는 backend만 있으며 overlay는 없다. 7.2.8/7.2.9 잔여 근거도 PMS description에 보완했으며 상태/진행률은 각각 doing/90%, doing/90%, 7.2.10 doing/25% 그대로다.

로컬 검사: `node --test porting/facereview/ui-overlay/tests/writeAdapter.test.ts` 5/5; `PYTHONDONTWRITEBYTECODE=1 python3 -m unittest product.tests.test_facereview_ui_browser_evidence` 15/15; profile browser script AST, `cargo fmt --all -- --check`, `git diff --check` 통과. 이 환경에서 MySQL/browser 통합은 실행하지 않았다. GitHub Verify `39900453676`는 code SHA `d29f1d372505e26fa8bec60062f0e6c036a771a3`에서 2026-10-23 04:30:40 UTC 조회 시 `in_progress`: core step 10 진행, UI prep 11·HTTP/worker/MySQL 12·artifact 13은 pending이었다. 이전 profile-read Verify `39899325826`은 SHA `dd59658274e93c65bd32c9f30ff0506c35aab982`에서 04:28:10 UTC cancelled; 단계 10은 cancelled, browser 11·12는 skipped였다. 따라서 이전 실행은 profile UI를 검증하지 않았고 신규 Verify 결과도 아직 없다. 보조 workflow는 새 SHA에서 당시 진행 중이었다.

코드 push 직전 fetch 및 소유 확인을 했고, 코드 커밋 `d29f1d372505e26fa8bec60062f0e6c036a771a3`은 main/origin에 일치한다. 현재 필요한 다음 작업은 새 Verify 상태/브라우저 결과를 한 차례 확인해 profile write slice를 닫거나 실패 근거를 조사하는 것이다. 2시간 polling은 하지 않는다. 완료 뒤 남은 UI 경로 가운데 독립적인 다음 기능을 고른다. 7.2.6의 ML 권리·visitor 정책을 임의 결정하지 않는다. 이번 slice는 Rust build를 실행하지 않아 target 증가나 cargo clean은 없었다. 기존 dirty pinned clone patch 및 사용 중인 5173은 손대지 않았다.

---

## 이전 재개 지점 기록 (2026-10-23 04:16 UTC snapshot, WBS 7.2.10 회원 프로필 브라우저 검증 대기)

프로필 slice의 CI가 진행 중이다. 검증 코드 SHA `dd59658274e93c65bd32c9f30ff0506c35aab982`를 main에 push했다. Verify run `39899325826`은 첫 조회 때 `pending`, 2026-10-23 04:16 UTC 재조회 때 `in_progress`였다. 기본 SHA `9cc43c5`의 Verify `39898679390` 및 보조 workflow 6개도 이번 slice 시작 시 이미 진행 중이었으나, 그들은 프로필 변경 전 SHA라 이번 assertion을 검증하지 않는다.

새 pinned React profile phase는 CI의 실제 AIP 서비스·전용 MySQL fixture와 생성 SDK를 사용해 로그인 후 `/my` → `/edit` 접근, 화면 닉네임 `AIP 프로필 표본회원`, 선택 장르 `eating`/`horror`(화면 라벨 먹방/공포)를 검사한다. `User` 및 `UserFavoriteGenre`의 실제 `/read` envelope·contract fingerprint·필드 선택과 회원별 filter도 확인하며 credential/body 값은 evidence에 남기지 않는다. 로컬 검사는 `PYTHONDONTWRITEBYTECODE=1 python3 -m unittest product.tests.test_facereview_ui_browser_evidence` 15개 통과, `cargo fmt --all -- --check`, 두 Python 파일 AST 구문 검사, `git diff --check` 통과다. 로컬 browser/server 통합은 실행하지 않았다. Verify 결과와 raw artifact를 받은 뒤에만 이 slice를 완료 처리한다.

정리: `cargo clean`은 35,516 files / 6.9 GiB 제거, 저장소 크기 9.1G → 3.1G, `target/` 부재를 확인했다. 더티 pinned UI clone 3개(244M/244M/246M)의 각 source diff는 outbox `aip-ui-dirty-clone-preservation/`의 별도 patch로 보존했다. `lsof +D` 세 경로 모두 열린 파일 0건이었으나, 해당 디렉터리 회수의 `rm -rf`는 실행 가드가 거부했다. 사용자가 금지한 다른 삭제 방식으로 우회하지 않았으므로 세 checkout은 아직 `/private/tmp`에 남아 있다. Codex process-list probe에서 `ps`가 `Operation not permitted`로 막혔지만 허용된 read-only `lsof`로 TCP 5173 listener PID 46256의 cwd는 `/Users/winterholic/development/projects/winterholic-ttakkari/front`임을 확인했다. 타 프로젝트 프로세스라 건드리지 않았다. 이 slice에서 local server/browser/fixture를 띄우지 않았다.

CI 재개 지점: Verify `39899325826`이 `in_progress`이므로 현재 Run에서 더 기다리지 않는다. 재개 시 상태와 job 진행 증거를 한 번 확인한다. 완료면 profile-browser-result JSON·screenshot·단계 로그를 읽고 assertion 결과를 기록한다. 실패면 safe artifact와 raw step output으로 원본 화면/definition/SDK/adapter/render 경로를 대조한 뒤 좁게 수정한다. 계속 실행 중이면 run ID와 다음 확인점을 유지하고 2시간 폴링하지 않는다. 다음 미완료 WBS 범위는 이 profile slice를 닫은 뒤에 정한다.

## 이전 재개 지점 (2026-10-23, WBS 7.2.10 브라우저 오류 조사)

오류 조사 결과를 [제품 검증 기록](../product/VERIFICATION.md)의 「후속: `/read Comment` envelope와 console 오류 진단」 및 Verify `39896024733` 기록에 정리했다. `WIRE.UNKNOWN_KEY`는 pinned FaceReview UI watch의 `getVideoComments`가 Comment 계약에 없는 root `traverse`, sort `direction`, 공개되지 않은 User `profile_image_id`를 요청한 문제였다. query builder를 nested `select` (`user.id`, `user.name`)와 `dir`로 고친 뒤 실제 React `/read Comment` 두 건은 HTTP 200 `ok=true`, rejected query key 0으로 확인됐다. Contract fingerprint mismatch나 auth refresh 오류는 아니며, 영향 범위는 watch 댓글 조회다. fixture에 Comment 행이 없어 누락 댓글의 실제 렌더 영향은 입증하지 않았다.

같은 실행의 console 오류는 `youtube` host class의 외부 `script` 요청 `net::ERR_FAILED`와 일치하며 page error는 0이다. 외부 미디어가 차단된 browser 환경에서 영상 재생이 제한되고, Verify `39896024733` screenshot에서는 오류 modal이 접근성 role locator를 가린 상태가 관측됐다. modal이 해당 assertion 실패에 기여했다는 것은 그 실행의 UI/network 증거에 한정한다. 앞선 Verify `39885042974`에서는 React bookmark on/reload·off/reload 및 SQL assertion이 통과했다. console 분류는 AIP Comment 오류와 별개다.

이 오류 조사와 `/read` query 수정은 2026-10-23 기록 당시 main `da2fe7c112c7f6c996ef349a6f005cb98a01731e`를 기준으로 했다. 이번 STATUS 보완은 docs-only다. 이 조사 범위는 닫혔다. 다음 작은 검증 조각은 전용 회원으로 pinned UI의 프로필 화면을 거쳐 `getUserName`이 읽는 `User`와 `UserFavoriteGenre` 필드를 원본 화면과 대조하는 것이다. API/프로필 전이의 실DB 시험은 이미 있지만 화면 경로는 minimum browser journey에 포함되지 않았다. 이 경로를 추가 검증해도 나머지 비밀번호·가입·방문자 동작, 25개 전체 경로, prerender, ML/model 권리, 방문자 정책 완료를 뜻하지 않는다. 과거 시점의 재개 기록은 아래에 보존한다.

## 후속 결과 (2026-10-23, Verify `39885042974`, `b504f62161fb9e3065ac9da9972ebe9928e0c315`)

아래 이전 재개 기록 뒤에 root cause 계측·최소 테스트 수정 후속 결과를 추가한다. Verify와 같은 SHA의 보조 workflow 6종은 모두 success다. 실 React는 pinned UI `2e7c021fa5abebd72278d841b8066a33d45cfbb3`로 로그인했고, 회원 목록에서 fixture 카드 2개를 렌더했으며 고정 영상 ID `aaaaaaaa-0000-4000-8000-000000000001` 카드를 선택했다. 기존 빈 화면의 root cause는 pinned UI `useAvailableVideos`의 썸네일 `Image()` 로드 필터와 CI의 외부 YouTube 이미지 차단 조합이었다. API 응답/adapter에는 fixture가 있었고 JS page error는 0이었다. 제품 fallback/selector 완화 없이, 브라우저 하네스가 그 fixture 썸네일 한 URL만 loopback HTTPS fixture 이미지로 연결하도록 했다(외부 미디어는 계속 차단, AIP API는 실서버).

add/reload에서 UI bookmark 상태가 `aria-pressed=true`, off/reload에서 `false`; 실제 `/apply` 응답은 각 phase HTTP 200 `ok=true`였다. `mysql_facereview_auth.rs`는 브라우저 앞 `video_bookmark` count 0, add 뒤 1, remove 뒤 0을 정확히 `assert_eq!`하며, 실행 코드는 두 React phase와 DB 조회를 순서대로 연결한다. 같은 SHA 전체 CI가 success라 해당 assertion 통과는 확인되지만, 업로드된 safe browser artifact에는 DB count 자체가 들어 있지 않다(그 실행값은 CI 통과+assert 코드에 근거한 확인이며 별도 DB JSON 증거는 아님). 스크린샷/JSON은 이번 Run outbox `ci-39885042974/{contract-evidence.json,add/,remove/}`에 있다.

계약 definition SHA `b03f…7db5f`, server/generated/UI/browser header fingerprint `cab5…6ce2` 일치. 목록별 response와 adapter 길이는 `homeVideoAll=1`, `homeCategory=2`, `homePersonalized=1`; 렌더 카드 2. 추가 관찰: artifact에는 `/read` 일부 `WIRE.UNKNOWN_KEY`(HTTP 200 envelope)와 console error 2(add), 1(remove)가 남지만 browser phase는 통과했고 page error는 0이다. `/read` sanitizer가 resource를 `unrecognized`로 남겨 구체적 요청을 알 수 없으므로 이 부수 오류의 원인은 미확정이며 bookmark/카드 root cause와 연관짓지 않는다. WBS 7.2.10은 이 범위의 React journey를 통과했으나 전체 7.2.10/ML 완료로 확장하지 않는다. 원본 pinned UI와 이 문서의 과거 기록은 보존했다.

## 현재 재개 지점 (2026-10-23, 7.2.10 React browser member journey)

실제 React 브라우저 여정은 Verify `39872659856`에서 로그인·`/session` 뒤 `/read` 2건이 HTTP 200 `WIRE.CONTRACT_MISMATCH`로 실패했다. `--read-only` 제거는 실패한 수정이므로 반복하지 않는다. producer 대조에서 서버 정의는 `facereview.aip`와 `facereview-auth/operations.aip`를 합친 파일이고, 기존 overlay는 relational fixture만 생성 입력으로 쓰는 차이를 확인했다. 로컬 CLI 비교에서 두 입력은 각각 SHA256 `7872ee9eafc7f2e88b0e0dbc6861917c797b351a4fbb3f9f6a4bbc34074a4900` / fingerprint `2c24f6f10a0cd60820c9911046f031e23da94df2c90ce8a7361df30d376aab56`, 합성 입력은 SHA256 `b03f938989a5407ac39d113dbbcd771e87c88f90884991d55530fadc14f7db5f` / fingerprint `cab5e5ff77cd04117f176035d913843907904d9ced810d1c53764b76c5536ce2`였다.

수정 SHA `e35113a73195a0ff1c7df43b41810eed2b8c1c97`의 Verify `39875726918`에서 합성 definition SHA는 UI·서버 입력 모두 `b03f…7db5f`, generated/UI/server listening fingerprint는 모두 `cab5…6ce2`였고, React의 실제 두 `/read`는 HTTP 200 `ok=true`, `contract_matches_runtime=true`로 돌아왔다. 따라서 이번 실행에서 contract mismatch는 재현되지 않아 producer 입력 정합 수정이 해결했다. 다만 브라우저는 로그인 뒤 회원 목록 영상을 화면에서 찾는 단계에서 실패했고 `add` screenshot은 홈 화면에 카드가 없는 상태다. artifact에는 `/apply` 요청이 없으므로 bookmark on/off와 DB 1행→0행 단정은 실행되지 않았다. UI가 받은 list 응답의 safe shape와 실패한 Playwright assertion은 artifact에 없어 그 다음 원인은 미확정이다. raw workflow log는 읽지 않았다.

전체 Verify 단계 10과 pinned UI 준비는 성공, 단계 12 실제 HTTP/worker/MySQL/browser는 실패; 같은 SHA의 Linux worker jobs, Linux cgroup v2, Database contracts, Object storage, App packages, External brokers 6개 보조 workflow는 success다. 원본 pinned UI `2e7c021fa5abebd72278d841b8066a33d45cfbb3`의 dirty checkout은 수정하지 않았다. 상세 JSON·screenshot과 경계는 [제품 검증](../product/VERIFICATION.md)의 「2026-10-23 WBS 7.2.10 React browser member journey」와 이번 Run outbox를 본다. WBS 7.2.10 journey는 미완료다.

### 오류 원인 고정 및 수정 후보

진단 전용 SHA `4c24eb6a2fb206d5598cf2841175bcdd8dc8490e`의 Verify `39888567142`는 strict envelope 검사에서 의도적으로 실패했다. safe artifact의 두 `/read` `Comment` 요청은 HTTP 200이지만 `ok=false`, `WIRE.UNKNOWN_KEY`였고 합성 definition·runtime·UI·browser fingerprint `cab5…6ce2`는 일치했다. 요청은 SDK `{query: ...}` envelope이며 유일하게 식별된 거절 query key는 `traverse`다. pinned UI와 overlay 구현 대조에서 호출은 watch 화면 `getVideoComments`; 공개 쿼리 형식은 `select` 안 nested `{user:{select:["id","name"]}}`다. 현재 CI 후보는 root traverse 제거, 정렬 키를 `direction`에서 SDK 계약의 `dir`로 변경, Comment 관계에 공개된 `id,name`만 선택한다. Comment 응답 실패는 watch 댓글 조회에 국한되고 로그인·bookmark read/apply 성공과 독립이다. fixture에는 Comment 행이 없으므로 실제 누락 댓글 데이터의 시각적 영향은 입증되지 않았다.

같은 artifact의 console error는 `error/network_resource_error`, source `unmapped` 1건이고 page error는 0이다. 원인은 이 run만으로 확정되지 않았다. 후속 browser instrument는 URL·token 없이 허용된 외부 미디어 host class, resource type, Chromium error code만 남긴다. 최종 수정 SHA의 관련 CI 확인을 기다린다. 진단 artifact: 이번 Run outbox `ci-39888567142/facereview-ui-browser-39888567142-1/add/`.

**후속 수정 SHA `4f5e6b039f503fecf93a6af364d13b4a7ada49b6`, Verify `39891300600`.** 실제 React `Comment` `/read`는 HTTP 200 `ok=true`, rejected query key 0으로 바뀌었고 nested User selection에 허용된 `id,name`이 갔다. 로그인·session, bookmark `/apply` 2건, `VideoBookmark` 읽기가 모두 성공했다. safe screenshot에는 main 화면의 fixture 카드와 watch 화면 댓글 2개가 보인다. query defect 수정은 runtime에서 통과했으나 browser step12는 add phase 종료 전에 failure로 끝나고 DB on/off 최종 journey는 아직 미확정이다. 실패 artifact에는 그때의 단계/예외 종류가 없었다.

**진단 artifact 결과 SHA `eddb7ed51c279a2c8da6cd4e5b24d941d853766b`, Verify `39893336376`.** 이 run은 `journey_stage=bookmark_reload`, `failure_type=AssertionError`로 끝났다. `즐겨찾기 추가`와 `즐겨찾기 해제` 역할 버튼은 둘 다 0개였고 page error는 0이다. AIP `/read Comment` 두 번 모두 `ok=true`, nested allowlisted user fields `id,name`; add `/apply` 두 번도 `ok=true`였다. 중단점은 reload 전/후를 구분하지 못하고 route/button state가 없어 아직 root cause 확정이 아니다. assertion을 약하게 하거나 browser route를 건너뛰지 않고 다음 safe artifact에 `bookmark_reloaded`와 route class/aria controls를 보강한다.

**최종 안전계측 SHA `8d1a25758873d5e631a039d5d092e249df3bc9f0`, Verify `39896024733`.** 단계 10/11 success, 단계 12 failure, artifact upload success. 실제 `/read Comment`는 2건 HTTP 200 `ok=true`, nested relation `user:{id,name}`; 로그인/session·list extension·bookmark에 필요한 API 응답도 성공했다. browser는 fixture card를 선택해 watch route에서 bookmark on, reload 후 `journey_stage=bookmark_reloaded`까지 도달했다. 실패 화면에서 watch fixture route와 video content가 있고 `[aria-pressed]` DOM에는 `즐겨찾기 해제=true`가 존재하지만 Playwright accessibility role locator로는 보이지 않았다. screenshot에는 외부 video provider 요청 차단 뒤 표시된 “영상을 불러올 수 없어요” modal이 화면을 덮는다. 이 조합은 외부 `youtube` script `net::ERR_FAILED` 두 건과 일치한다. 원인은 external media 차단으로 인한 UI modal (screenshot + failed resource + hidden accessible button으로부터 추론)이며 React pageerror는 0이다.

이 환경에선 reload 뒤 사용자에게 북마크 버튼이 접근 가능하지 않아 역할 기반 검증을 완료할 수 없고, 확인 CTA는 원본 pinned UI에서 `/`로 이동한다. 오류를 닫기 위해 UI를 건너뛰거나 숨은 버튼에 직접 조작할 수 없다. remove/reload와 SQL count 0→1→0은 이 run에서 검증되지 않았다. 따라서 bookmark UI/DB journey는 미완료로 남고, no-external-video 조건하에 다음 가능한 정상 React 경로를 새 root-cause evidence 없이 추측해 통과시키지 않는다. Screenshot/JSON: 이번 Run outbox `ci-39896024733/facereview-ui-browser-39896024733-1/add/`.

기존 console `network_resource_error` 2건의 원인은 이제 관측됐다: 두 request-failed event 모두 외부 `youtube` host class의 `script`, `net::ERR_FAILED`이며 harness의 의도적 외부 YouTube 차단과 일치한다. 이는 재생 불가 제한을 알리는 console 출력이고 React pageerror는 0이다. 정확한 클릭/재로드 중단 단계를 safe artifact에 추가하는 계측 변경을 적용해 최종 여정을 한 번 더 검증한다. 증거: 이번 Run outbox `ci-39891300600/facereview-ui-browser-39891300600-1/add/`.

### 이전 재개 지점 (2026-10-23, 인증 회원 SDK/API 최소 journey)

WBS 7.2.10의 모델·방문자 정책과 독립된 인증 회원 journey 조각은 코드 SHA `1033269ec1389ddd7b750b6ad9f5e53bd100a53b`에서 Linux CI Verify `39851918809`와 같은 SHA의 5개 보조 workflow까지 모두 success다. 실제 생성 TypeScript contract와 설치한 `@aip/sdk`를 실제 AIP 서비스에 연결해 전용 fixture 회원 login → refresh 1회 → UI 읽기 어댑터 영상 목록 → 북마크 생성/해제 → 댓글 생성/수정/삭제 → logout 뒤 stale bearer의 `WIRE.UNAUTHENTICATED`를 확인했다. 이 journey는 25개 UI API 경로 중 인증·회원 읽기·쓰기의 최소 경로를 덮는다. CI 상세는 [제품 검증](../product/VERIFICATION.md)의 「2026-10-23 WBS 7.2.10 인증 회원 SDK/API journey」에 기록했다. 화면에서 실제 로그인한 브라우저 journey, 전체 25개 경로 동등성, 전체 prerender는 미검증이다. ML inference/model 권리와 방문자 접근 정책은 이번 범위에서 다루지 않았으며 7.2.10 전체 완료로 세지 않는다. 다음은 나머지 인증 회원 화면/API 경로의 범위를 기존 결정과 대조해 한 조각씩 좁힌다.

## 이전 완료 snapshot (2026-10-23, 7.2.6/7.2.10 internal extension 조각 3)

완료 (2026-10-23): WBS 7.2.6/7.2.10 조각 3의 최소 `internal` write extension 표식을 문법·H-map·정식 facts·진단·runtime public HTTP 경계·TS/Python SDK 계약에 연결했다. `User.startWatching`/`User.recordFrame`만 server-only이며 `VideoViewLog.create` 공개 allow는 유지한다. DECISIONS와 VERIFICATION 갱신, 커밋 `9adc187` + golden 문서 보완 `71b0bbd`; 당시 main/origin 동일 SHA `71b0bbdb77fa6db2c173691f953a0f1797be66ea`, clean. Linux CI Verify `39824863529`는 해당 SHA에서 전체 검증과 live HTTP→Node/Python→MySQL E2E 모두 success. 실제 `mysql_facereview_watch` 1 pass로 trusted `startWatching` socket owner/write 경로 및 public `/apply` 거부를 MySQL·MongoDB·Redis와 함께 확인했고, generic HTTP 시험은 `/apply`·`/extension` 거부와 caller flag 위조 거부를 확인했다. 로컬 Mac Codex nested worker sandbox에서 Python probe가 `sandbox-exec: sandbox_apply: Operation not permitted`로 거부됐으나 Linux CI에서 통합 경로를 검증했다. 상세는 [제품 검증](../product/VERIFICATION.md)의 「2026-10-23 WBS 7.2.6/7.2.10 조각 3」이다. `recordFrame`의 향후 trusted caller, ML inference/model permission, 방문자 접근 정책과 prerender는 당시 미검증/미결정이었다.

이전 정지 snapshot (2026-10-23 01:20 KST): 당시 세션 257e4936은 WBS 7.2.6 조각 2·3, ML 게이트 조사, `/events/socket` 확장 협상 수정과 7.2.10 조각 1 읽기 어댑터를 마쳤다. 당시는 인증·쓰기 어댑터 조각 2가 최신 상태였으며, 이후 조각 3 경계 구현과 이번 인증 회원 API journey가 추가됐다. 당시 남은 모델 및 방문자 정책은 위 현재 재개 지점과 [제품 검증](../product/VERIFICATION.md)의 각 WBS 항목에서 범위를 확인한다.

## 이전 재개 지점 (2026-10-23, 7.2.10 조각 2 뒤)

조각 1 읽기 어댑터 뒤, 이번 Run에서 인증·쓰기 어댑터와 pinned checkout용 overlay script를 추가했다. AIP `/auth/*` one-time refresh·동시 401 coalescing·실패 콜백, 댓글·좋아요·북마크·요청 추천의 기존 명시 create/remove/writeExtension 계약을 다룬다. 테스트 20개, 생성 SDK package, UI TypeScript 및 Vite build가 통과했고, configured Ttakkari browser에서 FaceReview SPA 셸을 봤다(서비스 데이터 응답은 연결하지 않음). MySQL watch live test는 run-owned MySQL/Mongo/Redis를 준비했지만 서비스 시작 시 `SERVER_CONFIG` (`worker runtime check` 등 준비 단계 중 하나, 하위 원인은 미확인)로 실패해 공개 HTTP 확장 실행/거절 판정은 미검증이다. 공개 경로와 계약의 판정·향후 최소 내부 전용 표식 선택지는 [DECISIONS](../docs/DECISIONS.md) 및 [제품 검증](../product/VERIFICATION.md)의 조각 2를 본다. 모델 출처와 방문자 접근 정책은 계속 미결정이다. 다음: 같은 SHA CI와 main push를 확인한 뒤 7.2.10의 나머지 화면/API 범위로 이동한다.

## 이전 재개 지점 (2026-10-23, 7.2.6 조각 3 프레임 타임라인 저장 · ML 게이트 조사 · `/events/socket` 확장 협상 수정 뒤)

**조각 3(ML 미연결).** 쓰기 확장 `User.recordFrame { id, centi }`이 프레임 점수를 Mongo 세션 문서의 두 타임라인 맵에 멱등하게 쓴다(결정 D-P-WATCH-FRAMES). 점수는 입력이 아니라 서버만 쓰는 Redis 키에서 한 번 꺼내 쓰므로 회원이 위조할 수 없다. 그 키를 쓰는 서버 코드는 아직 없어서 소켓은 이 확장을 부르지 않고 프레임 ack는 `WIRE.NOT_AVAILABLE`이다. 시험은 고정 숫자를 직접 그 키에 넣은 것이며 실추론이 아니다. 원본의 `$inc` 카운터(`frame_count`·`emotion_sum`)와 `emotion_percentages` 재계산은 멱등하지 않아서 쓰지 않았고 세션 확정·집계 조각이 타임라인에서 계산해야 한다. 근거는 [제품 검증](../product/VERIFICATION.md)의 「7.2.6 조각 3」이다.

**CI.** 조각 3 커밋 `7c3ec334bd8585612de4b93e00dce4787d0bae7b`는 동일 SHA 8종 workflow가 모두 success다(Verify 39802081466, 22분). 조각 2 커밋 `7f0f702`는 7종 success이고 Verify는 30분 `timeout-minutes`에 걸려 cancelled였다(시험 단계는 모두 success, 상한을 45분으로 올림). 실제 `socket.io-client` 종단·`recordFrame` 단정·`permessage-deflate` 시험이 Linux runner에서 `ok`임을 로그로 확인했다.

**ML 게이트 조사 결과(사용자 결정 필요).** `model.h5`는 git에 추적된 적이 없고(`rev-list --objects --all`에 `.h5` 0개) 저장소에 학습 데이터·학습 코드·LICENSE·출처 문서가 없다. 구조는 Keras 2.4.0으로 저장한 ResNet 계열 177층, 입력 96×96×1, 최적화기 가중치 포함이라 팀이 직접 학습한 모델로 보이지만 추정이다. 선택지는 (A) 원본 모델 사용(사용자가 소유·사용 권한을 확인), (B) 허가된 대체 모델(수치 대조 불가), (C) 보류(현재 상태). 재현 환경은 Python 3.10, `tensorflow==2.15.0`, `opencv-python==4.9.0.80`, `cvlib==0.2.7`, `Pillow==10.2.0`, `numpy==1.26.3`이고 Keras 2.4.0 H5의 TF 2.15 로드 호환성은 미검증이다. 사용 허가된 입력 영상·프레임은 저장소에 없다. 상세는 제품 검증의 「7.2.6 ML 게이트 조사」다.

**`/events/socket`.** Chrome이 보내는 `permessage-deflate` 제안을 서버가 `WIRE.BAD_REQUEST`로 거절하던 것을 시험으로 재현하고(먼저 FAILED) 확장 거절 조건을 지웠다(RFC 6455 9.1절, 서버는 제안을 받지 않아도 된다). 실제 브라우저는 이 세션에 도구가 없어 실행하지 않았다. 다음은 사용자의 모델 결정을 기다리는 ML 조각을 제외하면 모델 없이 할 수 있는 일(점수를 쓰는 서버 코드의 자리, 평균 감정의 출처 결정, 세션 확정·집계 설계)과 7.2.10이다.

## 2026-10-23 재개: WBS 7.2.6 조각 4 (Mongo 빈 timeline 초기화)

확인한 기존 Verify `39901115868`은 SHA `6b1a8d4f70688caa9a61ed32bba6095c86fae7f9`, 2026-10-23 04:47 UTC 조회 시 `in_progress`였다(step 10 진행, profile browser·HTTP/worker/MySQL·artifact 단계 pending). run id와 상태를 제품 검증 기록에 남긴다. 완료되지 않아 profile/reset/withdraw 결과를 판정하거나 artifact를 회수하지 않았다.

`watch_frame`은 기존 `startWatching` 호출을 유지해 `VideoViewLog`·Redis 시청 세션 생성의 멱등성을 보존하고, 이어 서버 전용 `User.recordFrame`을 호출하도록 연결했다. staged 모델 점수가 없을 때도 Mongo `youtube_watching_data`에 `video_view_log_id`당 문서 하나를 upsert한다. `emotion_score_timeline`과 `most_emotion_timeline`은 빈 객체이며 점수·추론은 만들지 않는다. 점수가 staged 된 경우의 기존 `$set` 쓰기는 유지한다. 소켓 응답은 `WIRE.NOT_AVAILABLE`이고 모델 연결 부재를 설명한다. 단위 시험은 first-frame/reconnect의 view log 1행 및 중복 timeline 문서 1개를 확인하도록 확장했다.

검증: `PYTHONDONTWRITEBYTECODE=1 python3 -m unittest test_slice14l_frame` 6/6 통과; `cargo test --manifest-path spikes/spike-v6-transport/Cargo.toml --lib socketio::tests:: --locked` 25/25 통과; `cargo fmt --all` 및 뒤이은 `cargo fmt --all -- --check` 통과. 전체 `cargo test ... --lib`에서는 transport 관련 77개가 통과했으나 PG 연결이 필요한 다른 21개가 이 Run의 임시 DSN(`127.0.0.1:54321`)에서 `Connection refused`로 실패했다. 새 실제 MySQL/Mongo Socket.IO 회귀 단정은 로컬에서 실행하지 않았고 새 SHA의 Verify가 확인할 부분이다.

다음 재개점: 새 SHA의 Verify에서 `mysql_facereview_watch`가 첫 프레임·중복 프레임·재접속 후 view log 1개와 owner-scoped Mongo 빈 timeline 1개를 확인하는지 본다. 이어 기존 `39901115868`의 완료/취소 상태와 산출물 삭제 여부를 갱신한다. 모델 권한·재현 환경, 실제 브라우저 미디어 입력, visitor policy, ML 추론은 여전히 미결이며 여기서 확정하지 않는다.

## 이전 재개 지점 (2026-10-23, 7.2.6 조각 2 첫 프레임 세션 구현 뒤)

7.2.6 조각 2를 구현했다(결정 D-P-WATCH-SESSION, 검증은 [제품 검증](../product/VERIFICATION.md)의 「7.2.6 조각 2」). 유효한 첫 `watch_frame`이 쓰기 확장 `User.startWatching`으로 `VideoViewLog` 한 행과 Redis 시청 세션을 만들고, 사용자는 프레임마다 다시 검증한 토큰에서만 얻는다. 같은 연결의 중복 프레임·재접속·다른 회원의 같은 id·폐기된 principal·토큰 만료를 실제 MySQL 서비스와 Redis로 확인했다. 쓰기 확장 호출 코드는 `run_relational_write_extension`으로 모아 `/extension` 쓰기와 공유한다. **프레임 ack는 여전히 `WIRE.NOT_AVAILABLE`이고 분석·Mongo timeline이 없어 실제 영상·웹캠·ML 흐름은 증명되지 않았다.** 다음은 조각 3(Mongo timeline 저장: 저장 경로·순서·중복 처리부터, 감정값은 ML 자리로 비워 둠), ML 게이트 조사(`model.h5` 출처·허가 근거, TF/OpenCV 요건; 구현 없음), `/events/socket`의 `permessage-deflate` 거절이 실제 브라우저를 막는지 최소 재현이다. 7.2.10은 그 뒤다. 세션 257e4936이 이 일을 하고 있으며 e29ddf1e 같은 다른 실행은 7.2.6을 겹쳐 시작하지 않는다.

## 이전 재개 지점 (2026-10-22, 7.2.6 조각 1 Socket.IO 전송 경계 구현 뒤)

7.2.6의 첫 조각으로 Socket.IO 호환 전송 경계(Engine.IO v4 WebSocket 전용, CONNECT `auth.token`, 패킷 상한, 서버 ping/pong, disconnect, `watch_frame` 필드 검증)를 `spikes/spike-v6-transport/src/socketio.rs`에 구현했다(결정 D-P-WATCH-SOCKET, 검증은 [제품 검증](../product/VERIFICATION.md)의 같은 날짜 절). 단위 24개, 실제 서버 통합 4개(PostgreSQL 17), 실제 `socket.io-client` 4.8.3 종단 1개가 로컬에서 통과했고 spike-v6 전체 시험은 EXIT=0이다. **유효한 프레임도 `WIRE.NOT_AVAILABLE`로 답한다. 분석·세션·view log·Mongo timeline이 없으므로 7.2.6의 실제 영상·웹캠·ML 흐름은 하나도 증명되지 않았다.** 다음 조각: 첫 프레임의 세션 초기화와 `VideoViewLog` 멱등 생성(쓰기 경로·principal 유도 `user_id`·프레임별 재검사), 그 뒤 모델 출처와 재현 가능한 ML 런타임(게이트: 모델 출처 권한과 TensorFlow/OpenCV 환경 미확인), Mongo timeline 저장, 원본 backend를 끈 실제 브라우저 trace. 이 서버의 MySQL 서비스 위 종단 실행과 실제 브라우저 접속은 미검증이다.

7.2.8의 MySQL 종단 미검증은 로컬에서 갱신했다. `mysql_facereview_trending_job`을 이 Run의 격리 MySQL 8.4·MongoDB 7·Redis·PostgreSQL 컨테이너에서 실행해 `1 passed`(직접·atomic·중첩 우회 5가지의 `WIRE.NOT_EXPOSED`와 `myEmotionSummary` 정상 갱신 포함)였다. 앞 실행의 `SERVER_CONFIG` / `io other error`는 재현하지 못했고, Redis 미설정 가설은 변수만 뺀 통제 실행(같은 `1 passed`)으로 기각했다. 앞 실패의 원인은 확정하지 못했다(환경이 회수되어 검증 불가). Linux CI: 조각 커밋 `58e1fdb`의 8개 workflow가 모두 success이고 Verify(run 39788184086)의 로그에서 이 시험이 Linux runner에서 `1 passed`(46.35초)다. 같은 로그에서 새 `socket.io-client` 종단 시험도 `ok`다. 앞선 `40b4539`·`ab2f79f`의 Verify는 뒤 push로 취소돼 결과가 없다.

7.2.10은 구현 없이 사전 조사만 했다([제품 검증](../product/VERIFICATION.md)의 「7.2.10 사전 조사」): 고정 UI의 서버 호출은 `src/api` 여섯 파일 30곳(고유 경로 25개)과 소켓 1곳에 모여 있고, 전부 AIP 쪽 대응이 있지만 화면 응답 모양 일치와 토글(like·bookmark) 변환은 확인 전이다. 다음은 UI 작업 위치 결정과 SDK 바인딩 산출 방법, 그리고 어댑터 첫 조각이다. 최종 gate(원본 backend를 끈 브라우저 E2E)는 7.2.6의 실제 ML 흐름 뒤다.

## 이전 재개 지점 (2026-10-22, applyDelta 권한 slice push 완료 · 7.2.6 transport 시작 전)

7.2.8 `UserEmotionSummary.applyDelta` 서버 전용 전이 slice는 `40b4539bd2a6495de5a2da6c00d0c0c5559aa2fa`로 main push했다. 직접 공개 apply, 직접 atomic, 허용된 변경 뒤 중첩 atomic은 `WIRE.NOT_EXPOSED`로 거절하는 계약 검증 2개, 실제 PostgreSQL write 통합 1개, worker 중복 첫 행 생성의 `WIRE.ALREADY_EXISTS`→`WIRE.CONFLICT` scripted 실행 1개가 통과했다. MySQL+Mongo 시험은 서비스 시작의 `SERVER_CONFIG` / 일반 io 오류에서 끝나 MySQL HTTP 경로와 실제 MySQL extension 갱신은 미검증이다. 오류 출력으로 worker runtime/readiness/listener 중 어느 하위 단계인지 특정할 수 없고 배포 backend 상태를 뜻하지 않는다. 동일 SHA CI는 Linux cgroup v2 success, 나머지 7 workflow가 마지막 조회 시 in_progress였다. 이전 marker 커밋 `fbe56656539de00d51ca786804c77b6d48b67433`는 Linux cgroup v2, Object storage, App packages, External brokers success; Verify cancelled (뒤 커밋 push로 취소)였다.

이 Run이 띄운 MySQL/MariaDB/Mongo/PostgreSQL fixture는 종료했고 저장소 `target` 8.9 GiB는 `cargo clean`으로 제거했다. 현재 main은 origin/main과 일치하고 working tree clean이다. **다음 재개 지점:** `docs/DECISIONS.md`의 D-P-WATCH-TRANSPORT와 제품 검증 7.2.6 근거를 다시 읽고, Socket.IO-compatible protocol handshake/auth/payload bounds/disconnect 의미를 구현하기 전 테스트로 고정한 뒤 transport dependency/runtime entry point를 최소 slice로 구현한다. 현재 runtime에는 Axum WebSocket `/aip/subscribe`는 있지만 Socket.IO event/ack adapter는 검색상 없다. 실제 미디어·model provenance와 재현 가능한 TensorFlow/OpenCV 환경이 확인되기 전에는 frame inference나 browser ML E2E 완료로 세지 않는다.

## 재개 지점 (2026-10-22, 7.2.8 감정합계 권한 보완과 7.2.6 baseline 조사)

7.2.8의 `UserEmotionSummary.applyDelta`를 `internal` 노출로 제한하는 변경과 회귀시험을 진행 중이다. 일반 공개 apply/atomic은 `WIRE.NOT_EXPOSED`, public compose는 `DEF.UNSUPPORTED`이며, write extension/job은 기존 `access` grant를 거쳐 동일 transaction/owner/expect 검사를 받는다(D-P-INTERNAL-APPLY). `spike-v1-fixture` 2개, PostgreSQL `spike-v3-write` 1개, scripted worker의 `ALREADY_EXISTS`→`CONFLICT` 1개 시험은 방금 각각 통과했다. MySQL+Mongo `mysql_facereview_trending_job`은 실제 shell test fixture에서 `aip service serve` 시작이 `SERVER_CONFIG`의 일반 io 오류로 실패해 MySQL HTTP 및 실 worker E2E는 미검증이다. 오류 메시지만으로 worker/runtime/readiness/listener 중 하위 원인은 특정할 수 없으며 배포 backend로 일반화하지 않는다. 최신 근거와 명령은 [제품 검증 7.2.8 후속](../product/VERIFICATION.md)에 적었다.

7.2.6 baseline은 backend pinned SHA `7948b7e721837fdb73c427cb54e12cad4ec2d5af`, UI `2e7c021fa5abebd72278d841b8066a33d45cfbb3`다. 두 원본 working tree가 dirty하므로 pinned `HEAD`를 기준으로 읽었다. `.h5` 모델은 378 MB·owner-only 0700·ignored이고 inventory에 없으며 permission provenance 미확인. pinned source가 요구하는 TensorFlow/OpenCV/NumPy는 baseline Python lock과 기존 venv에 없어 real inference 재현은 아직 안 됐다. 최소 transport 후보·5단계 계획 및 SSE/events_days 분리 추적은 D-P-WATCH-TRANSPORT와 제품 검증 7.2.6 조사 절에 기록했다. WBS 7.2.6은 doing 5%이며 다음은 고정 미디어/모델 provenance, reproducible ML runtime, 그리고 handshake/auth/payload/disconnect 첫 slice의 API 경계를 구현 전 테스트로 고정하는 것이다. 가짜 감정값, mock, 빈 입력 시험은 ML E2E로 세지 않는다.

남은 조치: 변경·회귀·문서 조각을 정리해 커밋·push하고 동일 SHA CI를 조회한다. 다른 작업자의 변경물은 보존한다. 사용 중인 임시 DB/Mongo/Postgres fixture와 빌드 target은 각 gate 완료 후 소유 경로만 회수한다.

# 진행 현황과 쌓인 질문

## 재개 지점 (2026-10-22, 7.2.9 구현 완료 뒤 기록)

7.2.9의 남은 둘을 옮겼다. 시스템 상태는 읽기 확장 `User.systemStatus`(결정 D-P-SYSSTATUS, 원본과 10개 사례, 서버 지표는 파생 컬렉션 `server_status`로 Linux 전용이며 식은 psutil 값과 같음을 Linux 컨테이너로 확인), 시청 더미 데이터는 durable job `generateDummyData`(결정 D-P-DUMMY, 같은 시드의 원본과 20개 사례)다. 더미 데이터를 만들다가 기존 결함을 찾아 고쳤다: durable job이 Mongo의 개인 문서에 소유자를 UUID가 아닌 정수 핸들로 적었다(`mysql_jobs.rs`). 따라서 7.2.9의 잔여 4개(요청 생성, dashboard, 시스템 상태, 더미 데이터)는 모두 구현 조각을 마쳤고 각 절의 한계와 미검증 항목은 남아 있다. 남은 것: 실제 메일 provider live smoke 미실행, 더미 데이터의 비숫자 세션 경로 미실행, 생성 SDK의 단독 집계 바인딩과 화면 연결은 7.2.10, 대시보드 호출 횟수·Redis 캐시 없음 등 각 절의 알려진 한계. 이 조각은 9accfd7이고 동일 SHA CI 8종(Verify 39761260361 포함)이 모두 success다. 직전 조각 CI: 49aea49 7종(Verify 39756710567 포함) completed/success(Deployment checks는 경로 조건으로 실행되지 않음).

## 이전 재개 지점 (2026-10-22, 7.2.9 관리자 대시보드 뒤 기록)

7.2.9의 관리자 대시보드를 새 언어 기능 없이 옮겼다(결정 D-P-DASHBOARD). 관리자 전용 단독 집계 4개(`User.signupCount`, `User.weeklyActive`, `VideoRequest.pendingCount`, `Video.categoryViews`)와 읽기 확장 2개(`VideoRequest.pipeline`, `Video.contentHealth`)이고, 플랫폼에는 Mongo 파생 컬렉션 `video_distribution_stats` 하나를 더했다. 원본 `AdminService`와 25개 사례가 같았다(`reference/slice14h_oracle.py`, 기간 6개의 모든 버킷 포함). CI 재생(`mysql_facereview_trending_job`)은 시각이 매번 달라 원본 SQL과 비교한다. 한계: 호출 횟수가 많고(가입 추이 3y는 37번) 기간·기준 시각을 호출자가 정하며 Redis 캐시가 없다. 확장에 호출자 입력을 여는 일, distinct·평균 release, 시각 버킷 그룹 집계는 새 언어 기능이라 결정 없이 만들지 않았다. 남은 7.2.9: 시스템 상태(`get_system_status`, psutil 호스트 지표와 Redis 요청 지표)와 더미 데이터(`generate_dummy_data`, 관리자가 한 회원의 세션 문서를 Mongo에 만드는 기능). 이 조각의 커밋과 CI 결과는 아래 줄에 적는다. 시험 환경 재생성은 이전 재개 지점 단락의 방법과 같다(이번에는 컨테이너 이름 `aip-p729-*`, `scripts/mysql-live-fixture.sh`는 `AIP_FIXTURE_TAG=p729`).

## 이전 재개 지점 (2026-10-22, 7.2.9 `home/video/recommend` 뒤 기록)

7.2.9의 `POST home/video/recommend`를 쓰기 확장 `VideoRequest.recommend`로 옮겼다(결정 D-P-RECOMMEND). 확장이 목록의 URL마다 기존 공개 create를 같은 트랜잭션에서 부르고, `youtube_full_url`은 호출자가 못 쓰고 DB 기본값이 계산하며, 전역 중복 거절과 동시 요청 원자성은 `video_request.youtube_url` 유일 키(열은 `utf8mb4_bin`)가 맡는다(`db-defaults.sql`). 원본 `create_user_video_recommend`와 23개 사례가 같았다(`reference/slice14g_oracle.py`, CI 재생은 `mysql_facereview_trending_job`). 대소문자만 다른 id를 `ai_ci` 유일 키가 중복으로 거절하던 결함을 오라클이 드러내 열 collation으로 고쳤다. 플랫폼에 더한 것은 거절된 create 뒤 확장이 `WIRE.ALREADY_EXISTS`나 `WIRE.CONFLICT`로 끝나면 호출자가 그 코드를 받는 규칙 하나이고 감정 요약의 첫 행 생성 경쟁에도 적용했다. 변이로 유일 키 제거와 서버 전달 제거는 시험이 잡았고, 요약 확장의 `CONFLICT` 변환은 실행 증거가 없다(검증 기록 참고). 남은 7.2.9: dashboard(이전 재개 지점 단락의 분석이 그대로 유효: 시각 범위 filter, distinct·평균·합계 집계, Mongo 파생 명령 필요, 설계 관문 먼저), 시스템 상태, 더미 데이터. CI: f20f313 8종(Verify 39729331441 포함) completed/success. 시험 환경은 이 조각 끝에 회수한다(컨테이너 mysql·mariadb·mongo·redis·postgres 5개, `/private/tmp/aip-p729-*`와 `/private/tmp/aip-mysql-fixture-p729`).

## 이전 재개 지점 (2026-10-22, 7.2.8 조각 D 뒤 기록)

7.2.8의 마지막 읽기 `GET mypage/emotion/summary`를 쓰기 확장 `User.myEmotionSummary`로 옮겼다(결정 D-P-EMOSUMMARY). 이로써 7.2.8의 마이페이지 읽기 6개가 모두 구현됐다. 플랫폼에 더한 것: 쓰기 확장의 `access read.<Resource>`·`create.<Resource>`와 `ctx.data.read`·`create`·`apply(values, expect)`, 쓰기 확장 안의 Mongo·Redis 읽기를 효과 기록 없이 즉시 실행, `expect` 충돌을 `WIRE.CONFLICT`로 호출자에게 전달, 서버 파생 명령 `emotion_seconds`. 원본 `get_emotion_summary`와 19개 사례의 응답·행이 같았고(`reference/slice14f_oracle.py`, CI 재생은 `mysql_facereview_trending_job`) 원본 결함 하나(체크포인트 `DATETIME` 반올림으로 같은 세션을 매 호출 다시 더함)는 열을 `datetime(3)`으로 바꿔 고쳤다. 한계: 전이 `applyDelta`가 공개라서 회원이 자기 합계를 직접 쓸 수 있고(본인 화면만 영향), 확장만 부르는 전이 표시는 새 언어 기능으로 남겼다. 7.2.8의 미구현 잔여는 없으나 실제 메일 provider live smoke는 미실행이다. 다음은 7.2.9 잔여 4개(`home/video/recommend`, dashboard, 시스템 상태, 더미 데이터). 조각 D의 커밋은 4b99889이고 CI 8종(Verify 39724698719 포함)이 모두 success다. 시험 환경은 회수했다(컨테이너 6개, venv, `/private/tmp` 파일, 저장소 `target` 9.7G 삭제, 저장소 408M). 시험 환경을 다시 만들 때: MySQL 8.4·MariaDB 11.4는 `AIP_FIXTURE_TAG=<태그> AIP_FIXTURE_MYSQL_PORT=<포트> scripts/mysql-live-fixture.sh up`, mongo:7 인증은 15441(`MONGO_INITDB_ROOT_USERNAME=aip_test_slice13a`, `PASSWORD=local_fixture_only_13a`, `--tmpfs /data/db`), mongo:7 25443(인증 없음), redis:7.4 15442(`--requirepass local_fixture_only_13a`), postgres:17 5433(`POSTGRES_USER=runner`, `POSTGRES_HOST_AUTH_METHOD=trust`, 데이터베이스 `aip_test_ci`)이고 `AIP_TEST_DATABASE_URL='host=127.0.0.1 port=5433 dbname=aip_test_ci user=runner'`, `AIP_TEST_ADMIN_URL`(dbname=postgres), `AIP_TEST_MYSQL_URL`, `AIP_TEST_MONGO_URL`(`?authSource=admin`), `AIP_TEST_REDIS_URL=redis://:<비번>@127.0.0.1:15442`, `AIP_TEST_MONGO_UUID_URL=mongodb://127.0.0.1:25443`을 내보낸다. 원본 대조 venv는 `python3 -m venv` 뒤 Flask 3.0.2·Werkzeug 3.0.2·flask-socketio·flask-cors·SQLAlchemy 2.0·Flask-SQLAlchemy 3.1.1·PyMySQL·pymongo·cryptography·celery·redis·flask-smorest·marshmallow·PyJWT·isodate·requests·psutil이며 스크립트는 `numbers.py`가 없는 디렉터리에서 돌린다. 주의: 이 환경의 훅은 heredoc 안의 백틱을 막으니 백틱이 든 문서·코드는 Edit·Write로 쓰고, `git checkout <파일>`은 커밋 전 변경을 통째로 되돌리니 변이 시험 뒤 복원은 미리 만든 복사본으로 한다.

7.2.9 dashboard를 원본 `AdminService`(`get_business_stats`·`get_signup_trend`, 라인 380~585)에서 읽은 요약: (1) 가입 추이는 `func.date(User.created_at)`로 일별 집계(7d 7개, 30d 30개, 3m 주 단위 13개, 1y 월 12개, 3y 월 36개 버킷, 파이썬에서 묶음)라서 `User` 개수 집계에 시각 범위 filter(`created_at` 이상/미만)가 있으면 버킷당 한 번의 집계 호출로 표현할 수 있고 호출은 최대 36~40회라 확장 ctx 64회 안이다. 현재 `User` 읽기·집계에는 시각 filter가 없다. (2) WAU는 `VideoViewLog`의 최근 7일 `COUNT(DISTINCT user_id)`(Redis 캐시 키 `_WAU_CACHE_KEY`)라서 distinct 집계가 필요하다. (3) 요청 파이프라인은 PENDING 개수(기존 집계 + status filter)와 최근 30일 처리분의 `AVG(TIMESTAMPDIFF(MINUTE, created_at, updated_at))`라서 시각 차이 평균 집계가 필요하다. (4) 콘텐츠 건강은 Mongo `video_distribution`(`total_frames > 0`)의 평균·`dominant_emotion` 개수(파생 읽기 명령이 현실적, D-P-USERSTATS 방식)와 `Video.category`별 `SUM(view_count)`(16개 장르, 장르별 합계 집계 16회 또는 새 grouped 집계)다. 따라서 필요한 플랫폼 기능은 시각 범위 filter, distinct·평균(시각 차이)·합계 집계 또는 grouped 집계, Mongo 파생 명령이고 설계 관문(원칙 1·7)과 DECISIONS 기록이 먼저다. 시스템 상태(`get_system_status`, 라인 586~)는 psutil 호스트 지표와 Redis 요청 지표를 읽고, 더미 데이터(`generate_dummy_data`, 라인 687~)는 관리자가 한 회원의 세션 문서를 Mongo에 만드는 기능이다(시험 뒤 로컬 더미 잔여 회수와 구별한다).

## 이전 재개 지점 (2026-10-22, 7.2.8 조각 C 뒤 기록)

마이페이지 감정 읽기 5개(최근 시청·하이라이트·캘린더·모먼트·DNA)를 서버 파생 읽기 명령 `user_watch_stats`와 읽기 확장으로 옮겼고 원본 `MypageService`와 66개 사례가 같았다(`reference/slice14e_oracle.py`, CI 재생은 `mysql_facereview_trending_job`). 7.2.8의 남은 것: `GET mypage/emotion/summary`(`user_emotion_summary` 낙관적 잠금 증분 집계, 쓰기 필요: `UserEmotionSummary` 전이와 새 파생 명령 `emotion_seconds`가 필요하다). 그 뒤 7.2.9 잔여(`home/video/recommend`, dashboard, 시스템 상태, 더미 데이터). 시험 환경은 항목 끝에 회수한다(MySQL 8.4 `--rm`, mongo:7 인증, redis:7.4 인증, `/private/tmp`의 venv). 로컬 검증은 `CARGO_INCREMENTAL=0`과 저장소 루트 `--target-dir target` 하나만 쓴다.

## 디스크 정리 기록 (2026-10-22)

사용자 긴급 시정으로 AIP 빌드 산출물을 실제로 지웠다. 실측(`du`, `df -h /`의 Avail): 저장소 `projects/aip/aip` 52G에서 407M, `projects/aip` 전체 54G에서 3.1G, 루트 Avail 159Gi에서 235Gi. 지운 것은 모두 gitignored 빌드 산출물이거나 AIP 임시 파일이다: 저장소 `target/` 49G, `spikes/spike-v3-write/target` 1.5G, `spikes/spike-v6-transport/target` 0.77G, worktree `aip-mysql-ddl-fence-20261017/target` 18G와 `aip-mysql-driver-20261017/target` 7G(둘 다 git clean이고 origin과 같은 SHA, 소스는 보존), `/private/tmp`의 AIP 검증 로그·pgdata·minio 이미지·oracle venv·생성 .aip 등 약 0.5G, 그리고 이번 작업이 띄운 컨테이너 6개. 보존한 것: 추적 파일 전부, `product/dist`의 ignored 배포 증거, `~/.cargo` 공유 캐시, `/tmp/aip-gh-cache`·`aip-security-advisory-db`·`aip-update-npm-cache`, `~/development/target`(96M, 소유 불명), 다른 트랙(notting, flaskbb)의 `/tmp`와 컨테이너. 실행 중인 cargo·rustc 프로세스와 `lsof` 점유가 없는 것을 확인한 뒤 지웠다. 다음 작업은 `cargo build`부터 다시 한다(약 50G를 다시 쓰므로 항목이 끝날 때마다 `target/`과 임시 fixture를 회수한다: `target/debug/incremental`과 `target/debug/deps`의 오래된 테스트 바이너리가 대부분을 차지한다).

## 재개 지점 (2026-10-22 밤, 7.2.9 요청 승인 뒤 기록)

7.2.9의 요청 승인을 durable job `approveVideoRequest`로 옮겼다(29ec998): 쓰기 확장이 전이만 부를 수 있어서 7.2.7의 job `create`·Mongo 쓰기를 썼고, `VideoRequest.accept` 전이는 요청의 `youtube_url`을 가진 `Video`가 있을 때만 가능하다. 원본 `AdminService.approve_video_request`와 영상 행·Mongo 문서 2개·요청 상태가 같았다(`reference/slice14b_oracle.py`). 7.2.9의 남은 것: `POST home/video/recommend`(요청 생성: 서버 파생 `youtube_full_url`과 목록 전체 중복 거절. 정의 언어에 create의 계산된 값이 없어 새 언어 기능, 또는 사용자가 부르는 job이 필요하고 job은 비동기라서 선택이 필요하다. 원본은 `video_request.youtube_url`에 유일 인덱스가 없어 경쟁 상태에서 중복을 막지 못한다), dashboard(가입 추이·WAU·완주율: 날짜 구간 집계와 Mongo), 시스템 상태(호스트 지표), 더미 데이터(Mongo 쓰기). 7.2.8의 남은 마이페이지 Mongo 읽기 6개는 현재 Mongo 읽기 capability가 막는다: 읽기 filter는 동등과 `$in`뿐(날짜 범위 `$gte/$lt`·증분 `finalized_at > 체크포인트` 불가), 문서는 호출당 최대 100개·응답 1MB, 읽기 확장은 ctx 호출 64회, 그런데 원본은 `emotion_score_timeline`(초당 2프레임 × 영상 길이)이 큰 문서를 200개~전체 읽어 집계한다. 권장: 7.2.3의 `video_emotion_timeline`처럼 사용자 본인 `youtube_watching_data` 위의 서버 파생 읽기 명령(압축 40버킷·감정 초 합계·캘린더 일별·피크 모먼트·DNA 통계)을 서버에서 계산하고 확장은 점수 규칙(`_determine_dna_type`·`_compute_traits` 등)과 캐시 표(`UserEmotionSummary`·`UserEmotionDna`) 갱신만 맡기는 설계다. 이것은 읽기 capability 계약의 확장이라 DECISIONS에 대안 비교와 함께 먼저 적어야 한다(이번에는 구현하지 않음). CI 주의: main push는 이전 Verify를 취소한다(concurrency). 연속 push 뒤 Verify 결과는 마지막 SHA의 것만 남으니 push 사이에 30분 정도 둔다. 1951b9f·24ddd30·67cc27d의 Verify는 취소됐고, 그 변경을 모두 포함한 29ec998은 Verify 39708253311을 포함해 7종이 success다(Linux worker jobs 39708253334, Database contracts 39708253401 등).

## 이전 재개 지점 (2026-10-22, 7.2.8 조각 A·B 뒤 기록)

7.2.7은 done이다(아래 절). 7.2.8(마이페이지·이메일·비밀번호·탈퇴·감정 집계)은 관계형·인증 부분을 두 조각으로 끝냈고 doing이다. 조각 A(1951b9f): 로그인 때 약한 해시 재해시(검사한 해시일 때만 갱신), `User.completeTutorial`·`User.updateProfile`·`UserFavoriteGenre` create/remove(선호 장르 교체는 `atomic` 묶음), `/auth/me`는 본인 `User`+`UserFavoriteGenre` 읽기, 탈퇴는 기존 `User.withdraw`이고 다음 reissue가 모든 세션을 끝낸다. 플랫폼 수정: `repeat unchanged` 판정에서 시계(`now`)로 정하는 필드를 뺐다(MySQL·PostgreSQL 쓰기 경로). 조각 B(24ddd30): `credentials.mail`로 `/auth/email/verification`·`/auth/email/verify`·`/auth/password/verification`·`/auth/password`(Bearer 필요, 번호·토큰은 `aip_auth_challenges`에 digest, 시도 5회 제한, 비밀번호 변경은 다른 세션 폐기), 결정 D-P-CREDMAIL. 남은 7.2.8: 마이페이지 Mongo 읽기 6개(`videos/recent`·`emotion/summary`·`highlight`·`emotion/calendar`·`moments`·`emotion-dna`, 원본 `MypageService` 라인 250~760과 `_DNA_TYPES`). `emotion/summary`는 GET 안에서 `UserEmotionSummary`를 낙관적 잠금(`lock_version`)으로 갱신하는 증분 집계라서 읽기 확장만으로는 안 되고 `UserEmotionSummary` 전이(또는 쓰기 확장)가 필요하다. 원본 대조는 `reference/slice13e_oracle.py`·`slice13f_oracle.py` 방식(원본 `MypageService`를 같은 MySQL·MongoDB에서 직접 실행)을 쓴다. 그 다음 7.2.9의 남은 부분이다. CI: 73e9470은 7종 success(Verify 39703774966). 1951b9f·24ddd30의 CI는 push 직후라 확인 전이다.

## 이전 재개 지점 (2026-10-22, 7.2.7 완료 뒤 기록)

7.2.7(예약 업무)은 원본 Celery beat 3작업(`rebuild_recommendation_pool`, `fetch_youtube_trending_videos`, `fill_youtube_category_videos`)이 모두 durable job으로 옮겨져 완료다(조각 A·B·C, 근거는 [제품 검증](../product/VERIFICATION.md)의 7.2.7 세 절). 조각 C(카테고리 보충)는 원본 `execute()`와 36행·72문서·quota 1212가 같았고 실서버 시험이 통과했으며, 장르 사이 quota 검사를 빼는 변이가 시험을 깨뜨렸다. 같은 커밋이 CI Verify 39664738540의 실패(풀 job 시험이 예약 run의 정산을 기다리지 않고 개수를 센 경합)를 고쳤다. 한계: 서버 설정 `limits.job_calls`를 1000 이상으로 올려야 한다, 트렌딩·보충의 예약(`schedules`)은 서버 설정이며 이 두 job만의 예약 시험은 하지 않았다(풀 job이 같은 경로를 보였다), 실제 YouTube credential이 없어 live smoke는 미실행이다. 다음은 7.2.8(인증 잔여: 약한 해시 재해시, `/auth/me`·`/auth/tutorial`, 탈퇴 때 세션 폐기, 마이페이지·이메일·비밀번호 변경·감정 집계), 이어서 7.2.9의 남은 부분(요청 승인은 job `create`와 Mongo 쓰기 capability가 이미 있어 쓰기 확장 쪽 `create` 호출만 필요한지 확인, dashboard, 시스템 상태, 더미 데이터)이다. 시험 환경 준비는 아래 7.2.7 조각 A 단락과 같다.

## 이전 재개 지점 (2026-10-22, 7.2.7 조각 A 뒤 기록)

7.2.7(예약 업무)은 조각 A(추천 풀 캐시의 예약 갱신)까지 끝났고 doing이다. 남은 것은 조각 B(YouTube 인기 영상 수집 `YoutubeTrendingJob`)와 조각 C(카테고리 보충 `YoutubeCategoryFillJob`)이며 둘 다 새 플랫폼 기능이 먼저 필요하다. (1) HTTP 연동이 JSON 본문 POST뿐이라 YouTube Data API의 GET 쿼리(`pageToken`·`id`·`q`, API key 쿼리 파라미터)를 부를 수 없다: 서버 설정에 허용 파라미터 목록과 비밀 쿼리 파라미터(`secret_query_env`)를 두고, 확장은 허용된 스칼라 값만 주는 방식을 권한다(URL·메서드·키는 서버가 정한다는 기존 원칙의 연장). (2) job ctx가 전이만 부를 수 있어 새 `Video` 행(원본 `_save_video`)을 만들 수 없다: job의 `create` capability가 필요하고 7.2.9의 요청 승인(영상 생성과 `video_distribution` 초기 문서)도 같은 기능을 쓴다. Mongo 초기 문서는 이번에 연 job Mongo 쓰기(`update_one` + `$setOnInsert`, 반복 가능)로 된다. (3) 중복 확인용 `Video` 읽기 필터 `youtube_url.in`과 카테고리별 개수 집계가 정의에 없다. 원본 대조는 원본 job 클래스의 `_map_youtube_category`·`_classify_category_by_keywords`와 `requests.get`을 로컬 가짜 YouTube로 돌린 `execute()` 결과(Video 행과 Mongo 문서)를 쓰는 방식이 가능하다(`reference/slice14a_oracle.py`가 같은 원본 로딩을 이미 한다). 시험 환경은 이전 단락에 더해: 인증 있는 MongoDB 7(`mongo:7`, `MONGO_INITDB_ROOT_USERNAME=aip_test_slice13a`·`MONGO_INITDB_ROOT_PASSWORD=local_fixture_only_13a`, 15441, URL에 `?authSource=admin`), 별도 mongod(25443), Redis 7.4(15442), `scripts/mysql-live-fixture.sh up`(`AIP_FIXTURE_TAG`로 컨테이너 이름 지정), PostgreSQL 17(5433, `POSTGRES_USER=runner`·`POSTGRES_HOST_AUTH_METHOD=trust`)과 `AIP_TEST_DATABASE_URL`·`AIP_TEST_ADMIN_URL`. 원본 대조용 venv는 `python3 -m venv` 뒤 Flask 3.0.2·Werkzeug 3.0.2·SQLAlchemy 2.0·Flask-SQLAlchemy 3.1.1·PyMySQL·pymongo·redis·cryptography·PyJWT·flask-socketio·flask-cors·celery·flask-smorest·marshmallow·isodate·requests를 설치하고 `/tmp`가 아닌 디렉터리에서 실행한다. 그 다음은 7.2.8(인증 잔여)과 7.2.9의 남은 부분이다.

## 2026-10-22 WBS 7.2.7 조각 A: 추천 풀 캐시의 예약 갱신과 durable job의 Mongo·Redis 쓰기 (구현 조각, 로컬 실DB·실서버·원본 서비스 대조 통과, 7.2.7은 진행 중)

원본 Celery beat 작업 `rebuild_recommendation_pool`(30분 주기)을 durable job `rebuildRecommendationPool`로 옮겼다. 플랫폼에 job의 `access mongo.<collection>`·`access redis "<키>"` 쓰기(effect 장부에 key로 남고 재시도는 재생, 응답을 잃은 쓰기는 다시 보내도 같은 데이터가 되는 명령만 같은 key로 다시 보냄, job에는 saga 없음)와 서버 설정 `limits.job_calls`(기본 64)를 더했고 결정 근거는 [DECISIONS](../docs/DECISIONS.md)의 D-P-JOBSTORE다. 읽기 확장은 캐시를 먼저 읽고 비면 요청 안에서 풀을 만든다. 원본 `_build_and_cache_ranked_pool`과 같은 데이터에서 풀 19개·카테고리 6개·순서·만료가 모두 일치했다. 시험이 찾은 것: `Video` 읽기의 offset 비용 때문에 읽기 확장의 "5000개 상한"이 실제로는 약 800개였다(`cursor`를 선언해 keyset으로 읽게 고침), 읽기 확장의 폴백은 64 ctx 호출이라 약 3000개에서 실패한다, 서버가 job을 서빙하지 않을 때도 계약 지문을 job 포함으로 출력하던 기존 결함(`/read`가 `CONTRACT_MISMATCH`). 명령·결과·변이 확인·한계는 [제품 검증](../product/VERIFICATION.md)의 같은 날짜 조각 A 절이다.

## 이전 재개 지점 (2026-10-22, 7.2.5 완료 뒤 기록)

7.2.5(Mongo·Redis capability)가 done이다. 조각 A(`GET watch/video`, cee2d8a)와 조각 B(Mongo·Redis 쓰기 capability와 saga, c621143) 모두 main에 push됐고 Linux CI Verify가 success다. 다음은 7.2.7(예약 업무: 추천 풀 Redis 쓰기는 `set_ex` 1MB·`replace_hash_ex`로 가능, 쓰기 확장이 아니라 durable job의 ctx에 Mongo·Redis 쓰기를 열어야 하는지부터 확인), 7.2.8(마이페이지·이메일·비밀번호·탈퇴: 인증 코드와 재설정 토큰은 Redis `set_ex`·`getdel`, 메일은 외부 provider), 이어서 7.2.9의 남은 부분(요청 승인은 `video_distribution` 초기 문서 쓰기, 지표는 `incr_expire`·`push_trim_expire`·`lrange`, dashboard는 날짜 구간 집계)이다. 시험 환경 준비는 아래 단락과 같고 새로 필요한 것: Redis 7.4(`docker run --rm -p 127.0.0.1:15442:6379 redis:7.4 redis-server --requirepass local_fixture_only_13a`), 인증 있는 MongoDB 7(15441), 별도 mongod(25443, `AIP_TEST_MONGO_UUID_URL`), 그리고 spike-v4-worker 테스트가 쓰는 PostgreSQL(`AIP_TEST_DATABASE_URL`은 5433에 `--rm` postgres:17). 전체 검증은 `tools/verify.sh`의 cargo 단계를 같은 순서로 돌린 스크립트로 18단계 통과를 확인했다. 스크립트에 `PGPORT`를 내보내면 DB 가드가 거절한다. 주의: 이 환경의 훅이 heredoc 안의 백틱을 "명령 이름을 치환으로 구성"으로 막으니, 백틱이 든 문서·코드는 Edit·Write 도구로 쓴다.

## 이전 재개 지점 (2026-10-21 밤, 7.2.9 착수 전 기록)

7.2.2·7.2.3 완료(main에 push, CI success). 7.2.9(관리자, doing)는 읽기 조각(회원·요청·영상·댓글 목록 4개, 원본 `AdminService`와 62/62 일치)까지 구현했고 아래 "WBS 7.2.9 관리자 목록 읽기" 절과 [제품 검증](../product/VERIFICATION.md)에 기록했다. 남은 7.2.9: 요청 승인, dashboard, 시스템 상태, 더미 데이터(아래 막힌 것). 원본 `AdminService`를 같은 데이터에서 나란히 돌려 대조하는 `slice13e_oracle.py`(`slice13d_oracle.py`의 배포·원본 로딩을 재사용) 방식이 dashboard 읽기에도 쓰인다(venv·컨테이너 준비법은 아래). 막힌 것: 요청 승인(영상 생성과 Mongo 초기 분포 쓰기: 쓰기 확장은 전이만 부를 수 있고 Mongo는 읽기 capability뿐, 7.2.5), 더미 데이터(Mongo 쓰기), 시스템 상태(호스트 지표), dashboard(기간별 가입 추이·WAU·완주율은 날짜 구간 집계와 Mongo 필요, 7.2.5·7.2.7), `POST home/video/recommend`(7.2.3에서 넘어옴). 선행 7.2.5·7.2.7·7.2.8이 todo라서 7.2.9를 완료로 세려면 이들이 먼저다. 시험 준비: 원본 체크아웃(`~/development/projects/facereview/new-facereview`)은 CRLF라서 모든 파일이 수정으로 보이지만 내용은 같다(오라클이 `--ignore-cr-at-eol`로 판정). 원본 서비스를 돌릴 venv는 `python3 -m venv` 뒤 `Flask==3.0.2 Werkzeug==3.0.2 flask-socketio flask-cors SQLAlchemy>=2.0.36,<2.1 Flask-SQLAlchemy==3.1.1 PyMySQL pymongo cryptography celery redis flask-smorest marshmallow PyJWT`를 설치하면 된다(`/tmp`에서 실행하면 `/tmp/numbers.py`가 표준 라이브러리를 가리니 다른 디렉터리에서 실행). MySQL 8.4는 `scripts/mysql-live-fixture.sh up`(태그·포트는 환경 변수), MongoDB는 `docker run --rm --tmpfs /data/db mongo:8.0`.

## 2026-10-22 WBS 7.2.5 조각 B: Mongo·Redis 쓰기 capability와 외부 효과 saga (구현, 로컬 실DB·실서버 통과)

쓰기 확장이 관계형 트랜잭션 옆에서 MongoDB·Redis에 쓰고, 트랜잭션이 커밋되지 않으면 되돌리는 경로를 만들었다(원본 `common/saga`의 의미). 문법 `access redis "키 또는 접두어:*"`(읽기·쓰기 확장)와 쓰기 확장의 `mongo.<collection>`, 쓰기 명령(Mongo 4개, Redis 8개와 읽기 `lrange`), `compensate=True`가 남기는 보상 기록, `ExternalEffects`(효과 순서·최신 단계부터 보상·원본 모양의 `saga_transaction_log`), 커밋 불명은 보상하지 않고 `unknown`, 보상 실패는 새 코드 `WIRE.COMPENSATION_FAILED`, 설정 `workers.mongo.write_collections`·`ensure_indexes`가 들어갔다. 실제 MongoDB·Redis·MySQL·`aip service serve`로 성공·재생·확장 실패·검사 실패·Mongo 실패·Mongo 불통·Redis 불통·커밋 응답 유실을 확인했고 롤백 호출을 빼는 변이가 시험을 깨뜨리는 것도 확인했다. 한계: 프로세스가 커밋 전에 죽으면 `in_progress` log만 남고 자동 복구는 없다, Node worker 새 메서드는 문법 검사만 했다. 명령·결과는 [제품 검증](../product/VERIFICATION.md)의 같은 날짜 조각 B 절이다. 7.2.5의 나머지는 없고(원본 프레임 쓰기 흐름은 7.2.6, 인증·이메일 Redis 사용은 7.2.8, 추천 풀 캐시 갱신은 7.2.7이 이 capability를 쓴다), Linux CI는 c621143의 Verify(39650801406)가 `mysql_facereview_watch`·`mysql_external_effects`를 포함해 success다. 그래서 7.2.5를 done으로 센다.

## 2026-10-22 WBS 7.2.5 조각 A: `GET watch/video`와 조회수 전이 (구현 조각, 로컬 실DB·실서버·원본 서비스 대조 통과, 7.2.5는 진행 중)

7.2.3이 넘긴 `GET watch/video`를 옮겼다. Mongo capability에 파생 컬렉션 `video_emotion_timeline`(서버가 전 시청자 타임라인을 합산·100버킷화해 평균만 돌려주고 원본 세션은 열지 않음)을 더했고, `Video.watchDetail` 읽기 확장과 `Video.view` 전이(원본이 GET 안에서 하던 `view_count += 1`을 호출자가 읽기 뒤에 명시 호출)를 정식 정의에 선언했다. 정의 언어가 증감 대입을 null이 아닌 Int에만 허용해서 `db-defaults.sql`이 `video.view_count`를 NOT NULL로 바꾼다(원본은 NULL을 쓰지 않는다). 같은 데이터에서 원본 `get_video_detail`과 42/42 일치했다(영상 12 x 호출자 3 + 쓰기 6). 첫 실행의 3건 불일치(48.1 대 48.2)는 Mongo `$sum`의 보정 합산이 원인이어서 서버 쪽 합산을 원본과 같은 순서의 단순 합으로 바꿔 풀었다. 명령·결과·변이 확인은 [제품 검증](../product/VERIFICATION.md)의 2026-10-22 절이다. 7.2.5의 나머지(Mongo·Redis 쓰기 capability, 다중 저장 보상, TTL·네임스페이스)는 아직이다.

## 2026-10-21 WBS 7.2.9 관리자 목록 읽기 (구현 조각, 로컬 실DB·실서버·원본 서비스 대조와 Linux CI 통과, 7.2.9는 미완)

원본 `AdminService.get_users·get_video_requests·get_videos·get_comments`를 정식 정의의 읽기로 옮겼다. `User`의 행별 집계 `watchCount`·`commentCount`, 관리자 전용 `totalCount`(`User`·`VideoRequest`·`Comment`), `name.like`·`email.like`·`content.like` 필터, `VideoRequest`·`Comment`의 연관 행 요청을 더했고, 같은 데이터에서 원본 `AdminService`를 직접 실행해 62개 사례가 모두 같았다(관리자 2명 x 29상황과 일반 회원 거절 4건). 알려진 차이는 하나다: 삭제됐거나 NULL 플래그인 영상은 `Video` 행 정책이 관리자에게도 숨기므로 그 영상 댓글의 `video_title`이 null이고 `video` 필터로 그 댓글을 찾을 수 없다(원본은 보여 준다). 요청 승인(영상 생성과 Mongo 쓰기), dashboard, 시스템 상태, 더미 데이터와 `POST home/video/recommend`는 아직 없다. 명령·결과·변이 확인은 [제품 검증](../product/VERIFICATION.md)의 같은 날짜 절이다.

## 2026-10-21 WBS 7.2.3 추천 목록 포팅과 정식 정의 통합 (구현, 로컬 실DB·실서버·원본 서비스 대조와 Linux CI 통과)

원본 `home/category`·`home/personalized`·`watch/recommended`를 읽기 확장으로 옮겼다. 정식 `facereview.aip`가 이제 홈·시청 목록 확장 6개(검색·전체·북마크 3개와 추천 3개)를 Mongo 권한과 함께 직접 선언하고 `aip service serve`가 바로 서빙한다. 이를 위해 제품에 세 가지를 더했다: 읽기 확장의 `access mongo.<collection>` 선언, `Video`의 공개 `likeCount`·`commentCount` 집계, `youtube_watching_data` 프로젝션의 `emotion_percentages` 허용. 실제 서버와 원본의 `HomeService`·`WatchService`를 같은 MySQL 8.4·MongoDB 데이터에서 나란히 돌려 41개 답이 모두 같았고(같은 명령 3회 golden 동일), CI는 그 golden을 새 배포에 재생한다(`mysql_facereview_recommend`). 시험이 찾은 원본의 숨은 의미: 선호 장르 순위는 사용자가 고른 순서가 아니라 `(user_id, genre)` 유일 인덱스가 돌려주는 ENUM 선언 순서다. 명령·결과·변이 확인·한계는 [제품 검증](../product/VERIFICATION.md)의 같은 날짜 절이다. Redis 풀 캐시는 쓰지 않고 요청마다 풀을 만든다(오프라인 갱신 7.2.7). **옮기지 못한 둘**: `GET watch/video`(전 시청자 타임라인 집계는 본인 세션만 주는 Mongo capability가 막고, GET 안의 조회수 증가는 쓰기 확장이 전이만 부를 수 있어 못 묶는다)는 7.2.5로, `POST home/video/recommend`(서버 파생 `youtube_full_url`, 전역 중복 거절, 한 트랜잭션)는 계산된 기본값과 `video_request.youtube_url` 유일 인덱스가 필요해 7.2.9로 넘겼고 이 항목의 완료로 세지 않는다.

## 2026-10-21 WBS 7.2.2 비밀번호 로그인·가입·재발급·로그아웃 (구현, 로컬 실DB·HTTP와 Linux CI 통과)

MySQL 서비스가 외부 IdP 없이 FaceReview 계정으로 로그인할 수 있게 했다. 서버 설정의 `credentials`를 켜면 `/auth/check-email·signup·login·reissue·logout`이 생긴다. 비밀번호는 공식 Python 확장(`product/providers/password_auth.py`, Werkzeug 3.0.2 형식)을 샌드박스 worker에서 실행해 확인하고, 서버가 읽은 해시는 확장 밖으로 나가지 않는다. 가입한 계정과 원본이 만든 계정은 첫 로그인 때 서버가 계정 행의 UUID로 actor에 연결하며, 클라이언트는 role·owner·id를 정할 수 없다(알 수 없는 키는 422, role은 설정의 기본값). 접근 토큰은 서버가 서명한 `at+jwt`(300초)이고 같은 검증기를 통과하며 로그인 세션이 살아 있어야만 받는다. refresh token은 단회용이다. 원본은 이미 쓴 refresh token을 다시 받아 새 접근 토큰을 발급했지만(고정 소스와 실제 API로 확인한 보안 결함) 이 포팅은 재사용을 감지해 세션 전체를 폐기한다. 로그아웃·정지 계정·운영자의 `principal revoke`가 쿠키와 접근 토큰을 함께 막는다. 시험은 실제 `aip service` 바이너리, 원본 11개 표 스키마의 격리 MySQL 8.4, Seatbelt 안의 Python worker에서 한 건(`mysql_facereview_auth`)이 통과했고 세션 저장소는 MySQL 8.4와 MariaDB 11.4에서 각 4건이 통과했다. 명령·결과·변이 확인·한계(로그인 시도 제한, 재해시, `/me`·`/tutorial`, 브라우저 쿠키 실행, Linux CI)는 [제품 검증](../product/VERIFICATION.md)의 같은 날짜 절이다. 동시 가입 시험이 찾은 `principal bind` gap lock 교착(3회 중 1회)을 고쳤고 연속 8회 통과했다. Linux CI는 `673ee0e`의 Database contracts·Deployment checks와 `30ad7b2`의 Verify(Bubblewrap 안 Python worker와 이 인증 E2E 포함) 등 7종이 모두 success다. 로그인 시도 제한(proxy 책임)·약한 해시 재해시·`/auth/me`·`/auth/tutorial`·탈퇴 때 세션 폐기는 7.2.8, 브라우저의 교차 사이트 쿠키 실행과 화면 문구 분기는 7.2.10으로 넘겼고 이 항목의 완료로 세지 않는다. 이 절은 7.2.1.3의 제외 범위(SSE·WebSocket·`events_days`, `file_mounts`·`/files/mount-get`·`objects`, 명시 handler owner, 일부 실DB 경계 시험, MariaDB 서빙)를 지우지 않으며 7.2.6·7.2.10·7.2.11에서 추적한다. PostgreSQL `relay_messages` 재전달 문제는 코드 대조에 따른 추정이고 PostgreSQL 실행은 하지 않았으므로 측정한 결함으로 주장하지 않으며 별도 추적을 유지한다.

## 2026-10-21 WBS 7.2.1.3 MySQL 관리 파일 저장소 capability (구현, Linux CI 통과)

`access files.<bucket>`과 클라이언트 경로 `/files/put·get·delete`를 MySQL에 연결했다. 저장소는 `aip_managed_files` 한 표다(PostgreSQL의 `aip_files`와 이름을 나눴다). 부트스트랩과 `adopt-external --apply` 재실행이 만들고, `workers.enable_file_uploads`이거나 job이 `files.*`를 선언한 서비스만 시작 때 표를 검사한다. 이제 `enable_file_uploads` 설정을 거절하지 않는다. 바이트는 표에 LONGBLOB로 1MiB까지 두고, 핸들은 불투명 UUID이며 경로나 URL은 노출하지 않는다. 같은 이름에 같은 바이트·형식·만료를 다시 넣으면 같은 파일이고 다르면 충돌이다. 읽기는 owner·bucket·만료를 호출마다 데이터베이스 시계로 다시 검사한다. owner당 100개·10MiB 한도를 `aip_tx_locks` 버킷 잠금 아래에서 검사하고(만료된 파일은 세지 않고 이미 있는 이름의 재전송은 새 파일로 세지 않는다), job의 파일은 run과 effect로 범위를 좁혀 재시도가 같은 파일로 수렴하며 run 기한 하루 뒤에 만료된다. job의 put은 effect 장부와 같은 fenced transaction에 커밋되고 get은 장부 없이 읽는다. 만료 파일은 supervisor의 30초 정리 주기에서 1000건씩 최대 10배치 지운다. `file_mounts`(HTTP로 파일을 서빙하는 mount)와 `/files/mount-get`, 객체 저장소(`objects`)는 연결하지 않았다. 이 증분으로 7.2.1.3의 메시지·파일·이벤트 저장소 capability가 모두 MySQL에 연결되었다. Linux CI는 main `a0daf81`의 Verify 39577769052(실제 Node·Python worker E2E 포함), Database contracts 39577768890, Linux worker jobs 39577768904 등 8종이 모두 success다. 이 실행이 이벤트·메시지·메시지 핸들러·관리 파일 시험을 MySQL 8.4와 MariaDB 11.4 CI 컨테이너에서, worker E2E를 Linux Bubblewrap에서 돌렸다. 명령과 결과는 [제품 검증](../product/VERIFICATION.md)의 같은 날짜 절이다.

## 2026-10-21 WBS 7.2.1.3 MySQL 메시지 핸들러와 만료 메시지 정리 (구현, Linux CI 통과)

`message_handlers`를 MySQL에 연결했다. 서버가 설정된 큐의 준비된 메시지를 서비스 principal의 job으로 바꾸고, job 접수와 메시지 ack가 같은 fenced transaction에 커밋된다. job 키는 메시지 id라서 재전달은 같은 job으로 수렴한다. 접수가 거부되면 nack로 백오프를 거쳐 배달 횟수를 다 쓰면 dead letter가 되고, 서비스 principal이 비활성이면 메시지를 클레임하지도 않아 다시 바인딩할 때까지 준비 상태로 남는다. 시험에서 PostgreSQL 경로와 같은 결함을 찾았다. job 입력에 `deliveries`가 들어 있어서 접수와 ack 사이에 서버가 죽었다가 메시지가 재전달되면 입력이 달라 `IDEMPOTENCY_MISMATCH`가 되고, 이미 만들어진 job이 있는데도 메시지가 nack되어 결국 dead letter로 간다. MySQL 경로는 같은 키의 job이 이미 있으면 메시지를 ack하고 넘어가도록 고쳤다. PostgreSQL 경로(`spikes/spike-v6-transport/src/durable.rs`의 `relay_messages`)는 고치지 않았고 같은 결함이 남아 있다고 본다(코드 대조만 했고 PostgreSQL 실행은 하지 않았다). 만료된 메시지는 supervisor의 30초 정리 주기에서 1000건씩 최대 10배치 지운다. 파일 저장소 capability가 남아 7.2.1.3은 닫지 않는다. 명령과 결과는 [제품 검증](../product/VERIFICATION.md)의 같은 날짜 절이다.

## 2026-10-21 WBS 7.2.1.3 MySQL job 메시지 큐 capability (구현, Linux CI 통과)

`access messages.<queue>`와 클라이언트 경로 `/messages/enqueue·claim·ack·nack·dead·redrive`를 MySQL에 연결했다. 저장소는 `aip_message_queue` 한 표다(PostgreSQL의 `aip_messages`와 헷갈리지 않게 이름을 나눴다). 부트스트랩과 `adopt-external --apply` 재실행이 만들고, `messaging.enabled`이거나 job이 `messages.*`를 선언한 서비스만 시작 때 표를 검사한다. 이제 `messaging` 설정을 거절하지 않는다. 발행은 owner당 1000건·64MiB 한도를 `aip_tx_locks` 버킷 잠금 아래에서 검사하고(이미 있는 키의 재전송은 한도를 건너뛴다), 같은 키·같은 본문은 같은 메시지이며 다른 본문은 충돌이다. job의 발행은 effect 장부와 같은 fenced transaction에 커밋되어 재시도가 같은 메시지를 재생한다. 클레임은 `SKIP LOCKED`로 후보를 고르고 조건부 갱신으로 가져가며(`RETURNING`이 없어 토큰으로 다시 읽는다), ack·nack는 살아 있는 임차권만 커밋하고 배달 횟수를 다 쓰면 dead letter가 된다. 만료된 임차권은 다음 클레임 때 READY나 DEAD로 돌아가고 redrive·만료 정리도 같은 계약이다. 클라이언트의 발행·소비는 설정의 `client_publish`·`client_consume`이 따로 켜야 하고 job이 선언하지 않은 큐는 `NOT_EXPOSED`다. `message_handlers`(메시지를 job으로 중계)와 만료 메시지 자동 정리 루프는 아직 연결하지 않았다. 파일 저장소가 남아 7.2.1.3은 닫지 않는다. 명령과 결과는 [제품 검증](../product/VERIFICATION.md)의 같은 날짜 절이다.

## 2026-10-21 WBS 7.2.1.3 MySQL job 이벤트 스트림 capability (구현, Linux CI 통과)

`access events.<stream>`을 MySQL job에 연결했다. 이벤트 저장소는 `aip_event_streams`·`aip_event_log`·`aip_event_keys` 세 표다(`aip_events`는 쓰기 펜스 표로 이미 있어 `aip_event_log`로 이름을 나눴다). 부트스트랩과 `adopt-external --apply` 재실행이 만들고, job이 `events.*`를 선언한 계약만 서비스 시작 때 표 존재·엔진을 검사한다. 발행은 스트림 행 잠금으로 순서를 매기고 같은 키·같은 본문은 같은 cursor로 수렴하며 다른 본문은 충돌이다. job의 발행은 effect 장부와 같은 fenced transaction에 커밋되어 재시도가 같은 cursor를 재생한다. 클라이언트는 `/events/read`로 자기 owner의 스트림을 읽고, job이 선언하지 않은 스트림은 `NOT_EXPOSED`다. SSE·WebSocket 구독과 보존 일수(`events_days`) 정리는 연결하지 않았다. 같은 작업에서 outbox relay가 다른 relay가 막 커밋한 행을 다시 잡아 영수증 삽입이 중복 키로 실패하는 기존 경쟁(12회 중 1회)을 `INSERT IGNORE`와 실제 삽입 행 수 계산으로 고쳤다. 메시지·파일 저장소 capability가 남아 7.2.1.3은 닫지 않는다. 명령과 결과는 [제품 검증](../product/VERIFICATION.md)의 같은 날짜 절이다.

## 2026-10-21 WBS 7.2.1.3 MySQL job 외부 HTTP capability (구현, Linux CI 통과)

`access http.<name>`을 MySQL job에 연결했다. 서버 시작 시 선언된 연동이 `workers.integrations`·`outbound_webhooks`에 없으면 거부한다. 호출은 PostgreSQL 경로와 같은 규칙으로 effect 장부에 남긴다. 의도(PENDING)를 먼저 커밋한 뒤 요청을 보내고 답을 같은 fenced transaction에서 기록한다. 같은 effect로 재시도하면 기록된 답을 재생하고 다시 보내지 않는다. 요청이 확실히 나가지 않은 경우에는 의도를 지워 같은 키로 다시 보낼 수 있게 하고, 나갔는데 답을 잃은 경우는 UNKNOWN으로 남겨 반복하지 않는다. outbound webhook은 수신 쪽이 webhook-id로 중복을 걸러내므로 결과가 불명이어도 같은 id로 재전송한다. 실제 MySQL 8.4 단위 시험과 Node·Python 실제 worker E2E(첫 시도가 재시도를 요청하고 두 번째 시도가 같은 호출을 재생, peer 요청 1건)가 로컬에서 통과했다. Push 뒤 main bb6cc09의 Verify·Database contracts·Linux worker jobs 등 8종이 모두 success다(Verify 39451511559, Database contracts 39451511837). 메시지·파일·이벤트 저장소 capability가 남아 7.2.1.3은 닫지 않는다. 명령과 결과는 [제품 검증](../product/VERIFICATION.md)의 같은 날짜 절이다.

## 2026-10-21 WBS 7.2.1.3 MySQL 트리거(schedule·webhook·outbox)와 job 교착 수정 (구현, Linux CI 통과)

`schedules`·`webhooks`·`outbox_handlers`를 MySQL job 저장소에 연결했다. 접수는 설정의 service principal로 같은 fenced transaction에서 사용자 enqueue와 같은 검사를 거친다. 서버가 만드는 키와 트리거 전용 연산은 사용자가 못 쓰게 막았고(MySQL 경로에는 키 접두사 검사만 있었다), outbox 영수증은 job 접수와 같은 트랜잭션에 쓴다. `message_handlers`는 계속 거부한다. 같은 작업에서 E2E를 반복하다 `/jobs/status`가 간헐적으로 `WIRE.DB_UNAVAILABLE`을 내는 기존 결함을 찾았다. MDL 고정 쿼리 `SELECT 1 ... LIMIT 1`이 직렬화 격리에서 인덱스 첫 레코드에 공유 락을 잡아 job claim·settle UPDATE와 교착했다. `WHERE 1=0`으로 바꿔 행 락 없이 MDL만 잡게 했다(InnoDB 교착 로그로 원인 확인, 새 시험은 옛 쿼리에서 실패). 로컬은 macOS Seatbelt에서 Node·Python 실제 worker로 확장 E2E를 4회 통과했다. Linux CI와 MariaDB 쪽 고정 쿼리 회귀는 CI 결과를 기준으로 한다. WBS는 메시지·파일·이벤트·외부 HTTP capability가 남아 완료로 닫지 않는다. 명령과 결과는 [제품 검증](../product/VERIFICATION.md)의 같은 날짜 절이다.

## 2026-10-20 WBS 7.2.1.3 HTTP job·관리 효과 연결 (구현, Linux CI 통과)

MySQL `/jobs/enqueue`·`status`·`result`·`cancel`을 제품 principal·계약 지문 검사 뒤에 연결했다. supervisor가 기존 `MysqlJobStore`에서 claim하고 실제 worker를 실행하며 heartbeat·재시도·취소·종료를 관리한다. 데이터 읽기·집계와 전이, 중첩 job은 선언한 capability만 쓴다. 전이·감사·effect 영수증은 같은 fenced transaction에 커밋하며, stale epoch와 다른 요청의 같은 effect key는 거부한다. 기존 배포는 migration 계정의 `adopt-external --apply` 재실행으로 AIP effect journal을 준비한다. runtime은 DDL을 실행하지 않고 영수증의 InnoDB·복합 기본키·필수 열을 시작 전에 검사한다.

Codex macOS 명령 환경에서 실제 MySQL HTTP·callback journal 통합 1 passed, MySQL/MariaDB 저장소 각각 3 passed, 기존 MySQL 쓰기 8 passed, 채택·JWT·기존 배포 journal 추가 2 passed다. 통합 시험의 dispatcher는 명시적으로 멈춰 두고 callback을 직접 실행하므로 실제 worker 실행 증거로 쓰지 않는다. 실제 HTTP→Node/Python job·재시도 중 effect 재생·중첩 job·실행 중 취소는 기존 Linux CI의 CLI E2E에 추가했고 Verify 39438678105에서 Node·Python 양쪽 모두 통과했다(`http_apply_runs_real_node_and_python_workers_in_one_mysql_transaction` ok. 같은 키 수렴, 재시도 attempt 2에서도 effect 1회, 중첩 effect 2건, 지연 취소, 실행 중 취소 포함). WBS는 완료로 닫지 않는다. 메시지·파일·이벤트·HTTP 외부 capability와 trigger 연결, MySQL job 전용 OTLP span은 이 증분에서 검증하지 않았다. 아래 10월 18일의 미연결 설명은 당시 상태다.

## 2026-10-18 독립 검증 반영: spike clippy, S3 늦은 PUT 보수화, 의존성 (Claude 구현, 독립 검증 대기)

spike-v6 `items after a test module`와 v1·v2 clippy를 고치고 `tools/verify.sh`가 workspace 밖 engine crate를 lint하게 했다. 서명 PUT의 첫 `HEAD` 부재가 예약을 풀던 경로를 "보류 후 한 번 더 확인"으로 바꿨고(`settling`), 두 번째 확인 뒤 도착하는 전송과 장부 이전 객체는 여전히 세지 못한다. rustls 0.23.46으로 올려 `tools/dependency-check.sh`가 통과한다. 전체 게이트는 이 증분 뒤 SHA에서 다시 돌리지 않았고 마지막 성공은 `a97f1b9`다. 자세한 명령·결과·한계는 [릴리스 게이트](../product/RELEASE-GATE.md)의 "후속 증분"을 따른다. 독립 검증 전이므로 8.5·8.7을 완료로 표시하지 않는다.

## 2026-10-18 WBS 8.7 main 게이트 실행 (Claude 실행, 독립 검증 대기)

다른 트랙이 fmt·clippy를 정리한 뒤 main `a97f1b9`를 고정해 `scripts/release-gate.sh --baseline 4c27040`을 실행했고 모든 단계가 통과했다(Rust 1212 passed). 앞선 후보 `4e78ddb` 결과는 별개로 남겼고 main 성공으로 바꿔 적지 않았다. S3 quota의 65분 이후 미계상 경계는 하드 상한이 아님을 시험으로 고정하고 영향을 [릴리스 게이트](../product/RELEASE-GATE.md)에 적었다. 인계 목록도 같은 문서에 있다. 독립 검증 전이므로 완료로 표시하지 않는다.

## 2026-10-18 WBS 8.10 독립 검증 결함 6건 수정 (Claude 구현, 독립 검증 대기)

nbf 선택 claim 허용과 실제 Keycloak smoke, IdP smoke 17번째 확인의 본문 검사, Python 코드젠의 3.9/3.10 import, job span 부모 연결과 실제 경로 시험, poc-runtime all-targets clippy, prototype smoke 전용 DSN 가드를 고쳤다. 명령과 결과, 확인하지 못한 것은 [제품 검증](../product/VERIFICATION.md)의 WBS 8.10 절을 따른다. 독립 검증 전이므로 완료로 표시하지 않는다.

## 2026-10-18 WBS 7.2.3 FaceReview MySQL 읽기 포팅과 어댑터 결함 수정 (Claude 구현, 독립 검증 대기)

원본 FaceReview의 실제 MySQL 스키마(SQLAlchemy가 만든 11개 표)에서 9개 resource를 업무 DDL 0건으로 adopt하고 `aip service serve`로 읽었다. 원본 앱의 SQLAlchemy 쿼리와 같은 질문 21개(검색, 카테고리, 최신순·NULL 플래그 제외, offset 페이징, 댓글 가시성, 본인 전용 표, 필드 값)가 모두 같은 결과를 냈다. 원본은 `created_at` 등을 Python에서 채우므로 표에 DB default가 없고, 그래서 `default database` 필드는 DBA 단계(`db-defaults.sql`) 전에는 adoption이 이름을 대며 거절한다. 어댑터 결함 둘을 고쳤다: ENUM 값을 소문자로 비교해 `'GENERAL'` 같은 실제 값이 거절되던 것, 런타임 계정에 `CREATE TEMPORARY TABLES`가 없을 때 서버 능력 probe가 서버를 "InnoDB 없음"으로 오판하던 것(이제 권한 이름을 댄다). 서버 능력 게이트 뒤 `mysql_adoption_flow`가 실패한 채 푸시했던 것도 바로잡았다([VERIFICATION](../product/VERIFICATION.md) 정정). 쓰기 조각도 같은 실제 스키마에서 확인했다: 댓글 생성·수정·soft 삭제, 좋아요·북마크·영상 요청 생성이 원본이 남기는 행과 같고(UUID v4, DB default 플래그, UTC 시각), 위조 owner·삭제된 영상·익명·남의 댓글은 거절되며 유일 인덱스는 `WIRE.ALREADY_EXISTS`다. 이 과정에서 표현식 기본값(`DEFAULT_GENERATED`)을 generated 컬럼으로 오인하던 결함도 고쳤다. 권한 조각도 확인했다(원본 `admin_required`·`super_admin_required` 규칙): 사용자 비활성화(SUPER_ADMIN은 누구나, ADMIN은 GENERAL만, 댓글 숨김 포함)·권한 변경·요청 거절·soft 삭제가 규칙대로 되고 거절된 호출은 행을 바꾸지 않는다. 비활성화한 회원이 계속 읽고 쓰던 결함이 비교 하네스에서 드러나 모든 정책에 `active(actor)`를 넣어 막았다. 표당 resource 하나라는 매핑 제약, 기본키가 외래 키인 1:1 표 2개(감정 요약·DNA)의 본인 행 정책, 승인(영상 생성), 로그인 자체(서버 확장 필요)는 남았다. 남은 차이: 좋아요·북마크 취소(물리 삭제 문법 없음), 제목 OR 채널 검색(filter가 AND만), 목록 total, JSON·낙관적 잠금 표 2개, 음수 범위 Int 문법, Mongo·Redis 의존 목록, 쓰기·인증(7.2.2·7.2.4). 자세한 내용은 [fixture README](../product/tests/fixtures/facereview-mysql/README.md). 독립 검증 전이므로 완료로 표시하지 않는다.

## 2026-10-18 WBS 7.2.4 완료 게이트 판정 (Codex 실행, 별도 검토 가능)

7.2.4 계약의 범위는 댓글 생성·수정·soft delete의 저장 의미와 권한, 좋아요·북마크 on/off/on의 행 상태, 본인 탈퇴 때 bounded 자식 정리와 Comment/User soft delete다. 새 격리 MySQL 8.4에서 `reference/writes.py`의 AIP `/apply` 검증이 0 실패했고, 원본 고정 revision `7948b7e721837fdb73c427cb54e12cad4ec2d5af`의 실제 댓글 SQLAlchemy 메서드와 제품 HTTP 결과도 생성·수정·타인 수정 거부·soft delete에서 대조됐다. `mysql_facereview_reads`는 1/1 통과했다. 이전 증거의 PG17 audit/상한 rollback, MySQL like/bookmark 및 탈퇴, audit·멱등·cache, 원본 like/bookmark toggle은 각각 기존 원시 로그와 manifest를 참조한다. helper의 고정 경로·포트·계정은 필수 환경 입력과 loopback/high-port/non-root/role 분리 검사로 바꿨다. 검증은 전체 Flask 앱·브라우저 route/UI parity를 뜻하지 않는다.

계약은 기존 AIP `/apply`·SDK `apply()`에 선언된 동작을 추가하도록 했고, source의 단일 toggle route/응답을 복제하라는 요구는 없다. create/remove를 유지하는 것이 원칙과 일치하므로 호환 REST endpoint는 추가하지 않는다. 브라우저 댓글 gate는 7.1.3.2.2, 전수 화면 상태는 7.1.3.2.3 및 SDK/UI 통합은 7.2.10에 남는다. MariaDB는 adapter 호환성 게이트인 7.2.1의 범위다. WBS 7.2.4는 코어 계약 기준 완료로 PMS를 갱신한다.

## 2026-10-18 WBS 8.7 릴리스 게이트 증거 묶음 (Claude 구현, 독립 검증 대기)

새 clone에서 전체 회귀, release 빌드·패키징, 묶음 smoke, 이전 릴리스(`4c27040`)의 schema 업그레이드와 두 롤백을 실행하는 `scripts/release-gate.sh`와 `product/tools/upgrade-drill.mjs`를 추가했다. 드릴이 실제 결함을 찾았다: 구버전 durable job schema를 새 binary가 거부했다(`b1b7d1b`). 후보는 main이 아니라 MySQL 트랙의 미완 커밋(fmt·clippy 실패)을 뺀 로컬 트리여서 main에서 게이트를 다시 실행해야 한다. 결과와 한계는 [릴리스 게이트](../product/RELEASE-GATE.md)를 따른다. 시작 가이드(`product/GETTING-STARTED.md`)와 개발용 토큰 helper를 더했다. 독립 검증 전이므로 완료로 표시하지 않는다.

## 2026-10-18 WBS 7.2.1.3 MySQL 작업 저장소·outbox 릴레이·MariaDB 게이트 (Claude 구현, 독립 검증 대기)

PostgreSQL job 저장소의 상태 기계를 MySQL 8.4로 옮겼다(`SKIP LOCKED` claim, DB 시계, 키 digest 유일 열, lease epoch fencing, backoff·deadline·cancel·redrive). 실 MySQL 8.4.11에서 40개 run을 8 worker가 정확히 한 번씩 claim했고, 후보 잠금·epoch 검사·epoch 증가를 각각 끄면 시험이 실패한다. outbox 릴레이는 claim·enqueue·영수증을 한 트랜잭션으로 해 4개 동시 릴레이가 30행을 한 번씩만 dispatch하고 실패 배치는 아무것도 남기지 않는다. 서버 능력 probe는 MySQL 8.4를 통과시키고 MariaDB 11.4.13을 LATERAL·`FOR SHARE OF` 부재로 이름을 대며 거부한다(SKIP LOCKED·JSON_ARRAYAGG·CHECK·InnoDB는 있다). MariaDB 11.4에서도 같은 job 저장소·릴레이 시험 3개가 통과한다(방언 둘: JSON 바인딩, `FOR UPDATE OF` 없음). HTTP `/jobs/*`와 job 실행(worker·effect journal)은 연결하지 않았다. 표와 한계는 [MySQL adapter 계약](porting/MYSQL-ADAPTER-CONTRACT.md)의 "작업 저장소" 절이다. 독립 검증 전이므로 완료로 표시하지 않는다.

## 2026-10-18 WBS 8.5 보존 정리와 S3 owner quota (Claude 구현, 독립 검증 대기)

S3 서명 업로드를 owner별 byte·개수 상한에 예약하는 장부와 만료 예약 정리(저장소 `HEAD` 확인 뒤 확정·해제·multipart abort), outbox 보존 기간 설정(`retention`)과 handler 없는 topic 정리, events 나이 기준 정리를 구현했다. 실제 TLS MinIO와 PostgreSQL 17에서 실행했고 AWS·GCS 실계정은 미실행이다. 기존 문서의 outbox 미정리 서술은 O-20 이전 상태여서 고쳤다. 실행 결과와 한계는 [제품 검증](../product/VERIFICATION.md)의 WBS 8.5를 따른다. 독립 검증 전이므로 완료로 표시하지 않는다.

## 2026-10-18 WBS 8.11 8.9 후속 (Claude 구현, 독립 검증 대기)

spike-v3·v4 테스트의 사전 clippy 실패 3건을 정리해 세 spike의 `clippy --all-targets -D warnings`가 모두 통과한다. `Kind::Io`는 호출자의 COMMIT 호출에서 재현할 수 없어 접속 단계의 실제 Io 오류로 판정을 고정했다. 실행 결과는 [제품 검증](../product/VERIFICATION.md)의 WBS 8.11을 따른다. 독립 검증 전이므로 완료로 표시하지 않는다.

## 2026-10-18 WBS 7.2.1.1 MySQL 읽기 경로 (Claude 구현, 독립 검증 대기)

PMS에서 review였던 7.2.1.1은 `aip-migrate` 드라이버와 DDL 펜스만 있었고 서비스 경로가 없었다. 이번에 `aip service adopt-external`·`principal`·`serve`와 `/read`·`/ready`를 실제 MySQL 8.4에 연결했다. 읽기 계획기에 `Dialect`를 넣었고 PostgreSQL 출력은 바이트 단위로 같다. 원시 카탈로그 행을 펜스 지문에 넣기 전에는 VARCHAR 길이·collation·생성식·인덱스 속성·CHECK·스토리지 엔진 변경 열두 가지를 지문이 보지 못했고, ENUM·DATETIME·DECIMAL 열이나 view가 있는 schema에서는 지문 계산 자체가 실패했다. 셋 다 실DB 시험으로 재현한 뒤 고쳤다. 트리거는 최소 권한 계정에 보이지 않아 감지하지 못하는 한계로 남기고 시험으로 고정했다. MariaDB 서빙·채택은 거부한다(LATERAL 부재). `/apply`·작업·파일·rate limit 등은 7.2.1.2·7.2.1.3 대상이며 명시 거절한다. 실행 명령과 결과, 의미 차이(collation 등)는 [MySQL adapter 계약](porting/MYSQL-ADAPTER-CONTRACT.md)의 "읽기 경로" 절을 따른다. 독립 검증 전이므로 완료로 표시하지 않는다.

## 2026-10-18 WBS 7.2.1.2 MySQL 쓰기 경로 (Claude 구현, 독립 검증 대기)

`/apply`(전이·묶음·create)와 `/status`를 MySQL 8.4에 연결했다. PostgreSQL의 `RETURNING`·`xmin`·advisory lock이 없어 `FOR UPDATE` 판정, 매칭 행 수 확인, 엔진이 기록하는 touched 행, `aip_tx_locks` bucket 잠금, 같은 트랜잭션의 멱등 journal과 감사로 같은 계약을 다시 만들었다. 실 MySQL 8.4.11에서 원자성, 멱등 재생, 동시 용량·증가·같은 키·expect 경쟁, UUID 리소스, 부재 정책의 SERIALIZABLE 이중 예약 방지, COMMIT 전후 연결 단절 세 경우를 확인했고, 보호 장치를 하나씩 끄면 시험이 실패하는 것도 확인했다(판정 `FOR UPDATE`만은 `UPDATE`가 같은 보호를 해서 결과가 같다). `compose`·확장 쓰기·작업·outbox 전달은 명시 거절하고 7.2.1.3에 남긴다. 자세한 표와 한계는 [MySQL adapter 계약](porting/MYSQL-ADAPTER-CONTRACT.md)의 "쓰기 경로" 절이다. 독립 검증 전이므로 완료로 표시하지 않는다.

## 2026-10-18 WBS 8.9 후속: 나머지 COMMIT 경로와 spike-v6 사전 실패 (Claude 구현, 독립 검증 대기)

분류 규칙(COMMIT 전 단절은 `WIRE.DB_UNAVAILABLE`, 전송 뒤 단절·해석 불가 응답은 `WIRE.COMMIT_UNKNOWN`, 서버 거절은 기존 코드)을 `spike-v3-write`의 `apply`·`bundle`·`compose`, `spike-v4-worker`의 `invoke_write`, `write_extension.rs`, `durable.rs`에 적용했다. 세 spike는 `aip-service`·`aip-auth`가 path dependency로 쓰는 제품 경로라 문서화에 그치지 않고 고쳤다. 판정과 COMMIT 전송은 `spike-v3-write`의 공통 함수(`connection_failed`, `send_commit`, `before_commit_rejection`)로 묶었고 v4·v6 `/apply`가 같은 함수를 쓴다. 경로마다 장애 지점별 재현 테스트를 붙였고, 구현 전 red(v3 14개 중 12, v4 9개 중 8, v6 `write_extension` 10개 중 8, job의 COMMIT 손실 3개 모두 실패)와 구현 뒤 green, 분류를 되돌리는 변이 20종이 모두 실패하는 것을 확인했다. 변경 중 제품 동작이 바뀌는 곳은 managed effect journal의 COMMIT이다. 응답을 잃은 job은 확장 오류 대신 `WIRE.COMMIT_UNKNOWN`으로 끝나고, COMMIT 전에 연결을 잃은 job은 `WIRE.DB_UNAVAILABLE`로 재시도된다.

spike-v6 사전 실패는 셋 다 원인을 찾았다. `write_isolation`은 제품 결함이 아니라 `aip-test-support::database_url_with_option`이 URI 옵션의 공백을 `+`로 써서 PostgreSQL이 접속을 거부한 테스트 지원 결함이고(같은 함수를 쓰는 `aip-migrate`의 `migration_history` 2개도 main에서 실패했다), 고쳤다. pool 30ms 테스트는 첫 checkout의 연결 시간이 같은 기한을 쓰는 불안정 테스트라 연결을 미리 idle에 넣어 결정적으로 바꿨다. v12~v16의 `tsc` 부재는 새 worktree에 추적되지 않는 `node_modules`가 없는 환경 문제이며, 전역 설치 없이 저장소 경로(`npm ci --prefix spikes/spike-0-ts`)로 설치하자 5개가 통과했다. 실행 명령과 결과, 남은 미검증 항목은 [제품 검증](../product/VERIFICATION.md)의 "WBS 8.9 나머지 COMMIT 경로와 spike-v6 사전 실패"를 따른다. 독립 검증 전이므로 완료로 표시하지 않는다.

## 2026-10-18 WBS 8.9 분류 보강 (Claude 구현, 독립 검증 대기)

8.9가 연결이 닫힌 경우만 `COMMIT_UNKNOWN`으로 분류하던 범위를 실제 재현 테스트로 넓혀 확인했다. COMMIT 전송 전에 연결을 잃으면 `WIRE.DB_UNAVAILABLE`(커밋 없음, 같은 키로 안전한 재시도), COMMIT 전송 뒤의 응답 손실·해석 불가·예상 밖 응답은 `WIRE.COMMIT_UNKNOWN`이다. 서버가 거절한 COMMIT(제약 위반, 직렬화 실패, 교착)은 확정 코드를 유지한다. `commit_unknown_wire` 11개(기존 1개와 신규 10개)가 red 5 failed에서 green 11 passed가 되었고 분류를 되돌리는 변이 4종이 모두 실패했다. spike-v6 clippy `-D warnings`와 `cargo fmt --all --check`는 통과했고, spike-v6 전체 스위트의 실패 7개 타깃은 수정 전 main에서도 같다. 실행 명령과 미검증 항목(`Kind::Io` 직접 재현 불가, 다른 COMMIT 경로 미실행 등)은 [제품 검증](../product/VERIFICATION.md)의 WBS 8.9 분류 보강을 따른다. 독립 검증 전이므로 완료로 표시하지 않는다.

## 2026-10-17 WBS 8.x 통합 (Claude 구현·통합, GPT 독립 검증 미실시)

Codex 작업 세션이 취소된 7개 브랜치를 이어받아 검증하고 main에 통합했다. 병합된 것은 `codex/db-test-isolation-20261017`, `fix/wbs-8.1-serializable-concurrency`, `fix/wire-commit-unknown-20261017`(8.9), `codex/wbs-8-6-cli-lint-cleanup-20261017`(8.6), `feature/wbs-8.4-python-sdk`(8.4)이고, `feature/wbs-8.2-8.3-recovery-20261017`은 test tooling만 반영했다. `fix/otlp-status-code-standard`는 코드가 이미 main에 있어 병합하지 않았다. 브랜치별 실행 명령과 결과는 [제품 검증](../product/VERIFICATION.md)의 WBS 8.x 통합 회차를 따른다.

이 통합 Run에서 같은 환경(이 Run 전용 PostgreSQL, 컨테이너 IdP·Caddy)의 IdP·TLS smoke는 원래 설정(workers 포함)으로 17 passed, 0 failed였다. 이는 구현자 자체 실행이며 독립 검증은 아직 없다. 기존 독립 교차 검증의 FAILED(3/17) 기록은 그 환경의 사실로 유지한다. 8.2·8.3·8.4·8.6·8.9와 DB 가드는 모두 독립 검증 전이므로 완료로 표시하지 않고 "통합됨, 독립 검증 대기"로 둔다. DB 가드가 불완전해 공유 PostgreSQL(5432)에 의도치 않게 접속한 사고가 있었고 이 문서의 해당 절에 기록했다.

## 2026-10-17 포팅 진행 — UI 환경 갭과 MySQL adapter 경계

댓글 UI 7.1.3.2.2는 아직 실행하지 못했다. 설정된 Ttakkari browser가 loopback `23173`에 연결할 때 `ERR_CONNECTION_REFUSED`를 반환했고, 지정 browser preflight는 `status=blocked`, `browser_launched=false`와 Mach IPC 거부를 기록했다. 추가 브라우저 실행 경로로 우회하지 않았다. 외부 YouTube 모달이 원인일 수 있다는 설명은 미검증이며, 이를 AIP 제품 갭으로 분류하지 않는다. 다음 허용된 browser 실행에서만 iframe/API stub 또는 화면에 보이는 닫기 동작을 고정 절차로 추가한다. 상세 범위는 [브라우저 기준선](porting/BROWSER-BASELINE.md#2026-10-17-댓글-ui-실행-환경-갭)을 따른다.

AIP MySQL/MariaDB adapter의 실제 연결·catalog/read/write 검증은 여전히 미실행이다. `aip-migrate`의 PostgreSQL `tokio_postgres` 경로에 붙이지 않고, 허용된 기존 의존성으로 MySQL `INFORMATION_SCHEMA` 입력을 중립 manifest로 바꾸는 순수 함수·타입 계층을 추가했다. 합성 행 회귀는 PK/index/FK 순서·unsigned·enum·tinyint(1) 표시 폭·nullable/default/generated와 FK 대상·referential action을 보존하고, 중복/누락 metadata·미지원 action은 거부하며 varchar 길이·NULL default 모호성과 collation·생성식·on-update 등 표현 밖 의미를 진단한다. 원본 11개 RDB 모델의 선언 기준은 [FK 기준 manifest](porting/facereview-fk-manifest.json)다. 기존 댓글 PostgreSQL fixture는 Comment의 두 참조만 담으며 전체 모델 기준선은 아니다. 이는 DB 관측이나 MySQL adapter 지원이 아니다. MySQL/MariaDB URL의 `DB_ADAPTER_UNSUPPORTED` guard는 유지한다. 실제 driver·catalog query 연결과 MySQL 8.4 실 DB 검증, 이후 MariaDB 별도 실행 및 read/write는 남아 있으며 WBS 7.2.1 전체는 미완료다. 인증·권한 포팅은 그 뒤다. 코드 revision: `28bb463f4bb73f461ff813ec8da3f57ad726e165`.

## Infisical 후보 조사 및 Memoir PostgreSQL 작은 업무 적용 (2026-10-17)

공식 공개 저장소 `Infisical/infisical`의 고정판 `188cf58173019bf0fe59972ed1c7ef715bec979c`를 코드 검토했다. root 코드는 MIT Expat이고 `ee/`는 별도 상용 라이선스다. 프로젝트 편집 권한은 normalized Membership→MembershipRole 관계와 역할 평가를 거치며, 현재 DSL에 없는 관계 집합 권한 조회를 요구했다. 이를 한두 개 actor 참조로 축약하면 원본 권한 모델과 달라져 이 단위는 채택하지 않았다. Infisical 원본 서비스·운영 API·DB는 실행/접속하지 않았고 home-server secret/credential/settings도 읽지 않았다. 따라서 Infisical 적용·실DB 검증은 없다.

기록상 기존 PostgreSQL 후보 `port-trials/memoir-notes`에서 기존 실행 예제의 Note create/favorite/delete와 겹치지 않는 Folder rename을 선택했다. 고정 비교 commit `26396070917ee065b988b0de296baf54f6266322`의 `FolderService`, `FolderRepository`, entity와 request/controller를 기계적으로 대조한 범위 및 제한은 [시험 기록](../product/tests/fixtures/memoir-folder-rename/README.md)에 있다. 원본 worktree는 36개 사용자 변경이 있어 보존했고 HEAD의 해당 파일만 읽었다. 별도 전용 PostgreSQL DB에서 synthetic source-shaped row와 AIP resource를 채우고 서비스·설치형 TS SDK로 owner read/rename, unrelated actor의 빈 조회/거부, 거부 뒤 DB 무변경을 실행했다. 결과는 **코드 기준 적용 / 실DB AIP 검증**이다. Memoir 원본 서비스/API/DB는 실행하지 않아 원본 E2E 동등성은 주장하지 않는다.

실행 출력, 실제 저장값, 제한과 제외 범위는 [제품 검증](../product/VERIFICATION.md#memoir-folder-rename-실db-aip-검증-2026-10-17)을 따른다. 이 작은 시험은 Memoir 전체 포팅, Infisical 적용 또는 인증/권한 WBS 완료가 아니다. FaceReview WBS 7.1/7.2 gate, Infisical 전체 포팅 WBS 7.3, MySQL adapter gate는 그대로 미완료다.

## 페이스리뷰 원본 포팅 기준선 후속 (2026-10-17)

고정 원본 backend에서 허용된 로컬 Origin의 Socket.IO polling 연결, 빈 `watch_frame` 입력 거부, 연결 종료를 실제 확인했다.
이는 WBS 7.1.3.3.1의 부분 원본 기준선이며 AIP 포팅·실제 프레임·WebSocket·ML·worker/provider·통합 CI 완료를 뜻하지 않는다.
댓글 UI는 고정 YouTube 영상의 실제 제공자 오류 모달이 상호작용을 막아 열어 두었다. seed·player 동작을 바꿔 통과시키지 않았다.
API의 MySQL/Mongo/Redis 실제 서비스와 23 API case 기준선은 계속 재사용한다. 전체 범위와 다음 단계는 [포팅 README](porting/README.md)를 따른다.

> 현재 기준: [2026-10-09 백엔드 기능 포괄성 지침](sources/founder-backend-capability-directive-2026-10-09.md)과 기존 [통합 지침](sources/founder-integrated-directive-2026-10-03.md). 충돌하면 최신 원문의 범위·표식을 따른다.

## 외부 IdP·TLS reverse proxy 및 실제 broker·storage smoke 검증 (2026-10-17)

WBS 항목인 외부 IdP/TLS reverse proxy 운영 smoke 및 실제 Kafka/RabbitMQ/MinIO(S3 호환) 연동 smoke의 로컬 컨테이너 기반 검증 기록이다. CA 코드·문서 보완은 독립 검증 PASS다. IdP 전체 smoke는 독립 교차 검증에서 FAILED(3/17 실행, 나머지 14개 미실행)였고, 이후 통합 Run에서 원래 설정으로 17 passed였다. 후자는 자체 실행이라 WBS A는 독립 검증 전까지 미완료로 둔다.
- **외부 IdP 및 TLS reverse proxy**: Caddy TLS proxy 환경에서 자체 경량 RSA/JWKS/JWT fixture(`conformance/idp-proxy/idp/server.mjs`) 기반으로 검증했다. Keycloak·상용 IdP 연동 및 RFC 9068 전체 conformance 적합성은 미검증이다. 원 작업(`cbaa401`)에서는 17개 검사 통과를 보고했으나, 독립 교차 검증(`cbaa401`, 별도 DB)에서는 3개 assertion(health/discovery/JWKS) 통과 후 `aip service` 시작 단계에서 `SERVER_CONFIG`로 중단(exit 1)되었다. 후속 진단에서 실패 stage는 `worker_runtime_probe`로 확인되었다(macOS `isolation=auto` 환경의 `node --version` exit 71 / `permission_or_sandbox_denial`; nested Mac Run의 환경 제약이며 정확한 syscall/규칙 불명, production 일반화 금지). 프록시·CORS·request ID·JWT 검증 등 남은 14개 assertion은 여전히 미실행 상태다.
- **실제 Kafka·RabbitMQ 및 MinIO**: `scripts/smoke-brokers-storage.sh`의 세 cargo 명령군(`brokers_live` 2 passed, `aip-service` recovery 1 passed, `cloud_storage` 1 passed) 기반 총 4개 live 통합 검사(`real_kafka_confirm_and_import_round_trip`, `real_rabbitmq_confirm_and_import_round_trip`, `broker_worker_recovers_from_database_disconnect_without_losing_delivery_budget`, `minio_https_actual_object_and_multipart_contract`)를 통과했다(필터된 0개 타깃은 제외). MinIO 소스 빌드는 독립 검증 시 기존 로컬 이미지 재사용으로 미실행되었으며 공식 이미지 중단 단정은 제외했다.
- 상세 실행 결과 및 회귀 수정은 [제품 검증](../product/VERIFICATION.md)을 따른다. (작업 기준 부모 커밋: `4e8e2126a91bd15e21afd870fe5373c0ccba9adb`).

## 내부 metrics·OTLP exporter·durable job trace 연결 (2026-10-17)

WBS 항목인 내부 metrics 확장, OTLP/HTTP exporter, durable job run trace 연결을 구현했다.
1. 내부 metrics: HTTP 요청(`aip_requests_total`, `aip_request_duration_seconds`), operation(`aip_operations_total`, `aip_operation_duration_seconds`), durable job(`aip_jobs_total`, `aip_job_duration_seconds`)의 카운터 및 지연 histogram(5ms~30s)을 계측하고 AIP 오류 코드별 실패를 분류한다.
2. OTLP exporter: `OtlpConfig`로 설정하며 기본값은 off다. 비활성화 시 `Option` 검사만 수행되어 추가 오버헤드가 없다. 활성화 시 bounded mpsc queue(4096) 기반의 백그라운드 워커가 `/v1/traces`와 `/v1/metrics`로 OTLP JSON 규격 배치 전송을 수행한다.
3. Durable job trace 연결: HTTP 인그레스의 W3C `traceparent`에서 유입된 `trace_id`를 PostgreSQL `aip_jobs.trace_id`에 저장하여 재시도·attempt 간 보존한다. job 실행 시 해당 trace_id 컨텍스트를 복원하여 HTTP 요청 span과 durable job run span이 동일한 trace로 묶인다. 민감값(인증 토큰, 요청 body/query payload)은 span 속성에서 제외한다.
검증: mock in-process OTLP 수신기를 통한 통합 테스트(`tests/otlp.rs`), `aip-execution`, `aip-service`, `cargo fmt`, `cargo clippy`, `cargo deny` 전체 통과.

2026-10-17 후속 수정: commit `4e8e2126a91bd15e21afd870fe5373c0ccba9adb`가 `aip_jobs.trace_id`를 fresh DDL에만 추가해 이미 설치된 job schema를 자동 업그레이드하지 못했다. 기존 marker의 DDL digest가 현재 생성 DDL과 다르면 planner가 `derive()` 전 `WIRE.SCHEMA_MISMATCH`로 거부했고, 검증 전 Store enqueue/claim은 PostgreSQL `42703`을 냈다. 검증된 구형 digest 하나만 catalog marker와 함께 인식해 `aip service migrate` 계획에 nullable `trace_id` 추가를 넣고, 기존 migration transaction/journal 경로에서 적용하도록 수정했다. 임의 drift, 알 수 없는 digest, 이전 digest와 맞지 않는 실제 컬럼 구조는 계속 거부한다. 전용 PostgreSQL에서 `aip service init`과 실제 `aip service migrate` plan/apply/replay 명령을 거친 회귀는 기존 NULL job claim, 신규 enqueue/claim, idempotency replay, retry trace 보존, actor/principal/idempotency/facts/wire metadata 보존과 drift negative case를 확인했다. 독립 검증은 고정 source SHA `9bc78de3654c163e29f3cb369ec21ff496499e85`의 DB upgrade 범위에서 **PASS**이며 `verification_outcome=passed`로 기록한다. 이는 이 DB upgrade 범위의 판정이고, 전체 WBS·제품 출시·worker E2E 완료를 뜻하지 않는다. main 반영·production 배포 상태는 이 기록과 별도다.

OTLP 표준 status code 패치(`661024d6d23629b2eb276105eaba22144f0ff401`)의 코드와 테스트만 latest main에 통합했다. 독립 Collector 검증은 HTTP 200→`Ok`, 405→`Error`, job `SUCCEEDED`→`Ok`, `FAILED`/`CANCELLED`→`Error`, `RETRY`→`Unset`을 확인했다. 이 검증은 synthetic Lease 기반 emitter까지이며 worker/DB lifecycle 검증은 아니다. 원 branch의 `STATUS`/`VERIFICATION` 문서는 가져오지 않아 IdP 전체 FAILED(3/17, 14 미실행) 및 CA 독립 PASS 정정을 유지한다. exporter failure, parent flag 보존 등 다른 관측성 개선은 미완료 범위에 남긴다.

## resource 독립 실행 고도화 (2026-10-10)

후속 요청으로 [동기 operation](../product/EXECUTION.md)을 제품 V1/V2/V4/V5/V6와 운영 신원·배포·설치 SDK에 연결했다. 업무 resource 0개 모드와 기존 actor resource 모드를 구분하며, allow·입출력 범위·기한·ctx 거부·현재 principal 재검사를 유지한다. 다음 [durable Job 제안](backend-capability/durable-job-proposal.md)은 실행권 세대·권한 회수·취소/완료 경합·결과 ACL·schema 업그레이드 접점까지 구체화했지만 아직 구현하지 않았다. 현재 실행 근거는 [제품 검증](../product/VERIFICATION.md)을 따른다.

## 현재 포괄성 감사 (2026-10-09)

[백엔드 기능 감사 A~F](backend-capability/README.md)는 SQL 밖 기능을 포함한 105개 최소 항목·여섯 시나리오·Claude Code/Codex 상호 검토를 기록한다. 제품 지원과 별도 PoC를 구분했고, 업무 SQL 없는 Node/Python 계산·DB 불가·선언 deadline·6초 동기 실행을 실제 실험했다. 제품/PoC 재실행 범위와 미검증은 [검증 기록](backend-capability/validation.md)에 있다. 이 감사는 분석·제안이며 새 실행 모델의 대규모 변경을 수행하지 않았다. 최종 DSL·쓰기 모델·Id wire 등 기존 Open도 선점하지 않는다.

직전 작은 보완은 Ref 대상 identity·nullable 생성 후보이며 `ecf3369`에 따로 커밋했다. 후속 작은 허점과 비SQL 실행 모델의 우선순위는 [로드맵](backend-capability/roadmap-and-risks.md)을 따른다. 아래 제품/실험 수치는 각 시점의 이력이다.

## 현재 제품 작업 (2026-10-05)

사용자의 “운영인증, 일반마이그레이션, 제품통합 다 해” 지시로 범위를 확장했다. 기존 PoC는 보존하고 root CLI의 `aip service`와 `aip-auth`, `aip-migrate`, `aip-service`를 추가했다. caller 정의·서버 권한·기존 실행 엔진·하나의 SDK를 재사용한다.

운영 JWT 검증과 지속 actor 연결, 일반 DDL 계획·적용·배포 저널, 데이터와 멱등 결과를 보존하는 명시적 prototype 이관, 설치 SDK를 제품 경로에 연결했다. 재귀 제한은 source·token·AST·predicate 확장·path·SQL 생성·복제 payload에 함께 적용한다. 활성 서버의 구 정책 실행, 연결 재활성화 시 폐기 토큰 복구, Id wire 변경에 따른 멱등 namespace 변화도 회귀 검증 대상으로 고정한다.

2026-10-06 최종 제품 검증은 [제품 결과](../product/VERIFICATION.md)와 [배포 묶음](../product/dist/2026-10-06T11-19-25-091Z/README.md)을 따른다. 현재 실행 절차·검증 명령·미지원 범위는 [제품 서비스](../crates/aip-service/README.md), [마이그레이션](../crates/aip-migrate/README.md), [SDK](../product/SDK.md)를 따른다. 자체 fixture 기반 IdP·TLS proxy 및 실제 broker/storage smoke는 2026-10-17 로컬 컨테이너 기반으로 실행됐다. CA 코드·문서 보완은 독립 검증 PASS이나 IdP 전체 smoke는 독립 교차 검증에서 FAILED(worker runtime probe 이후 14개 assertion 미실행)였고, 통합 Run의 자체 실행은 17 passed다. 독립 재검증은 남아 있다. 다른 OS·무중단 대형 backfill·클라우드 상용 IdP/외부 서비스 직접 연동 및 RFC 9068 전체 conformance는 미검증이다. 최종 DSL·Id wire·쓰기 조합·라이선스 결정은 유지한다.

## 이전 단계: 2026-10-04 밤 작업 요약

설계 문서를 바탕으로 **작은 검증 실험 V1부터 V15까지 실행했다.** 본 `crates/`는 건드리지 않았고(통합 지침 §16·17), 실험은 `spikes/`의 독립 Rust crate에 있다. V1부터 V7까지는 기존 Codex 6.1-sol 검토 이력이 있고, V8·V9·V10·V11·V12·V13 후속 작업은 Codex 메인과 Luna high·Sol high가 검증했다. V14는 메인이 테스트·구현·회귀를 수행하고 Sol high가 독립 검토했다. Luna는 V14에서 사용 한도로 참여하지 못했다. V15에서는 다시 참여해 생성기 테스트·범위 대조를 맡았고, 메인이 구현·HTTP/DB·회귀를, Sol high가 설계·독립 검증을 맡았다. 찾은 문제는 재발 테스트로 고정했다.

1. **문법(V1)**: 독립 `.aip`, TS/Python 안 선언 블록, TS/Python 정형 데이터 5가지 작성 방식이 같은 의미(typed facts)로 바뀐다. 잘못된 기호·동적 문자열·주석 속 가짜 선언은 거부한다. 형식을 고르는 근거(작성 경험)는 아직 측정하지 않았다.
2. **호출자 읽기(V2)**: 화면이 고른 필드·관계·필터·정렬을 서버 정책 아래 매개변수 SQL 하나로 실행했다. 열린 필드는 요청만 바꾸면 되고, 닫힌 필드는 서버 정의 변경이 필요하다는 경계가 실제로 동작한다.
3. **쓰기(V3)**: 알림 읽음·모집 마감 같은 단순 쓰기는 표준 전이 하나로 원자성·동시성까지 처리됐다. 일괄 승인과 관리자 위임에서는 **서버 정의 전이(W2)가 순서까지 서버 안에 두고 업무 불변식을 지켰다.** 공개 동작 묶음(W0)과 호출자 조합(W1)은 커밋 시점 불변식·사후조건이 있어야 안전했고, W1은 업무마다 새 조합 개념(값 출처, 자기 행)이 필요했다. 어느 쪽을 표준으로 둘지는 아직 정하지 않는다.
4. **확장(V4)**: Node·Python 확장이 서버가 묶어 준 권한 안에서만 데이터를 읽고, 쓰기 확장은 서버 트랜잭션 안에서 공개 전이만 불러 실패·기한 초과 시 전부 되돌려졌다. 다만 **worker를 별도 프로세스로 띄우는 것만으로는 확장이 DB에 직접 붙는 것을 막지 못했다.** macOS에서는 네트워크 차단으로 막혔지만 운영 환경용 격리 방식은 아직 정하지 않았다. 후속 프로토타입 준비에서 공통 scalar·bounded stdout/stderr·Result 시작·읽기 Grant 정리를 연결했고 Rust23개와 실제 Node/Python 초과 쓰기 롤백·외부 취소·반복 호출, 음성 대조7종을 검증했다. 후속 READ 연결은 SafeNumber/DecimalString을 ctx와 입출력에 적용하고 생성 SDK·HTTP·명시적 MacNetDeny worker로8개 실제 흐름을 실행했다. 당시 V4 전체25개 실패0이며 후속 bootstrap과 WRITE 입력 전송 기한 보완은 프로토타입 최신 기록을 따른다. 후속 WRITE HTTP/생성 SDK는 같은 공개 전이·멱등 저장소·원자적 결과 저장을 사용하며 [prototype 최신 결과](../prototype/README.md#write-공식-확장의-실행-결과)를 따른다. 운영 격리는 남는다. 외부 효과(알림)는 outbox로 "커밋된 것만 전달 시도, 실패 재시도 뒤 격리, 멱등 키로 중복 효과 억제"를 확인했다.
5. **프론트 SDK(V5)**: 화면이 고른 필드에서 결과 타입이 추론되고 계약 밖 요청은 컴파일 오류가 된다. 캐시는 정책이 읽는 권한 테이블을 포함한 읽기 의존 resource로 무효화하며, 같은 연결의 쓰기·계정 전환·결과 미확정·늦은 응답을 처리한다. 다른 연결의 변경·권한 회수는 V9의 수명 만료 뒤 재조회까지 지연될 수 있다.
6. **전송(V6)**: 최소 HTTP 서버와 TS 클라이언트를 연결했다. 쓰기 응답이 유실돼도 같은 멱등 키로 다시 보내 결과를 확정하고, 두 번 실행되지 않는다. 인증은 V8에서 서버 서명 토큰으로 붙였다(위조·변조·만료·다른 키 거부, 실패 토큰은 익명으로 낮추지 않음).
7. **마이그레이션(V7)**: 두 계약 버전의 변경을 안전·호출자 영향·보안 검토·파괴적·데이터 위반으로 나누고, 행 정책 변경은 실제 회원별로 보이는 행을 측정해 검토 근거로 붙인다(측정만으로 안전을 확정하지 않음).
8. **수명·복구(V9)**: 캐시는 검증한 세션 수명과 실험용 신선도 상한 안에서만 재사용한다. 시간 의존 조회는 현재 시각으로 실행하고 미캐시한다. 같은 멱등 키의 반복·동시 호출, 인증 만료 뒤 미확정 쓰기 보존, 잘못된 성공 응답, 헤더 크기 상한을 반례로 검증했다. [실행 결과·한계](alignment/V9-cache-lifetime-results.md).
9. **세션 갱신(V10)**: 첫 read/apply 전에 서버 principal을 확인한다. 같은 actor의 새 토큰으로 pending을 자동 복구하고 다른 actor 교체는 원래 정보를 유지한 채 거부한다. 캐시와 진행 중 요청을 인증 세대로 분리하고 signed -1과 익명 멱등 scope를 분리했다. 실제 토큰 만료·승인 응답 유실 뒤 replay와 DB 단일 효과를 확인했다. [실행 결과·한계](alignment/V10-session-recovery-results.md).
10. **선택적 검사(V11)**: 개발 조언 off/on을 실행 계약과 분리했다. docs 없음·변경·소속 이동이 실행 facts·생성 TS·DB 권한을 바꾸지 않는지 대조했고, 조언이 있어도 정상 실행을 막지 않는다. raw 요청의 타입·권한·비용 검사는 필수다. [실행 결과·한계](alignment/V11-optional-checks-results.md).

11. **계약 타입 연결(V12)**: 앱별 생성 binding만 전달하면 실제 HTTP 결과의 선택형 타입을 얻는다. 계약 식별값과 성공 envelope를 캐시 저장 전에 검사하고, 구세대 오류·미확정 쓰기 경계를 유지한다. typed facade의 캐시 주입 우회는 닫았다. 큰 Id 표현·typed apply·전체 행 decoder는 남는다. [실행 결과·한계](alignment/V12-typed-transport-results.md).

12. **큰 Id 왕복(V13)**: Legacy 반올림으로 이웃 행을 변경하는 실제 DB 반례를 재현했다. 숫자 안전범위 제한과 십진 문자열 후보는 서버 공통 경계에서 처리하며, 계약 불일치는 첫 쓰기도 DB 전에 거부한다. 공개 Id 표현은 아직 선택하지 않았다. [실행 결과·한계](alignment/V13-id-boundary-results.md).

13. **공개 쓰기 타입(V14)**: 같은 binding에서 공개 전이 입력과 readonly 결과가 추론된다. 두 Id 후보·읽기 없는 쓰기·응답 유실/손상 복구·실제 결과 동결을 검증했다. 공개 쓰기 변경도 지문에 포함하며 서버 최종 검사는 유지한다. V14 당시 미지원 scalar where는 후속 V15에서 연결했다. 조합/worker 타입·최종 API는 남는다. [실행 결과·한계](alignment/V14-typed-apply-results.md).

14. **공개 필터 실행(V15)**: Url·Enum·Time read/where 공통 검사와 Text.prefix를 실제 PG/HTTP에서 대조했다. 날짜·offset·소수초 반올림·NUL 선거부·타입/지문·권한을 확인했다. [실행 결과·한계](alignment/V15-filter-value-results.md).

15. **실행 프로토타입 연결**: [check/gen/init/serve CLI·단일 SDK](../prototype/README.md)를 기존 엔진으로 연결했다. CLI22개와 데이터 보존·전체 DDL 롤백·DB 설정·종료·세 음성 대조를 검증했다. safe/decimal 각각 실제 생성 SDK→서버→공개 정의 변경→재생성·재기동에서 구 binding 첫 쓰기 거부와 새 binding 실행, 기존 데이터·멱등 결과 보존을 확인했다. Sol은 소스 SDK decimal 흐름을 독립 재실행했다. 로컬 private SDK tarball은 새 소비자의 offline 설치·일반 JS 실행·strict TS 검사와 출력 보호를4개 테스트로 검증했고, source/package × decimal/safe4개 실제 서버 흐름을 실행했다. 새 Luna가 패키지4개·설치 SDK 두 wire를 독립 재실행했다. 공식 READ 확장도 단일 생성 binding/SDK에서 source/package × safe/decimal × Node/Python8개 조합을 실행했다. 당시 SDK 경계9개·Rust88개와 실제 child 수명/동시4개 상한·실행 guard8종 음성 대조를 검증했다. 후속 HTTP Origin/파싱5초/연결64개·캐시 전 표준 read 검사·관계 대상 집계·내장 worker를 보완했다. WRITE까지 생성 계약·HTTP·SDK에 연결해 실제8개 source/package/wire/lang 조합·동시 같은 key 단일 실행·pre-commit rollback·실제 COMMIT_UNKNOWN 뒤 재생을 확인했다. 정본 Event.confirm과 bundle의 두 언어 구현을 제공하며 CLI27개·SDK/패키지37개·builder5개·Chrome8개·재배치 bundle8개를 검증했다. CPU loop의 동시4개/5번째 거부·기한·child/row lock 반환과 상한/key lock 음성 대조도 확인했다. DB connect/clock 각각5초·owned driver·SQL operation deadline·init/serve SQL5초와 실제 catalog 구조 대조까지 보완했다. 실제 COMMIT 응답 손실과 다음 retry BEGIN 정지에도 pending/cache unknown을 보존하고 같은 key 재생으로 복구했다. 실행 중 만료된 세션이 이미 커밋한 WRITE의 성공을 실패로 바꾸는 반례도 Node/Python·두 wire에서 수정·검증했다. 정의 숫자 overflow·중첩 abort·캐시의 WRITE 완료 간격·닫힌 stdout/stderr도 보완했다. 최신 Rust175개(V1 포함)·SDK47개·builder5개·WRITE8개·Chrome8개·재배치 bundle8개는 실패0이다. [DB 후속 결과](../prototype/README.md#정의-입력캐시-완료cli-출력-후속-결과)의 marker 호환·검증 범위를 따른다. 최신 명령·범위는 prototype 후속 결과를 따른다. 이 항목은 제품 통합 이전 host의 private 실험 이력이다. 이후 운영 인증·일반 migration·제품 조립의 현재 결과는 위 현재 제품 작업을 따른다. 최종 WRITE API/조합은 남는다.

아리아리 원본과 다르게 동작한 점(코드 읽기 + 실험 비교, 원본은 실행 확인 안 함): 일괄 승인에서 없는 지원 id를 조용히 빼고 나머지만 승인하는 것으로 보임, 권한보다 동아리 검사가 먼저, 회원 중복을 막는 DB 제약 없음, 자기 자신에게 관리자 위임 시 관리자가 사라질 것으로 보임.

**지금 답이 필요한 질문은 없다.** 아래는 기술 비교가 더 진행되면 창시자에게 갈 수 있는 선택이다(아직 질문 아님).
- 권한 때문에 숨긴 필드를 null로 줄지, 빼고 줄지, 요청을 거부할지(V2-R1)
- 쓰기 결과가 불확실할 때(커밋 중 시간 초과) 호출자 경험: 재시도·상태 조회·멱등 키(V3-R6)
- 초기 표준 쓰기에 어느 정도의 호출자 조합 자유를 줄지(V3 §6·§7)
- 확장의 외부 네트워크 허용 범위와 운영 환경 격리 수준(V4 §3)
- Id 공개 표현: 큰 정수 Id를 문자열로 주고받을지, 숫자로 두고 범위를 제한할지(V5 §7 R5-05)
- 화면 데이터의 신선도: 다른 사용자의 변경을 얼마나 빨리 반영할지, 마감 시각처럼 시간이 지나면 바뀌는 조회를 얼마나 낡게 허용할지(V5 §6·§7)

## 현재 상태

요청된 A~E 설계 산출물을 작성했다. 창시자 목적과 기술 상세 승인은 분리했고, 기존 결정 문서의 확인 대기를 답변/검증 상태로 갱신했다. 코드·신규 문법 실행 검증은 이 문서 작업에 포함하지 않았다.

| 읽을 곳 | 현재 내용 |
|---|---|
| [A](alignment/A-founder-intent-matrix.md) | 최초 7원칙·각 FOUNDER 구절의 실제 code/doc 대응·누락 |
| [B](alignment/B-decision-reclassification.md) | 25개 질문·Q0~Q9·14개 의제 재분류, DD별 목적/상세 상태 정본 |
| [C](alignment/C-syntax-proposal.md) | 서버 A/E/H·프론트 read/apply·조합/확장/metadata 후보 |
| [D](alignment/D-development-experience.md) | 아리아리 실제 소스와 10개 변경 시나리오 비교. 절감률/성능 수치 미측정 |
| [E](alignment/E-technical-risks.md) | RK-01~10 반례·초기 실험 순서·V1부터 V15까지 실행 범위와 남은 항목 |
| [독립 대조](reviews/INTEGRATED-codex-luna-r1.md) | Luna high의 의미 누락 3건과 메인 확인/문법 반영 |
| [후속 판단](90-open-questions.md) | 비교 후 남을 출시/DX/호환/라이선스 선택. 이미 답한 25개를 재질문하지 않음 |

확인된 기준: 호출자 읽기·단순 쓰기·풍부한 표준 동작, Rust 자체 포트 서버+공식 프론트 SDK, JS/TS·Python 공식 확장, 선택적 구조화 설명, 선택적 개발 검사와 필수 실행 안전 검사 분리, 오픈소스·외부 기여.

미정 또는 검증 대상: 독립 파일 여부·구체 문법/Typed IR·안전한 쓰기 조합·초기 언어/프론트/서드파티 범위·비용 제어·worker/ABI·DB·호환 기간·라이선스. DIRECTION의 운영 입장·승인·롤백·동기화·오류 정책을 확정으로 승격하지 않았다.

이번 협업: Claude Code는 토큰 소진으로 미참여. Codex 메인이 설계·구현·판정하고 Luna high가 조사·인증 검증·테스트를, Sol high가 설계·동시성·수명 경계 비평을 맡았다. 기존 Claude/Codex 합의는 이전 지침 검토 이력이다.

문서 구조 검증·미실행 범위는 [V0 검증 결과](alignment/V0-validation-results.md)에 남긴다.

### 2026-10-04 밤 자율 작업 (spike 실험, 본 crates 미변경)

| 실험 | 결과 | 독립 검토 |
|---|---|---|
| [V1 의미 fixture](alignment/V1-semantic-fixture-results.md) | `spikes/spike-v1-fixture/`. A·E-TS·E-Py·H-TS·H-Py 5형식이 같은 typed facts. 음성 대조 49건 + r1·r2 반례 + 손으로 쓴 기대 facts 대조. 최신 전체 재실행 17 passed | Codex 6.1-sol r1 "부분 성립"(P1 8건), r2 3건 → 모두 반영·재발 테스트 고정 |
| [V2 호출자 읽기 slice](alignment/V2-caller-read-results.md) | `spikes/spike-v2-read/`. facts만으로 매개변수 SQL 생성, 로컬 PG에서 행·필드·관계·집계 정책, 열린/닫힌 capability, filter/sort 별도 허용, depth/rows/cost/deadline·출력 상한 거부 확인. 5 passed | Codex r2b(실제 DB 재현) 12건 중 읽기 쪽 반영 |
| [V3 표준 쓰기](alignment/V3-standard-write-results.md) | `spikes/spike-v3-write/`. V3-1 알림 읽음·모집 마감(대상·상한·원자성·동시성·권한 의존 행 공유 잠금·기한). V3-2 일괄 승인을 W2(서버 정의 전이)·W0(공개 동작 묶음)·W1(서버 선언 범위 조합)로 같은 반례 표에 비교. 5 passed. V3-3 관리자 위임: 교대형 불변식에 커밋 시점 제약 필요, W0·W1은 순서 민감. W1은 compose에 selfRow를 더해 표현했고 반례 결과가 W0과 같음 | V3-1 Codex r2b, V3-2 Codex r3 반영, V3-3 r4b 발견 없음, W1 selfRow r6 반영 |
| [V4 공식 확장](alignment/V4-official-extension-results.md) | `spikes/spike-v4-worker/`. Rust 서버 + Node/Python worker. ctx 토큰(actor·허용 집계·입력 고정·기한), 출력 검사, 기한, 토큰 폐기, worker 장애. **발견: 환경 변수를 지워도 worker가 로컬 DB에 직접 연결 가능(격리 없음)**. V4-3 쓰기 확장: 서버 트랜잭션 안 공개 전이만, 실패·오류 삼킴·기한·늦은 ctx 모두 rollback, 커밋 결과 미확정 구분 | Codex r4(필터 중단)·r4b·r6 반영 |
| [V5 SDK](alignment/V5-sdk-results.md) | `spikes/spike-v5-sdk/`. facts에서 TS 계약 타입 생성, 화면 select에서 결과 행 타입 추론(관계 포함), 닫힌 필드·허용 밖 filter/sort·값 타입·루트 조회 불가 8가지를 컴파일 오류로. tsc 음성 대조 포함. V5-2 읽기 deps 건전성, V5-3 캐시 + V9 수명(node 10). V12 공개 읽기 식별값·V13 wire·V14 공개 쓰기 projection 대조 포함 최신 Rust 17 passed | V5-1·2 Codex r4b, V5-3 Codex r5 반영 |
| [V6 전송](alignment/V6-transport-results.md) | `spikes/spike-v6-transport/`. Rust 최소 HTTP 서버 + TS 클라이언트(V5 캐시). 쓰기 태그로 캐시 무효화, 응답 유실 시 같은 키 재요청으로 확정, 재실행 없음, 확정 전 캐시 저장 보류. node e2e 7 | Codex r7 반영 |
| [V7 마이그레이션](alignment/V7-migration-results.md) | `spikes/spike-v7-migrate/`. facts 두 버전 비교 → 변경 9종 분류, 행 정책 확대/축소를 회원별 보이는 행으로 측정, 불변식·enum·필수 필드·범위는 기존 데이터 위반 수를 적용 전에 계산. 분류하지 못한 차이는 모두 검토 | Codex r8 반영 |
| [V8 인증](alignment/V8-auth-results.md) | V6에 서버 서명 세션 토큰(HMAC, 기동 시 무작위 키, 만료). 시험용 actor 헤더 제거. 8사례 + 단위. V9에서 입력 크기와 캐시 만료 경계 추가 | Luna high 단위·서버 검증, 메인 반례·보강 |
| [V9 수명·복구](alignment/V9-cache-lifetime-results.md) | V5/V6 재사용. 당시 캐시·복구 Node 17, 실제 서버/DB Node 4. 세션 만료·시간 조회·다른 연결의 변경/권한 회수·같은 키 동시 호출·미확정 복구·입력 상한 | Sol high 설계·독립 비평, Luna high 인증 대조·테스트 작성 |
| [V10 세션 복구](alignment/V10-session-recovery-results.md) | 서버 `/session`, SDK `replaceSession`, 같은 actor pending 복구, 인증 세대 경합, signed -1/익명 분리. 최신 캐시/복구 단위 Node 27, 새 HTTP/DB Node 3 | Sol high 설계·최초 거부 반례, Luna high 테스트·문서 대조, 메인 실제 만료·회귀 검증 |
| [V11 선택적 검사](alignment/V11-optional-checks-results.md) | 기존 V1/V2/V5 재사용. 옵션/metadata/생성 TS/필수 거부 단위 8개·PG 24조합 1개. 고장 주입 3종 검출 | Luna high 단위 실패 대조, Sol high 설계·구현 리뷰, 메인 구현·PG·고장 주입 검증 |
| [V12 계약 타입 연결](alignment/V12-typed-transport-results.md) | V5 generic core·V6 typed facade. 생성 계약 식별값·실제 HTTP/PG·tsc 정상/음성·Node 11개. binding 누락/캐시 주입 반례 수정, 고장 주입 3종 검출 | Luna high 테스트·V5/V11 회귀, Sol high 설계·독립 우회 재현·수정 후 재검증 |
| [V13 큰 Id 왕복](alignment/V13-id-boundary-results.md) | Legacy 잘못된 행 재현·SafeNumber/DecimalString PG/HTTP·첫 apply 계약 거부·모드별 멱등·고장 주입 6종. V2/V3/V5/V6 전체 Rust 12·5·9·10개 실패 0 | Luna high 단위/회귀·범위 대조, Sol high 첫 apply 비평·guard Node 3개 독립 재검증 |
| [V14 공개 쓰기 타입](alignment/V14-typed-apply-results.md) | V5/V6 후보 binding·action/target/where 타입·두 Id mode·PG/HTTP Node 14개씩·형식 오류 pending·실제 결과 동결·고장 주입 6종. 전체 Rust V5 13·V6 11·V11 9개 실패 0 | 메인 테스트/구현/회귀, Sol high 초과 키·결과 변조 재현과 수정 후 독립 tsc/Node 재검증 |
| [V15 공개 필터 실행](alignment/V15-filter-value-results.md) | Url/Enum/Time read·where와 리터럴 prefix, 필수 값 검사·PG 반올림·NUL·타입/지문·권한·7종 고장 주입. 전체 Rust V2 19·V3 5·V5 17·V6 12·V11 9개 실패0 | Luna high 생성기 테스트·범위 검토, Sol high Time/PG·독립 검사, 메인 구현·HTTP/DB·전체 회귀 |

V3-2 관찰(검증 필요): 세 후보 모두 공통 반례(다른 동아리·권한 없음·누락 id·이미 회원·거절됨·동시 승인)를 같은 안전한 결과로 막았다. W0은 커밋 검사 없이는 지원하지 않은 회원 생성이 가능했고, W1은 허용 단계·값 출처·생성 allow(역할 제한 포함)·사후조건 두 개를 선언해야 W2와 같은 업무를 지켰다. 이 fixture의 선언 구성에서는 W1이 W2보다 서버 정의를 줄이지 못했다(11줄 vs 8줄). W1이 항상 불리하다는 근거는 아니며, 관리자 위임은 이후 V3-3에서 실행했다. 순환 전이와 다른 업무에서 조합의 장점이 유지되는지는 남은 비교다.

V13은 큰 Id의 두 후보를 실제 왕복으로 비교했다. 적용은 V6 HTTP read/apply와 후속 공식 READ worker이며 bundle/compose/WRITE worker·최종 표현 선택·일반 Int 정밀도는 남는다. V14는 direct 공개 전이의 생성 타입·응답 검사·쓰기 식별값을 실행했다. 후속 [V15 공개 필터 값 정합](alignment/V15-filter-value-results.md)은 Url/Enum/Time read/where·Text.prefix를 연결했다. [프로토타입](../prototype/README.md)은 초기화→생성 SDK→서버 호출→공개 정의 변경·재기동을 한 실행 흐름으로 검증했다. SDK tarball의 새 소비자 설치와 현재 macOS host의 재배치 bundle은 검증했으나 전체 제품 clean install, create/compose 값 타입·WRITE 확장 통합 등의 공백은 남는다. prototype 표준 read의 공개 선택 행·관계 decoder는 캐시 저장 전에 연결했다. 모든 값/API의 최종 domain을 확정한 것은 아니다. 공식 READ는 생성 타입/descriptor·서명 세션·필수 지문·worker 값/ctx 검사로 연결했다. 시간 의존 조회의 명시적 유효 시각, 다른 연결 변경 통보·재연결 복구, 운영 환경 worker 격리 비교도 남는다. V10은 토큰 교체 후 복구만 검증했으며 로그인·refresh-token 제공자와 브라우저 재시작 뒤 영속 복구는 남는다. V11은 직접 참조 부재 조언 한 가지와 resource docs만 검증했으며 제품 검사 전체·설명 소속 전체는 남는다. 기존 독립 검토·쓰기 확장·SDK 작업은 위 결과 문서를 따른다.

이 실험들은 문법·planner·쓰기 모델 결정이 아니다. spike가 임의로 정한 규칙(V1-R*, V2-R*, V3-R*, V4-R*)은 각 결과 문서에 기술 후보로 적었다.

## 이전 진행 현황과 로그

아래 합의/질문 수·사용량·자료 요청·아리아리 결함은 당시 작업 이력이다. 현재 대기 상태·실행 검증 결과로 재사용하지 않는다.

<details>
<summary>통합 지침 이전 진행 기록 보기</summary>

> 창시자가 한 번에 확인하는 문서. 위에서부터 읽으면 된다. 마지막 갱신 시각은 각 절 머리에 적는다.
> 작업 방식: 결정 문서마다 Claude 초안 → Codex(gpt-5.6-sol) 비판 → Claude 반영 → 합의까지 반복. Codex가 연결되지 않으면 Claude Sonnet 서브에이전트로 대체한다.

## 0. 저녁에 먼저 읽을 것 (2026-10-03 19시 기준)

1. **결정 문서 14개를 만들었고, 14개 모두 에이전트 간 합의에 도달했다.** 문서 간 교차 정합성 검토도 1회 거쳤다(P0 0, P1 14건 반영). 합의는 Claude와 비판자(Codex gpt-5.6-sol 또는 Claude Sonnet) 사이의 합의일 뿐이고, 결정 상태는 모두 미결정이다. 창시자 답이 있어야 확정된다.
2. **답해 주실 질문은 25개.** `90-open-questions.md` 맨 위 표에 순서대로 있다. 핵심은 앞의 1~7번(읽기·쓰기 모델)이고, 8~13번(작성 방식, 자연어 문법화)은 예시를 보고 고르게 돼 있다.
3. **원본 자료 요청 3건**: "최초 7가지 원칙" 원문, A형·B형 문법 예시, "자연어의 문법화" 설명(예시 문장 하나면 충분).
4. **아리아리 코드에서 찾은 결함(별도 확인 권장, 모두 코드 읽기 추정, 실행 확인 안 함)**:
   - 서비스 공지·FAQ 등록·수정·삭제 8개 API에 역할 검사가 없음(`SystemNoticeService`, `SystemFaqService`에 "검증 로직 추가 필요" 주석). 설명은 "운영 관리자만"
   - 진행 중 모집 조회·모집 존재 여부 조회에 학교 가시성 검사 없음
   - 조회수 일별 집계 키에 현재 조회수 값이 들어가 "최근 14일 조회수"가 의도대로 계산되지 않을 가능성(`ViewsManager.java` 87행)
   - 지원자 일괄 승인에서 권한 검사보다 동아리 검사가 먼저라 응답 차이로 id 존재를 추측할 수 있음
   - 오류 응답: 커스텀 예외는 HTTP 400 고정인데 본문 code는 403·404 등, DB 오류는 SQL 오류 메시지를 클라이언트에 그대로 돌려줌(`ExceptionControllerAdvice.java`)
5. 토론 기록은 `reviews/`에 문서별·라운드별로 남아 있다. 각 결정 문서 변경 이력에 무엇이 왜 바뀌었는지 적혀 있다.

## 1. 한눈에 보기

| 문서 | 주제 | 상태 | 창시자 질문 |
|---|---|---|---|
| DD-01 | 화면이 새 데이터를 원할 때 서버에 조회를 추가해야 하나(읽기 조합) | 에이전트 합의(3라운드) | F1~F4 |
| DD-02 | 쓰기에서 호출자는 무엇을 표현하고 서버는 무엇을 쥐나 | 에이전트 합의(3라운드) | F1·F2 |
| DD-03 | 조합 읽기를 운영 서버가 언제부터 받아 주나 | 에이전트 합의(3라운드) | F1 |
| DD-04 | 서버 쪽 정의를 무엇으로 쓰나(`.aip`·TS·Python) | 에이전트 합의(Codex 3라운드) | F1~F3 |
| DD-05 | "자연어의 문법화"가 무엇인가 | 에이전트 합의(Sonnet 3라운드) | F1~F3 (예시 먼저 → 원하는 효과 → 실행 영향) |
| DD-06 | Rust 엔진과 JS/Python 앱의 관계(독립 서버 / 임베드 / 설치 도구가 서버 자동 기동) | 에이전트 합의(Sonnet 2라운드) | F1~F3 (배포 환경 → 엔진 위치 → 확장 작성자) |
| DD-08 | 호출자 요청을 어떤 형태로 보내나 | 에이전트 합의(Sonnet 2라운드). 추천: JSON 트리 + 값 분리 | 없음(기술 제안) |
| DD-09 | 상세를 열면 올라가는 익명 통계(조회수) | 에이전트 합의(Sonnet 2라운드). 열람 지점 선언 방식은 DD-01과 함께 미결정 | F1 |
| DD-10 | "내가 북마크했나" 같은 보는 사람 기준 값 | 에이전트 합의(Sonnet 3라운드) | 없음(DD-01 F1·DD-04에 종속) |
| DD-11 | 쓰기 뒤 화면의 어떤 데이터를 다시 불러올지(프론트 캐시 무효화) | 에이전트 합의(Sonnet 3라운드). 추천: 읽기·쓰기 양쪽 태그 | F1(남의 변경이 얼마나 빨리 보여야 하나) |
| DD-12 | 서버 정의가 바뀔 때 무엇을 사람이 확인한 뒤 반영하나(스키마 진화·롤백) | 에이전트 합의(Sonnet 3라운드) | F1~F3 |
| DD-13 | 프론트 SDK가 무엇을 맡나 | 에이전트 합의(Sonnet 3라운드) | F1(라이브러리 범위: 데이터만 / UI 조각까지), F2(첫 출시 지원 프레임워크) |
| DD-14 | 응답과 오류의 모양 | 에이전트 합의(Sonnet 2라운드) | F1(권한 부족을 얼마나 알릴지) |
| DD-07 | 여러 건을 한 번에 처리하다 일부가 실패하면 | 에이전트 합의(Sonnet 2라운드). 추천: 전부 취소 + 안 되는 항목 한 번에 모두 알림 | F1 |

상태 어휘: 확정(창시자 승인) / 잠정 합의 / 검증 필요 / 미결정. "에이전트 합의"는 Claude와 Codex 사이의 합의일 뿐 결정 상태는 모두 **미결정**이다.

## 2. 쌓인 질문 (답하기 쉬운 순서)

`90-open-questions.md` 맨 위 표가 정본이다. 새 질문은 거기에 추가하고 여기에는 요약만 둔다.

- 총 25개: DD-01 F1~F4, DD-02 F1·F2, DD-03 F1, DD-04 F1~F3, DD-05 F1~F3, DD-06 F1~F3, DD-07 F1, DD-09 F1, DD-11 F1, DD-12 F1~F3, DD-13 F1·F2, DD-14 F1. `90-open-questions.md` 맨 위 표에 순서대로
- 원본 자료 요청: Q0 "최초 7가지 원칙" 원문, Q3 A형·B형 문법 예시, Q5 "자연어의 문법화" 설명

## 3. 진행 로그

- 2026-10-03 오후: DD-01~03 각각 Codex와 3라운드 토론, 에이전트 합의. 질문 7개 정리
- 2026-10-03 오후: 창시자 외출. 자율 진행 시작(다음 결정 문서 DD-04부터)
- 15:30 DD-04·05·07 초안. DD-04는 Codex 1라운드. Codex 사용량 5시간 한도 31% 남음(19:27 리셋) → DD-07부터 Sonnet 서브에이전트 병행
- DD-05 실험 E1: 아리아리 API 설명 문장 중 권한 규칙 36개, 그중 12개 코드 대조 → 불일치 2, 예외 누락 1, 강제 불완전 1, "관리자" 용어 모호
- DD-04 Codex 1라운드: 초안 추천(TS로 쓰고 `.aip`는 읽기 표기)이 사실상 고유 문법을 쓰는 형식에서 없애는 것이라는 지적, 예시가 같은 내용이 아니었음, TS 정의 실행 시 보안 경계 누락 → v2에서 추천 철회, 같은 9개 항목을 담은 예시 4개, "TS/Python 프로젝트 안 AIP 선언 블록"을 1순위 검증 후보로. Codex 사용량 24% 남음
- DD-07 Sonnet 1라운드: 사전 점검(U3)은 점검과 실행 사이 상태 변화로 같은 실패가 남고, 점검 자체가 정보 노출 통로가 된다는 지적 + 아리아리 코드의 권한 검사 앞 동아리 검사 오라클 발견 → v2 추천 변경
- DD-04 Codex 2라운드: 예시에 필드·간선 정책 누락, 확장 참조 규칙 충돌, F1·F2 모순 가능 → v3. Codex 사용량 18% 남음, 이후 문서는 Sonnet으로
- DD-01 실험 E1: 아리아리 모집 GET 11개 모두 수정 대안 2로 표현 가능. 새 요구 2개 발견(읽기에 딸린 조회수 증가, "내 동아리인가" 같은 행위자 기준 값) → DD-09·DD-10 예정. 원본 코드에서 학교 가시성 검사가 빠진 조회 2곳 발견(존재 여부, 진행 중 단건. 코드 읽기 추정)
- **중요 발견(DD-05 Sonnet 대조, 코드 확인)**: 아리아리 서비스 공지·FAQ 8개 API가 설명은 "운영 관리자만"인데 서비스 코드에 역할 검사가 없고 `// 검증 로직 추가 필요` 주석만 있음. 로그인 회원이면 서비스 공지 등록 가능으로 보임(실행 확인 안 함, 프론트·게이트웨이 보호 여부 확인 필요). 권한 설명 36개 중 불일치 13개. 아리아리 운영 중이라면 별도로 확인 권장
- DD-09 Sonnet 1라운드: 아리아리 조회수 일별 키에 현재 조회수 값이 들어가 14일 창이 의도대로 동작하지 않을 가능성(`ViewsManager.java` 87행, 코드 확인), PoC는 창 집계 미구현·중복 제거 키가 호출자 헤더 → v2(선언한 상세 형태만 집계, 서버 파생 키, 식별 값 해시)
- Codex가 DD-11 실행 전 사용량 한도에 걸림(화면 안내: 19:27 이후 재시도). 한도 안내 창에서 모델이 `GPT-6-Luna medium`으로 바뀐 상태로 보임(확인 필요). Codex에 gpt-6 계열이 있다는 것이 드러남 → 처음 말씀하신 "6.1sol"이 gpt-6 계열이었을 가능성(확인 필요). DD-11은 Sonnet으로 진행
- DD-11 Sonnet 1라운드: 아리아리 무효화 대조 26경로 중 빠짐 7·과잉 1, 모집 마감 시 프론트가 "CLOSED"를 직접 써 넣는 등 서버 파생 상태 재구현 → v2(읽기 의존 태그 + 쓰기 변경 태그, 동기 경로 한정 명시)
- 교차 정합성 검토(`reviews/CROSS-sonnet-r1.md`): P0 없음, P1 14건 반영. 확장 추가는 사람 확인 대상에서 제외(원문 승인 절차 금지), 확장 우회 방지 불변식의 보장 방식이 DD-06 답에 종속됨을 명시, 저장된 조회와 등록된 요청의 관계 정의, 질문 번호 체계 정리, 클라이언트 SDK 담당 문서(DD-13) 신설
- DD-14 Sonnet 1라운드: PoC도 DB 오류 원문을 응답에 넣고(`exec.rs` db.message()), 다른 테넌트 행은 본문 코드가 NOT_FOUND와 달라 존재 추측 가능(`codes.rs` TENANT_MISMATCH) 확인 → v2(존재 비노출 세 층, 운영에 개발 모드 없음, 문구는 코드+매개변수). PoC 부채로 기록
- 2차 교차 검토(`reviews/CROSS-sonnet-r2.md`): P0 0, P1 4 반영(호출자 요청 작성 API 담당을 DD-13으로, DD-13 오류 분류와 DD-14 category 일치, validUntil 정의 연결, 응답 형식 후속 문서를 DD-14로). 1차 P1 14건 반영 확인

</details>

## 최신 변경 이력

- 2026-10-03 창시자 통합 지침의 기준·산출물·미정 범위·실제 협업을 표시하고 이전 기록을 이력으로 전환.
