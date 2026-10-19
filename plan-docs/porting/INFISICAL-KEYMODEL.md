# I05 키 모델 (WBS 7.3.3.6, Infisical) — 2026-10-18 정정판

이전판은 "원본 Infisical은 E2E 암호화이고 AIP는 서버 보유 키라 모델이 충돌한다"고 전제하고 안 A/B/C 중 하나를 창시자에게 고르게 했다. 독립 검증 결과 그 전제가 틀렸다. **결정 요청은 철회한다.** 이 문서는 원본 소스를 읽어 확인한 사실로 다시 쓴 것이다. 원본은 실행하지 않았다.

기준 revision은 `47246becc445869ba73077baa0a53a683e3508ce`(MIT 범위, `backend/src`). 아래 줄 번호는 이 revision의 값이다.

## 원본의 기본 키 모델은 서버 보유 키다

| 사실 | 근거 | 확인 방법 |
|---|---|---|
| 새 프로젝트는 항상 `ProjectVersion.V3`으로 만들어지고 프로젝트 행에 서버 KMS 키 id(`kmsSecretManagerKeyId`)가 붙는다 | `services/project/project-service.ts:374-375` (`kmsSecretManagerKeyId: kmsKeyId`, `version: ProjectVersion.V3`) | 소스 판독 |
| 루트 키(`ROOT_ENCRYPTION_KEY`, 소프트웨어 또는 HSM)를 서버 메모리에 올리고, KMS 키 재료는 AES-256-GCM으로 그 루트 키에 감싸 저장한다 | `services/kms/kms-service.ts:154`(변수), `:200`(`cipher.encrypt(kmsKeyMaterial, ROOT_ENCRYPTION_KEY)`), `:1777`(적재). `lib/config/env.ts:310`에 `ROOT_ENCRYPTION_KEY` 환경 변수 | 소스 판독 |
| 암복호는 서버가 한다. 데이터 키를 안쪽 스코프에 두고 `encryptor`/`decryptor`를 돌려주며 호출자는 키를 보지 못한다 | `services/kms/kms-service.ts:1266-1291`(`createCipherPairWithDataKey`) | 소스 판독 |
| blind index는 서버가 계산한다. 데이터 키에서 HKDF-SHA256으로 전용 키를 만들고 HMAC-SHA256으로 값의 색인을 낸다 | `lib/crypto/blind-index.ts:6-26`(HKDF, 도메인 분리 문자열), `:30-38`(HMAC), `:58-71`(`createSecretValueBlindIndexer`), 사용처 `kms-service.ts:1273` | 소스 판독 |
| SRP 로그인은 레거시 경로다. 이메일 인증이 기본이고 SRP 코드는 "legacy SRP clients"에서만 도달한다고 주석에 적혀 있다 | `services/auth/auth-login-service.ts:410`("Deprecated: provider token validation removed. This path is only reachable from legacy SRP clients."), `:444`("Deprecated SRP path: default to email auth.") | 소스 판독 |
| 레거시 프로젝트 버전 `V1`, `V2`가 enum에 남아 있고 V3로 올리는 큐(`ProjectV3Migration`, `UpgradeProjectToGhost`)가 있다 | `db/schemas/models.ts:448-452`, `queue/queue-service.ts`의 `QueueName` | 소스 판독. V1/V2 시절의 클라이언트 보유 키 흐름은 읽지 않았다(`확인 필요`) |

즉 원본의 기본 경로는 클라이언트가 키를 쥐는 zero-knowledge가 아니라 서버가 루트 키, 키 재료, 데이터 키를 쥐고 암복호와 색인을 하는 서버측 KMS다. E2E는 레거시 V1/V2와 SRP 호환 경로의 모양이다.

확인하지 못한 것이 하나 있다. 시크릿 값 자체가 이 데이터 키로 암호화돼 저장되는 호출 경로(`secret-v2-bridge` 서비스)는 이 세션의 보호 장치가 파일명 때문에 읽기를 막아 직접 보지 못했다. 위 사실들과 V3 프로젝트에 서버 KMS 키가 붙는다는 점에서 그렇게 추정하지만 `확인 필요`다.

## 이전 서술 철회

- "원본은 E2E다, 서버는 암호문과 메타만 본다"는 서술을 철회한다.
- 안 A(서버 키 일원화), 안 B(암호 영역을 잔여 서비스로), 안 C(값 영역 opaque 분리)의 비교와 "안 C 추천"을 철회한다. 안 C의 "원본 E2E 약속을 바꾸지 않는다"는 전제 자체가 사실이 아니다.
- 창시자 결정 요청(C 승인 / A로 변경 / B로 변경)을 철회한다. 사용자에게 남는 이 주제의 결정은 없다. 남는 것은 아래 "AIP 쪽 갭"이다.

## AIP와의 대응

서버측 키는 AIP의 방향과 같다. 서버가 인가와 암호를 함께 최종 결정하는 구조다(`docs/PRINCIPLES.md` 원칙 2). 그래서 안 A(서버 키로 일원화)가 원본과 같은 모델이다. 모델 충돌은 없다.

| 원본 요소 | AIP 쪽 대응 | 현재 상태 |
|---|---|---|
| 루트 키(env 또는 HSM) | 서비스 설정의 키 공급자(설계안: `docs/design/13`) | **없음**(제품 경로). PoC 런타임(`crates/aip-runtime`)에만 `AIP_ENCRYPTION_KEYS`가 있다 |
| 프로젝트별 데이터 키 | `encrypted(scope: <refField>)` 범위별 DEK(설계안) | **없음** |
| 서버측 암복호 | 제품 방언 `encrypted` 필드 수정자(설계안) | **선언 정규화 구현, 실행·배포 차단**(`DEF.ENCRYPTION_RUNTIME_UNAVAILABLE`) |
| blind index (HKDF+HMAC) | `lookup: eq`(설계안, HMAC-SHA256 별도 키) | **없음** |
| 키 회전, rekey | durable job 기반 rekey(설계안) | PoC 런타임에만 있음 |

## AIP 쪽 갭 (G2)

위 표의 "설계안"은 `docs/design/13`(Proposed)이다. 설계 13(`a257b8d`)은 GPT 독립 정적 리뷰에서 "현 초안 그대로 구현에 넘기지 않는다"는 판정을 받았다. P1은 blind index 회전과 unique, SKIP LOCKED 재개 누락, idempotency·job 평문 경로, HMAC 회전 뒤 재시도, 이전 binary rollback 호환성이다. 이 리뷰는 정적 검토이며 테스트·빌드·DB를 실행하지 않았으므로 실행상 고장의 증명은 아니다. 사용자 승인도 없다. 따라서 이 초안은 승인된 설계도 제품이 보증하는 기능도 아니고, 암호화에 의존하는 Infisical 포팅 항목(값·버전·회전·blind index·동기화 내용)은 **미구현·설계 미확정**으로 취급한다.

[INFISICAL-GAPS.md](INFISICAL-GAPS.md) G2에 정식 제품 갭으로 기록했다. 요약하면 `encrypted(scope: ...)` 선언과 키·DEK primitive는 있지만 제품 저장·읽기·저널 통합이 미완이라 실행·배포가 차단된다. 설계는 `docs/design/13-field-encryption.md`(상태: Proposed)가 맡고 있고 이 포팅 트랙은 core를 고치지 않는다. 그 사이 `porting/infisical/app.aip`는 시크릿 **값을 선언하지 않는다**. 평문 Text 열에 값을 넣으면 모든 읽기 가능자에게 평문이 노출되는 우회가 되기 때문이다. 이전 초안의 `valueCiphertext: Text?`는 실DB에서 읽기도 안 됐다(`WIRE.FIELD_NOT_EXPOSED`). 값 저장·버전·회전·blind index 조회는 설계 승인과 암호화 제품 통합이 검증된 뒤에 선언한다.

## 부록: 7.3.3.8 EE 제외 비준은 철회, 대체 문서로 이동

이전판 부록의 "EE 77 서비스·122 라우터 파일 제외 비준"은 근거가 부족했다. 권한 평가 핵심(`ee/services/permission`)이 EE에 있고 MIT 파일 446개가 EE 모듈을 import한다. 이 사실로 "MIT만 포팅"이 성립하는지 다시 분석했고 사용자 판단 항목으로 [INFISICAL-EE-BOUNDARY.md](INFISICAL-EE-BOUNDARY.md)에 정리했다. 비준은 사용자 판단 전까지 잠정이다.
