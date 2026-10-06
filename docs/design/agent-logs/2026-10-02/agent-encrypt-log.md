# agent-encrypt log

## 0. 조사 (읽기)
- 아리아리: `examples/ariari/app.aip:70` Member.email `personal encrypted visible to self`가 유일한 encrypted 사용. 다른 예제(shop/saas/cms)는 encrypted 없음(grep).
- 현재: IR `Field.encrypted`는 이미 있음(CORE_IR 0.11). aip-pg/src/lib.rs `unsupported_entity_features`가 W602 발행, 평문 저장.
- SQL은 전부 컴파일 시점 고정, 읽기는 `jsonb_build_object`로 응답 JSON을 DB가 만듦(select.rs). 쓰기는 `$n::text::type` 파라미터 바인딩(Env 문자열).
- 읽기 위치 표식 방식 결정 근거: JSON 내 임의 키를 런타임이 일괄 스캔하면 사용자 Json/Text 값이 마커를 흉내 내 다른 행 암호문을 복호화시키는 공격이 가능 -> 계획에 경로 목록(DecryptPath)을 두고 그 경로에서만 복호화.
- export personal data는 런타임에서 개발용 provider가 payload(subject id만)를 로그로 남길 뿐 데이터를 읽지 않음(dispatch.rs `export.` 분기). 
- aes-gcm crate 사용 가능 확인(`cargo add aes-gcm` -> 0.11.1).

## 1. 구현 진행 (코드 단계)
- aip-ir: 코드 E318/E319/E320 + `AIP.ENCRYPTION.{KEYS_MISSING,DECRYPT_FAILED}`, `AIP.SCHEMA.ENCRYPTION_CHANGE` 등록, W602는 deprecated. analyze.rs 규칙 + diff.rs `FieldEncryption`. conformance 10건 추가(encrypted_*, ok_encrypted) 통과.
- aip-plan: `Step::{NewId,Encrypt}`, `QueryPlan/CommandPlan.decrypt`, `JobExport.decrypt`, `ColumnSpec.encrypted`, `EntitySpec.{published,history}_table`(암호화 컬럼 있을 때만 채움 -> 다른 골든 불변). 모두 serde default/skip.
- aip-pg: select.rs에서 암호화 컬럼은 `{"c":암호문,"i":행id}`로 반환 + 경로 기록, masked는 `'m'` 키로 마스크 이름만 전달(복호화 후 런타임이 적용). Insert는 id를 런타임이 정해(NewId) 명시 INSERT, Set은 기존 행 id 평가 후 암호화. upsert/toggle/insert-from/update-many/비파라미터 값은 E320. sqlexpr에는 E319 backstop.
- aip-runtime: crypto.rs(AES-256-GCM, aes-gcm 0.11.1), exec.rs Encrypt/NewId, engine.rs 응답 복호화·idempotency 보관본은 암호문 유지·audit 입력 마스킹·idempotency 해시는 HMAC 지문, jobs.rs CSV 복호화, rekey.rs, 시작 시 키 검사(crypto::load).
- CLI: `aip rekey <file>`, `aip run`은 DB 접근 전에 키 검사, explain에 Encrypt/decrypt 표시.

## 2. negative control (변이 후 원복+touch)
- 변이 A: `exec.rs` Encrypt 단계에서 `keys.encrypt` 대신 평문을 바인딩 -> 5개 e2e 실패(읽기 쪽 엄격 검사가 먼저 걸림: "stored value is not in the encrypted format").
- 변이 B(암호화·검증 호출 제거: `Keys::encrypt`가 평문 반환, `decrypt`는 비-envelope를 그대로 통과): `the_database_holds_ciphertext_and_the_api_answers_plaintext`가 `encrypt_e2e.rs:82 assert!(stored.starts_with("v1:t1:"))`에서 실패 + 복사본 테스트가 `!work.contains("secret draft")`에서 실패 (4개 실패). 원복 후 9개 통과.
- 변이 C(AAD 제거: `aad()`가 field,row를 무시): `a_ciphertext_moved_to_another_row_or_field_is_refused` 실패(다른 행 암호문이 복호화됨) + 단위 `crypto::tests::bound_to_field_and_row` 실패. 원복 후 통과.
