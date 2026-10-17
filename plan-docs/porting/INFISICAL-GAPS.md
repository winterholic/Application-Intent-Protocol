# Infisical 포팅이 드러낸 AIP 제품 갭 (WBS 7.3, 2026-10-18)

이 목록은 설계 문서(`INFISICAL-DESIGN.md`)에 있던 "AIP 제품 갭 목록"을 대체한다. 갭은 AIP **제품 코드**에 없는 것을 뜻한다. 이 포팅 트랙은 core를 수정하지 않고 갭을 기록한다. 우회로 메우지 않는다. 각 항목은 확인 방법과 상태를 구분한다.

상태 표기: **재현**(이 세션이 명령을 실행해 확인), **판독**(소스를 읽어 확인, 실행 안 함), **분류 변경**(이전에 갭이라 적었으나 갭이 아님).

## 정식 제품 갭

### G2. `encrypted` 필드 수정자와 서버 키 공급이 제품에 없다 (중요)

원본 Infisical의 기본 경로는 서버측 KMS다([INFISICAL-KEYMODEL.md](INFISICAL-KEYMODEL.md)). 이것을 AIP로 옮기려면 서버가 키를 쥐고 필드를 암복호하는 기능이 필요한데 제품 경로에는 없다.

| 사실 | 확인 |
|---|---|
| 제품 서비스 방언은 `value: Text encrypted`를 거부한다. 출력: `DEF.PARSE_EXPECTED`, "`:` 필요, `}` 발견", scope `resource User > fields` | **재현**: `aip service check`를 `fields { ...; secret: Text encrypted }` 정의로 실행 |
| `AIP_ENCRYPTION_KEYS`는 PoC 런타임 `crates/aip-runtime`(`crypto.rs`, `rekey.rs`, `engine.rs`)에만 있다. `aip-service`와 `spikes/*`의 Cargo 의존성에 `aip-runtime`이 없다 | **재현**: `grep -rln AIP_ENCRYPTION_KEYS crates spikes product`와 Cargo.toml 대조 |
| PoC 런타임은 `poc-runtime` feature로만 빌드되고 기본 빌드에서 `aip run/migrate/rekey`는 `RUNTIME_FEATURE_DISABLED`다 | **판독**: `crates/aip-cli/Cargo.toml`, `product/OPERATIONS.md` |

영향: 시크릿 값, 값 버전, 회전, blind index 조회(G4)를 선언할 수 없다. 비밀 관리 앱과 개인정보 앱을 AIP 제품 방언으로 만들 수 없다. 임시 평문 Text 열은 모든 읽기 가능자에게 평문이 노출되므로 우회로 판단해 쓰지 않았다.

설계 소유: 별도 세션의 `docs/design/13-field-encryption.md`(상태 Proposed, 창시자 승인 대기). 설계 13(`a257b8d`)은 GPT 독립 정적 리뷰에서 "현 초안 그대로 구현에 넘기지 않는다"는 판정을 받았다. P1은 blind index 회전과 unique, SKIP LOCKED 재개 누락, idempotency·job 평문 경로, HMAC 회전 뒤 재시도, 이전 binary rollback 호환성이다. 이 리뷰는 정적 검토이며 테스트·빌드·DB를 실행하지 않았으므로 실행상 고장의 증명은 아니다. 사용자 승인도 없다. 따라서 이 초안은 승인된 설계도 제품이 보증하는 기능도 아니고, 암호화에 의존하는 Infisical 포팅 항목(값·버전·회전·blind index·동기화 내용)은 **미구현·설계 미확정**으로 취급한다. 이 트랙은 그 문서를 수정하지 않는다. 문서가 인용한 `porting/infisical/app.aip:134` 등 줄 번호는 이 재작업 이후 달라졌다(`valueCiphertext`는 정의에서 제거).

현재 포팅 처리: `app.aip`는 값을 선언하지 않고 머리 주석과 `docs { summary }`로 갭을 표시한다. 값 없는 Secret은 이름과 세대, 소프트 삭제만 가진다.

### G10. 행 삭제와 부분 유일 제약이 없다

제품 방언에는 물리 delete가 없고(`product/SDK.md` "선언 없는 update나 물리 delete는 열리지 않는다") 조건부 유일 인덱스도 없다. 그래서 "삭제 후 같은 이름으로 다시 만들기"를 `unique (folder, key)`로 표현할 수 없다. 이전 초안은 삭제를 값 null 처리로 흉내 내서 삭제된 시크릿이 목록에 남고 이름을 재사용할 수 없었다(**재현**, `evidence/before-legacy-53b1537.txt`).

현재 포팅 처리(우회가 아닌 방언 안의 표현): 이름(`SecretKey`)과 세대(`Secret`)를 나누고 `limit atMostOneLiveSecret ... invariant ... per secretKey`로 "이름당 살아 있는 세대 1개"를 유일 인덱스로 강제한다. 삭제된 세대는 `rows read`에서 빠진다. **재현**: `tests/test_live_db.py::test_deleted_secret_leaves_reads_and_the_name_can_be_recreated_twice`. 비용: 삭제 행이 지워지지 않고 남는다(감사에는 유리, 용량에는 불리). 제품에 부분 유일 제약이 생기면 한 resource로 합칠 수 있다.

### G11. create가 같은 트랜잭션에서 관련 행을 만들지 못한다

`expose create`에는 `transition`의 `create` 효과가 없다. 조직을 만든 사용자가 그 조직의 첫 admin이 되려면 별도 요청이 필요하다. 이전 초안은 이 단계가 없어 만든 조직을 만든 사람이 읽을 수도 없는 고아 조직이 됐다(**재현**).

현재 포팅 처리: `Organization.createdBy`를 `actor`로 강제하고, 조직의 생성자만 구성원이 0명인 조직에서 자기 자신을 admin으로 한 번 등록할 수 있게 `OrgMembership.create`의 `allow`를 구성했다(**재현**: `test_organization_creator_becomes_admin_exactly_once`). 두 요청 사이 짧은 구간이 남는다.

### G12. create 결과 id를 다음 연산에 넘기는 일반 조합이 없다

`SecretKey`를 만든 id를 `Secret` create에 쓰려면 요청이 둘이다(`product/SDK.md` "연산 사이로 값을 넘기는 일반 조합은 없다"). 원자 묶음이 필요한 곳은 `compose`의 `item`으로 제한된다. 포팅에서는 두 요청으로 처리하며 두 요청 사이 실패 시 이름만 남는다.

## 이전에 갭으로 적었으나 분류를 바꾼 것

| 이전 ID | 이전 서술 | 정정 |
|---|---|---|
| G3 | SRP 패스워드 프로토콜을 AIP auth가 지원하지 않는 갭 | **분류 변경**. SRP는 원본에서도 레거시 경로이고 로그인과 비밀번호는 AIP가 아니라 외부 IdP의 몫이다([INFISICAL-AUTHN.md](INFISICAL-AUTHN.md)). 제품 갭이 아니다 |
| G2(이전) | "E2E/클라이언트 보유 키 모델이 AIP의 서버 키와 충돌" | **철회**. 원본의 기본 경로가 서버측 KMS라 충돌이 없다. 위 새 G2가 대체 |
| G5 | TTL/lease 동적 시크릿 의미 | 동적 시크릿은 `ee/services/dynamic-secret`에 있다. EE 범위이므로 MIT 분모의 갭이 아니다(EE 경계는 [INFISICAL-EE-BOUNDARY.md](INFISICAL-EE-BOUNDARY.md)) |

## 미확정 갭

| ID | 내용 | 상태 |
|---|---|---|
| G1 | 순위 기반 전문검색 선언 불가 | FlaskBB 7.5가 제품 방언 `search` 파서 거부를 확인했다(다른 세션의 기록). 이 세션은 재확인하지 않았다 |
| G4 | blind index(암호문 동등 검색) | G2에 종속. 설계 13의 `lookup: eq`가 대응안이지만 이 안의 blind index 회전과 unique가 리뷰 P1이라 미확정. 원본은 서버가 HKDF+HMAC으로 계산한다 |
| G6 | X.509/EST/SCEP/ACME 발급 의미 | 확장 job으로 감쌀 수 있으나 선언 의미는 없음. 미착수, 판독 수준 |
| G7 | CASL 조건식 → predicate | 이 세션에서 역할 기반 권한(admin/member/viewer/no-access, 조직 admin 암묵 권한)은 predicate로 표현하고 실DB로 확인했다. 원본의 조건식(환경, 경로, 태그 조건)과 권한 평가 핵심의 EE 소재는 미확인이며 EE 경계 문서에서 다룬다 |
| G8 | provider capability 등록 절차 | 90종 연동의 등록·검증·버전 운영 절차 미정. 미착수 |
| G13 | 서버 job 확장이 큐별 의미(동기화, 리마인더 전달, 웹훅)를 실제로 수행하려면 필요한 capability(외부 HTTP, SMTP, 큐 재시도 정책)와 resource가 아직 선언되지 않았다 | 큐 33개 중 1개만 구현([INFISICAL-JOBS.md](INFISICAL-JOBS.md)) |
