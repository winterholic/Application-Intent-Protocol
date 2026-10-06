# forms3 log

## 0. 시작 관찰 (코드 읽기)
- 기준선 골든은 scratchpad/base3 에 보관 예정(diff 요약용).
- 문법 정본(spec/grammar.md:370,373)은 지시문과 다르다: `consent X version N required for [..]`, `impersonate E by <expr> audited reason required ttl <dur>`. 정본과 파서를 따른다. 따라서 Start...Impersonation 은 `reason` 파라미터가 필수.
- 설계 문서 근거: 08-coverage-catalog.md:63 `B08 | 관리자 대리 로그인(impersonation) | impersonate 형식 (감사 필수, 명시 승인)`, :149 `F11 | 초안/게시 | publishable 형식`, :270 `O03 | 약관 동의와 재동의 | consent 형식`, :517 `entity Article { publishable } // F11: draft/published 두 버전, Publish/Discard intent 생성`, :573 `consent Terms version 3 required for [SubmitApply, CreateClub] // O03: 미동의 시 CONSENT_REQUIRED`, :411 `impersonate Member by actor.role = SUPPORT audited reason required ttl 30m`.
- 설계 문서에 저장 방식, 권한, 대리 중 금지 규칙, HTTP 상태, 토큰 형식 언급 없음("명세 언급 없음") -> 아래 결정은 모두 내 결정.
- 기존 상태: sema는 consent 대상 intent 존재를 E104로, IR validate는 I111로 이미 검사. impersonate 는 sema check.rs 가 방문하지 않음(to_core.rs:1983 note).

## 1. 진행
- [consent/IR] CORE_IR_VERSION aip-core/0.6 -> 0.7, 단언 테스트 ir_version_is_0_7. 새 런타임 코드 AIP.CONSENT.REQUIRED(403, 비재시도). IR에 publish_by(Traits), Query.drafts 추가. form_intents 에 consent/impersonation_intents/publishable 정의. analyze: consent 버전/대상, impersonate 대상=actor 엔티티/ttl/중복, publishable 규칙, drafts 규칙 추가(기존 코드 E101,E110,E208,E210,E301 재사용).

## 1. 진행
- [IR] CORE_IR_VERSION aip-core/0.6 -> 0.7, 단언 테스트 ir_version_is_0_7. 새 런타임 코드 AIP.CONSENT.REQUIRED(403, 비재시도). IR에 Traits.publish_by, Query.drafts 추가. form_intents 에 consent/impersonation_intents/publishable 정의. analyze: consent 버전/대상, impersonate 대상=actor 엔티티/ttl/중복, publishable 규칙, drafts 규칙(기존 코드 E101,E110,E208,E210,E301 재사용).
- [구현 1차] aip-plan(CheckKind::Consent, Program.impersonation), aip-pg(schema::published, ddl published/consent/impersonation 표, plan/{consent,impersonate,publish}.rs, superuser_sql 세션 가드, 개인정보 command 가드), runtime(auth Identity+세션 토큰, engine open_session/감사/__imp, http 토큰 서명, exec CONSENT_REQUIRED), contract(forms.rs), sema(check/to_core/impersonate). 새 예제 examples/cms/app.aip.
- 골든 diff(기존): ariari/shop/saas plan 골든 바이트 불변(diff 확인). 계약/클라이언트 골든은 core_ir 버전 문자열만 변경. 신규: cms.plan.json, cms 계약/클라이언트 골든.
- [e2e] crates/aip-cli/tests/cms_e2e.rs 6개 통과: publishable_end_to_end, consent_end_to_end, impersonation_end_to_end, impersonation_ttl_runs_out(ttl 1s로 재컴파일, 실제 대기), impersonation_tokens_over_http(실제 소켓, 서명 위조 5종 + 유령 세션), cms_negative_controls(플랜에서 검사 단계를 제거하면 공격이 성공함을 단언).
- 테스트 작성 중 발견: sql_one 의 `WITH x AS (q) SELECT to_jsonb(x)` 는 컬럼 별칭이 x 이면 스칼라가 되어 Null 로 보임(별칭을 val 로). nested 검사는 target-privileged 검사와 겹쳐서(대리 대상은 by 를 만족할 수 없음) 단독으로는 공격 재현이 안 됨 -> 두 검사를 같이 끄는 경우로 negative control 구성.

## 2. negative control (구현 변이, 코드를 일시로 망가뜨리고 e2e 실패 확인 후 원복; 스크립트 /tmp/negctl.py)
각 줄: 변이 -> 실패한 테스트와 단언.
- publishable 쿼리 뷰 교체 끔(클라이언트가 작업본 테이블을 읽음) -> publishable_end_to_end `a draft is in the editor's list and not in the readers'` 실패.
- Publish allow 를 항상 true -> publishable_end_to_end 외부인 Publish 거부 단언(`left: None`) 실패.
- 삭제 시 게시본 제거 트리거 제거 -> publishable_hard_delete, publishable_end_to_end(`deleting an article unpublishes it`) 실패.
- consent 검사 단계를 발행하지 않음 -> consent_end_to_end 첫 거부 단언 실패.
- consent 버전 조건 제거 -> `consent to version 1 does not cover version 2` 실패.
- 철회가 아무것도 안 함 -> 철회 뒤 status 가 GIVEN 으로 남아 실패.
- 대리 세션 만료 검사 제거 -> impersonation_end_to_end 만료 단언 실패.
- 종료된 세션을 계속 허용 -> 종료 후 호출 단언 실패.
- 대리 중 감사 행을 쓰지 않음 -> 감사 행 단언(`RenameMe is audited`) 실패.
- superuser_sql 의 세션 가드 제거 -> `a session does not lend the superuser bypass` 실패.
- 개인정보/동의/표결 가드(`IMPERSONATION_FORBIDDEN`) 미발행 -> export 거부 단언 실패.
- 토큰 서명 검증 제거 -> auth 단위 테스트 2개와 impersonation_tokens_over_http(위조 토큰이 200) 실패.
- 토큰의 세션을 엔진에 넘기지 않음 -> impersonation_tokens_over_http 실패.
- 계획 수준(상시 테스트 cms_negative_controls): Publish/Discard allow 단계 제거, 독자 쿼리가 작업본 테이블을 읽게 치환, consent 단계 제거, NESTED+PRIVILEGED 단계 제거, PRIVILEGED 단계만 제거(NESTED 가 여전히 막음 확인), 개인정보 가드 제거, superuser 가드를 vacuous 하게 치환. 전부 공격이 실제로 성공함을 단언.
- 주의(방법론): 변이 후 원복한 파일은 mtime 이 오래되어 cargo 가 재빌드하지 않는 함정이 있었다(스모크에서 낡은 바이너리가 걸림). 스크립트가 변이 전후로 모든 .rs 를 touch 하도록 고치고 14건을 처음부터 다시 돌렸다. 14건 모두 여전히 실패(잡힘). 변이 잔재 grep 0건.

## 3. 형식별로 내가 정한 세부와 근거

### publishable (F11)
- 근거 인용: 08-coverage-catalog.md:149 `F11 | 초안/게시 | publishable 형식`, :517 `entity Article { publishable }  // F11: draft/published 두 버전, Publish/Discard intent 생성`. 저장 방식, 권한, 조회 방식은 "명세 언급 없음".
- 저장 방식: 엔티티 테이블 = 작업본, `<table>_published` = 게시본(`CREATE TABLE ... (LIKE <table>, published_at, PRIMARY KEY(id))`, 제약 없는 스냅숏). 대안(같은 테이블 + 초안 JSON 컬럼, `<table>_draft` 섀도)은 모든 command의 쓰기와 로드, 제약, FK, history/versioned 트리거, 테넌트 고정 트리거를 초안 쪽으로 돌려야 해서 버렸다. history/versioned 는 작업본 테이블에 그대로 붙어 있어 영향 없음(history 테이블 이름은 `Table.history_table` 로 분리해 게시본 스키마가 이름을 바꿔도 안전).
- 읽기: `Schema::published()`(같은 컬럼, 테이블 이름만 `_published`)로 컴파일하는 것이 query 하나뿐이라 변경이 한 곳. 파라미터 로드, 참조 조인, 테넌트 필터 경로도 같은 스키마를 따라 게시본을 본다. 게시된 적 없는 행을 파라미터로 받으면 NOT_FOUND.
- 편집 명령은 무변경(작업본). 생성 intent: `PublishE(e)`(작업본 -> 게시본 UPSERT, `published_at = now()`, 반환 `{publishedAt}`), `DiscardEDraft(e)`(게시본 -> 작업본 UPDATE, 게시된 적 없으면 `PRECONDITION.FAILED(NOT_PUBLISHED)`).
- 권한: `publishable by <cond>` 문법 추가(행 필드와 actor, 엔티티 내부 `visible to` 와 같은 방식). 두 intent가 같은 조건. 조건이 없으면 superuser만, superuser도 없으면 아무도 못 하므로 E301(보수적). 문법: spec/grammar.md `'publishable' ('by' expr)?`.
- 초안 조회: 수식어 하나 `drafts query`(순서 `internal`, `cross tenant`, `drafts`). 읽는 엔티티가 publishable 이 아니거나 `allow public` 이면 E208.
- 삭제: 작업본 행이 DELETE 되거나 `deleted_at` 이 채워지면 트리거(`<table>__unpublish`)가 게시본을 지운다.
- 테넌트: Publish/Discard 는 승인 폼과 같은 앞부분(Load 잠금 + 앵커 핀). 독자 query는 앵커 필터가 게시본 스키마 위에서 계산된다. e2e 에서 두 사이트로 확인(목록이 섞이지 않음, 다른 사이트 편집자의 Publish 는 FORBIDDEN).
- 한계(문서화): 게시본의 touch 카운터와 version 은 다음 게시 전까지 작업본과 다르다. 게시본이 참조하던 행이 지워지면 Discard 가 FK 로 실패할 수 있다.

### consent (O03)
- 근거 인용: 08-coverage-catalog.md:270 `O03 | 약관 동의와 재동의 | consent 형식`, :573 `consent Terms version 3 required for [SubmitApply, CreateClub]  // O03: 미동의 시 CONSENT_REQUIRED`. 명세에 HTTP 상태, 기록 구조 언급 없음.
- 지시문의 문법과 정본이 다르다(정본: `required for`). 정본을 따랐다.
- 새 코드 `AIP.CONSENT.REQUIRED` = 403, 재시도 불가, reason = 동의 이름. 근거: 호출자는 알려져 있고 사용자의 행동이 필요한 거부이므로 409(PRECONDITION)가 아니라 403. 계약의 intent errors 에 `AIP.CONSENT.REQUIRED(Terms)` 가 실린다(CreateArticle, ArticleDrafts). 인증 없는 호출은 UNAUTHENTICATED.
- 검사는 intent 자신의 `allow` 다음 단계(권한 없는 사람에게 동의 상태를 흘리지 않음). command, query, job 시작에 적용. internal 이나 폼 이름이 대상이면 E208, 버전 < 1 은 E210, 없는 intent 는 E104(sema)/I111(IR), 생성 intent 이름 충돌은 E101.
- 기록: `_aip_consent(id, name, actor, version, given_at, withdrawn_at)`, 활성 기록은 (name, actor, version) 부분 UNIQUE -> Give 멱등. Withdraw 는 활성 기록 전부에 `withdrawn_at`. 이력은 지우지 않는다. 버전을 올리면 검사가 `version = N` 이라 이전 기록은 남아 있고 인정만 안 된다. Status = GIVEN | NONE | OUTDATED | WITHDRAWN.
- superuser 도 동의를 우회하지 않는다. 동의 Give/Withdraw 는 본인 행위라 대리 중에는 거부.
- `_aip_consent` 는 consent 폼이 있는 프로그램에만 생성(다른 예제의 DDL 골든 불변).

### impersonate (B08)
- 근거 인용: 08-coverage-catalog.md:63 `B08 | 관리자 대리 로그인(impersonation) | impersonate 형식 (감사 필수, 명시 승인)`, :411 `impersonate Member by actor.role = SUPPORT audited reason required ttl 30m`. 대리 중 금지 규칙, 토큰 형식은 "명세 언급 없음" -> 보수적으로 결정.
- 정본 문법에 `audited reason required` 가 있어서 Start 는 `reason`(1~500자) 필수. Start/Stop 모두 audited.
- 세션: `_aip_impersonation(id, operator, target, reason, started_at, expires_at, ended_at)`. Start 는 운영자가 `by`(또는 superuser)를 만족해야 하고 아래 거부 규칙을 적용한 뒤 세션을 열고 `{session, target, expiresAt}` 반환, HTTP 계층이 `token` 을 서명해 덧붙인다(엔진은 비밀을 모름). 만료는 서버가 `expires_at` 으로 판단(토큰 exp 도 같은 시각).
- 토큰: `aip1.<b64 본문>.<b64 HMAC>`, 본문 `<actor>.<exp>`(기존) 또는 `<target>.<exp>.<session>`. 서명이 본문 전체를 덮으므로 세션을 바꾸거나 떼거나 붙이면 서명이 깨진다(단위 테스트 + 실제 소켓 e2e 5종: 서명 한 글자 변조, 다른 비밀로 서명, 다른 actor 로 본문 바꿔치기, 일반 토큰에 세션 이식, 서명 없음). 비밀을 가진 쪽이 만든 존재하지 않는 세션은 엔진이 IMPERSONATION_ENDED 로 거부.
- 엔진: 세션이 있는 호출은 호출마다 `_aip_impersonation` 에서 (id, target = actor, 종료 안 됨, ttl 안) 를 확인하고 운영자를 세션 행에서 가져온다(호출자가 넣은 값은 무시). 세션 중 모든 command 는 `_aip_audit` 에 actor=대상, `impersonated_by`=운영자, `impersonation`=세션(감사 컬럼은 impersonate 폼이 있을 때만 추가). 조회(query)는 감사하지 않음.
- 거부 규칙(전부 DB 단계, 코드는 AUTH.FORBIDDEN + reason): 세션 안에서 시작 `IMPERSONATION_NESTED`, 자기 자신 `IMPERSONATION_SELF`, `by` 나 superuser 를 만족하는 대상 `IMPERSONATION_TARGET_PRIVILEGED`(세션이 운영자보다 큰 힘을 빌려주지 않게; 이 검사 때문에 NESTED 는 단독으로는 재현 불가, 둘이 겹치는 방어), 세션 중 superuser 우회 OFF(`superuser_sql` 에 `__imp IS NULL`), 본인 데이터 `erase`/`export personal data` 를 쓰는 command 와 그런 body 의 job 시작, 동의 Give/Withdraw, approval 투표(approve/reject)는 `IMPERSONATION_FORBIDDEN`. `audited` 경로 자체는 금지하지 않음(이미 감사되고 우회가 아님).
- Stop: 세션 안에서는 그 세션, 운영자 본인 토큰이면 열린 세션 전부. 종료 후 토큰은 즉시 401(IMPERSONATION_ENDED).

## 4. 변경 파일
- IR/분석: crates/aip-ir/src/{lib,forms,facts,form_intents,analyze,validate,codes,builtin}.rs
- 문법/프런트엔드: crates/aip-syntax/src/{ast,parser}.rs, crates/aip-sema/src/{check,lower,model,to_core}.rs
- 계획/백엔드: crates/aip-plan/src/lib.rs, crates/aip-pg/src/{lib,schema,ddl,sqlexpr}.rs, crates/aip-pg/src/plan.rs, crates/aip-pg/src/plan/{consent,impersonate,publish,job,approval}.rs
- 런타임: crates/aip-runtime/src/{auth,engine,exec,http}.rs
- 계약: crates/aip-contract/src/{lib,forms}.rs
- 예제: examples/cms/app.aip (신규), examples/{cms,ariari,shop,saas}/client/aip.ts(버전 문자열, cms 신규)
- 테스트: crates/aip-cli/tests/cms_e2e.rs (신규), crates/aip-cli/Cargo.toml(dev-dep base64), crates/aip-cli/tests/{golden,contract_golden,codes}.rs(cms 추가), crates/aip-sema/tests/core_ir.rs(0.7), conformance/sema/ 12건 신규(publishable_unowned, ok_publishable, drafts_not_publishable, drafts_public, publish_by_not_bool, consent_version_zero, consent_unknown_intent, consent_internal_intent, consent_clashes_with_intent, impersonate_not_the_actor, impersonate_declared_twice, impersonate_without_ttl), crates/aip-pg/tests/golden/{cms,sema_ok_publishable}.plan.json 신규, crates/aip-cli/tests/golden/{cms.*,*.contract.json(version), *.client.ts(version)}, crates/aip-ir/tests/code_sites.snap
- 문서: spec/grammar.md, spec/diagnostics.md(재생성), docs/DECISIONS.md(D-P19~21), docs/design/09-verification-log.md

## 5. 남은 TODO / 확인 못함
- 확인 못함: consent 가 job 시작 intent 에 걸리는 경우의 실행 e2e(플랜 코드와 분석 규칙은 있음, cms 에 job 이 없음).
- 확인 못함: 세션 중 approval 투표 거부는 e2e 로 확인했으나 job body 가 export/erase 인 job 시작 거부는 e2e 없음.
- 게시본의 touch 카운터와 version 이 다음 게시 전까지 작업본과 다르다.
- 열린 세션은 운영자가 `by` 를 잃어도 ttl 까지 유효(세션 중 재검증 안 함). 세션 중 조회는 감사하지 않음.
- 기존 DB에 새 내부 테이블(`_aip_consent` 등)을 추가하는 마이그레이션은 없다(`--reset` 만; 기존 한계와 동일).
- 대리 토큰을 쓰는 클라이언트 지원(생성 TS 클라이언트에 토큰 교체 헬퍼)은 하지 않음.

## 6. 최종 명령 출력 (2026-10-02)
- `cargo fmt --all && cargo clippy -q --workspace --all-targets` -> 출력        0 줄(경고 0)
- `cargo test -q --workspace` -> passed 119 failed 0 (기존 111 + cms_e2e 7 + auth 단위 1)
- `./target/debug/aip check-docs docs spec README.md` -> 67 blocks: 60 parsed, 0 failed, 7 skipped (fragments with '...')
- tsc(`npx tsc --ignoreConfig --noEmit --strict --target es2022 --lib es2022,dom ../../examples/<app>/client/aip.ts`, app = cms ariari shop saas) -> 출력 없음(오류 0)
- 실서버 스모크(aip run + curl, 포트 4177): Start -> token, 토큰으로 RenameMe 200, ExportMyData 403 IMPERSONATION_FORBIDDEN, 위조 401, Stop ended=1, Stop 뒤 401 IMPERSONATION_ENDED, 감사에 RenameMe(impersonated_by 있음) 기록. 끝에 pkill 했고 서버 없음.
