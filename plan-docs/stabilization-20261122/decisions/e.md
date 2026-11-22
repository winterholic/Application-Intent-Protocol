# Track E 소유자 결정 항목 (진단 JSON, 확장 접근)

작성: 2026-11-22, 브랜치 `stab/e-20261122`. 여기 있는 항목은 공개 의미·호환성·지원 범위를 바꾸므로 Track E가 확정하지 않았다. 결정 대기로 독립 작업을 멈추지 않았고, 호환 유지형 가산 구현은 이미 들어갔다(`diagnostics-fields.md`). 근거 문서는 `plan-docs/hardening-1.0/diagnostics-contract.md`(G1~G10), `policy.md` R1.

| ID | 주제 | 지금 상태 | 추천 |
|---|---|---|---|
| D-E1 | 진단 JSON 장기 호환성 약속의 범위 | 약속 없음. 기계 필드는 시험으로 고정 | A(최소 계약) |
| D-E2 | `schema` 버전 필드(G4) | 넣지 않음 | 약속 범위(D-E1)가 정해지면 같이 결정. A면 1.0 전에 추가 |
| D-E3 | 위치 없는 진단의 표현(G1) | 0:0 유지 + `located:false` 추가. 위치를 알 수 있는 사용처는 실제 좌표로 채움(결정 불필요 범위) | 현행 유지 |
| D-E4 | 문구로 갈리는 코드의 분리(G9) | `DEF.ACCESS_TARGET_NOT_EXPOSED` 적용됨(2라운드). flaskbb workflow 전환은 F1 | (a) 새 코드 추가(적용된 방식) |
| D-E5 | 정의 오류가 아닌 실패 코드 62종(G5) | `reason`만 추가 | (나) 잠정 유지 + 후속 (가) |
| D-E6 | `BrokenPipe`를 종료 0으로 처리(G10) | 그대로 | 문서화만 |
| D-E7 | 읽기 확장 무선언 읽기(R1) | (a) opt-in `access read.<Resource>` 구현됨(선언 안 한 확장은 불변) | (a) opt-in |
| D-E8 | 쓰기 확장의 미사용 grant 경고 | 하지 않음 | 실행 중 관측으로, 우선순위 낮음 |

## D-E1 진단 JSON 장기 호환성 약속

**문제.** 어느 필드를 1.x 동안 제거·타입 변경·의미 변경하지 않겠다고 약속할지 정해지지 않았다. `COMPATIBILITY.md`의 진단 JSON 표는 선택 필드(`scope`, `help`, `explain`)를 항상 있는 것처럼 읽히게 쓴다(G7). 이번 작업이 필드를 더했으므로 "무엇이 약속인가"가 더 중요해졌다.

**재현 근거.** `aip service check`의 실제 출력에서 호스트 리터럴 형식(`.h.ts`)의 `DEF.H_SHAPE`와 `DEF.WRONG_LANGUAGE`에는 `scope`가 없다(`diagnostic_fields.rs`의 `a_diagnostic_without_a_scope_has_neither_scope_nor_scope_path`). 키 집합 정확 비교 시험은 가산 변경마다 깨졌다(G3, 이번에 최소 계약 검사로 바꿈).

**선택지.**

- A. 최소 계약 보장: 종료 코드, 스트림, `ok`, `code`와 `severity`, `line`·`column`(위치 없음은 0:0, `located`), `message`의 존재와 타입만. `message`·`help`·`scope`·`explain`·`scopePath`·`anchor`·`reason`은 변경 가능. 장점은 문구 개선과 필드 추가가 자유롭고 `diagnostics-contract.md` 4절과 일치. 단점은 AI·IDE가 `scopePath`·`anchor`에 의존하면 약속 밖이다.
- B. 지금 출력의 모든 키를 보장. 장점은 소비자가 안심한다. 단점은 문구(`message`, `help`)까지 고정돼 개선과 번역을 막고, 선택 필드 부재 케이스가 많아 고정 비용이 크다.
- C. 약속하지 않음(현상). 장점은 비용이 없다. 단점은 소비자가 어느 필드가 안전한지 모른다.

**추천.** A. 단 `anchor`와 `scopePath`, `reason`은 이번에 새로 더한 필드라 "기계 필드이지만 보장 대상은 소유자가 고름"으로 두고, 쓰는 소비자(편집기, 에이전트)가 생기면 A에 편입한다.

**코드 변경 범위.** 구현은 이미 있다. 약속을 정하면 `COMPATIBILITY.md` 진단 JSON 절과 `product/SUPPORT.md`를 고친다(소유자 승인 필요, G7). 시험은 `crates/aip-cli/tests/service.rs`의 `contract` 모듈이 A의 최소 검사를 이미 구현한다.

**기존 소비자 영향.** 없음(문서만).

## D-E2 `schema` 버전 필드 (G4)

**문제.** 저장된 JSON(CI 로그, 아티팩트)만 보고는 어느 스키마인지 알 수 없다. `aip --version`을 다시 물어야 한다.

**재현 근거.** 출력에 `schema`·`version` 키가 없다(`COMPATIBILITY.md` 진단 JSON 항: "별도 스키마 버전 필드는 없다").

**선택지.**

- (a) 지금 최상위에 `"schema":"aip-diagnostics/1"`을 성공·정의 거부·그 밖의 실패 세 모양 모두에 추가. 없으면 1로 해석한다는 규칙을 문서화한다. 장점은 가산 변경이라 호환 유지, 나중에 major를 올릴 표지가 생긴다. 단점은 버전 필드를 두는 순간 "major를 올릴 때만 깨질 수 있다"는 약속이 된다.
- (b) 1.x에서 가산으로 추가. 필드가 없는 출력은 v1로 읽으므로 지금 넣지 않아도 호환이 같다.
- (c) 두지 않는다.

**추천.** D-E1과 함께 정한다. D-E1이 A면 (a). 이번에 구현하지 않은 이유: 필드 하나이지만 약속이 되는 값이라 소유자 확정 뒤에 넣는 것이 안전하다.

**코드 변경 범위.** `crates/aip-service/src/diagnostics.rs`의 세 구성 함수(`definition_rejected`, `error`, `check_report`)에 한 줄씩과 시험. 약 10줄.

**기존 소비자 영향.** 알 수 없는 필드를 무시하는 소비자(5.4절 규칙, `service.rs`의 `contract`가 시험)에는 없다. 닫힌 스키마로 검증하는 소비자는 깨진다. 저장소 안에는 없다(확인: 진단을 읽는 저장소 코드는 키를 골라 읽는다).

## D-E3 위치 없는 진단의 표현 (G1)

**문제.** 정의 전체에서 나와 위치가 없는 진단이 `line:0, column:0`으로 나온다. 5.2절의 "1 이상" 규칙과 어긋나고 편집기가 -1 줄로 바꿀 수 있다.

**재현 근거.** 빈 `.aip`에 `aip service check` → `"column":0,"line":0`, `DEF.MISSING_ITEM`(`service.rs`의 `a_definition_without_a_position_is_marked_and_keeps_the_zero_coordinates`).

**선택지.**

- (a) 0:0을 유지하고 `located:false`를 더한다. **구현했다.** 장점은 기존 필드 값이 하나도 바뀌지 않는다(before/after 비교에서 기존 필드 125개 차이 0, `track-e.md`). 단점은 `line`을 그대로 쓰는 소비자는 여전히 0을 본다.
- (b) 1:1로 바꾸고 `located:false`를 더한다. 장점은 `line`·`column`이 항상 1 이상. 단점은 기존 값이 바뀌어 0:0을 "위치 없음"으로 읽던 소비자가 있으면 깨지고, 실제 1:1 진단과 `line`만으로는 구분되지 않는다.
- (c) 가능한 진단에 실제 좌표를 채운다. 장점은 가장 정확하다. **위치를 알 수 있는 곳은 채웠다**(아래). 위치가 정의되지 않는 곳(`actor` 선언 없음, 블록 없음, 암호화 런타임 게이트)은 채울 좌표가 없어 0:0과 `located:false`로 남는다.

**추천.** (a)를 유지한다. (b)는 권하지 않는다.

**코드 변경 범위.** (a)는 끝났다. (c)의 소유자 결정이 필요 없는 부분도 끝났다(2라운드): 리터럴이 span을 가져 잘못된 리터럴(`email = "x"`, `d = "2024-99-99"`, 조건이 숫자 `5`)이 0:0 대신 리터럴을 가리킨다. `DEF.SOURCE_TOO_LARGE`는 상한을 넘기는 첫 문자, `DEF.MULTIPLE_BLOCKS`는 두 번째 `aip`/`define`, `DEF.WRONG_BINDING`은 남는 import 또는 첫 사용을 가리킨다. 남은 0:0: `DEF.MISSING_ITEM`(actor 선언 없음), `DEF.NO_BLOCK`, `DEF.ENCRYPTION_RUNTIME_UNAVAILABLE`. 이 셋이 "표현 자체를 바꿀지"(1:1 등)의 대상이다.

**기존 소비자 영향.** (a) 없음. (b)는 위 단점. (c)는 위치 없음이 줄어드는 방향이라 `located`를 읽는 소비자에게 유리하고, 0:0만 읽던 소비자는 값이 바뀐다.

## D-E4 문구로 갈리는 코드의 분리 (G9)

**문제.** 저장소 안 소비자가 `message` 부분 문자열로 오류 종류를 가른다. `DEF.UNKNOWN_SYMBOL`은 서로 다른 조치를 요구하는 여러 상황을 덮는다.

**재현 근거.** `.github/workflows/flaskbb-post-order.yml:183`에서 `DEF.UNKNOWN_SYMBOL`과 `"expose apply" in d["message"]`를 함께 검사한다. 발생지는 `sema.rs`의 쓰기 extension/job access이며 현재 둘 다 `DEF.ACCESS_TARGET_NOT_EXPOSED`를 낸다. 같은 코드 `DEF.UNKNOWN_SYMBOL`은 actor 대상(`sema.rs:281`), enum 값(`sema.rs:520`) 등에서 계속 사용된다(G9). 남은 코드별 발생지 수는 `diagnostics-fields.md` 7절.

**선택지.**

- (a) 새 코드를 추가하되(`DEF.ACCESS_TARGET_NOT_EXPOSED`) 그 자리의 출력 코드를 새 코드로 바꾸고 flaskbb CI를 코드 비교로 옮긴다. **적용했다**(2라운드). 장점은 문구 의존이 사라진다. 단점은 그 자리에서 `DEF.UNKNOWN_SYMBOL`을 매칭하던 소비자가 깨진다(저장소 안에는 이 CI 하나).
- (b) 기존 진단은 그대로 두고 새 선택 필드 `reason`(`ACCESS_TARGET_NOT_EXPOSED`)을 더해 CI가 그 필드를 본다. 장점은 코드 값이 안 바뀐다. 단점은 코드가 두 겹(코드와 reason)이고 `explain-code`가 reason을 설명하지 못한다.
- (c) 하지 않는다.

**추천.** (a). `diagnostics-contract.md` 5.1절 8번이 이 방식(새 코드 추가 후 소비자 전환, 한 번만)을 정하고 있다. (b)는 `reason`이 이미 비정의 실패에서 같은 역할을 하는 것과 일관되지만, 정의 오류는 코드 레지스트리가 설명의 중심이라 코드가 맞다.

**코드 변경 범위.** `sema.rs`의 두 자리(쓰기 확장 access, job access), `codes.rs`(항목 추가), 생성 문서 골든 1회. 적용 결과: 저장소 정의 195개를 `--dev-checks` 유무로 이전·이후 바이너리에 먹인 390회 호출에서 달라진 것은 `porting/flaskbb/tests/fixtures/topic-move-private-grant-repro.aip`의 `code`, `explain`, `help`뿐이다(`message`는 그대로). `.github/workflows/flaskbb-post-order.yml`은 F1 소유라 바꾸지 않았다. 필요한 diff는 `track-e.md` 7절.

**기존 소비자 영향.** (a)는 위 단점. 다른 소비자는 코드가 `DEF.UNKNOWN_SYMBOL`일 때 그 자리를 구분하지 않고 쓰는 경우뿐이며 저장소에서는 찾지 못했다(`grep` 기준).

## D-E5 정의 오류가 아닌 실패 코드 62종 (G5)

**문제.** `aip-service`의 실패 코드(`SOURCE_IO`, `DB_CONNECT` 등 서로 다른 62개 문자열)는 영역 접두가 없고 `aip explain-code SOURCE_IO`는 모른다. `spec/definition-diagnostics.md`는 "모든 AIP 오류 코드는 `영역.이름`"이라고 쓴다.

**재현 근거.** `aip service check`의 없는 파일에서 `{"code":"SOURCE_IO",..}`, `aip explain-code SOURCE_IO` → 종료 1.

**선택지.** (가) 이 코드들을 `CLI` 같은 영역으로 등록하고 `explain-code`에 싣는다(코드 문자열이 `CLI.SOURCE_IO`로 바뀌면 동작 변화). (나) 잠정 코드로 두고 `diagnostics` 유무로 두 모양을 구분한다.

**추천.** (나)를 유지하고 후속으로 (가). 이번에 파일 읽기 실패 네 종류에 `reason`을 더해 문구 파싱 없이 가를 수 있게 했다(코드는 그대로).

**코드 변경 범위.** (가)는 `crates/aip-service/src/*.rs`의 `error("…")` 호출 전체와 레지스트리. 62종이라 큰 변경.

**기존 소비자 영향.** (가)는 코드 이름을 바꾸므로 `SOURCE_IO` 등을 매칭하는 소비자가 깨진다.

## D-E6 `BrokenPipe`를 종료 0으로 처리 (G10)

**문제.** stdout이 닫혀 JSON이 전달되지 않아도 종료 0이다. "종료 0이면 JSON이 전달됨"이 깨질 수 있다.

**재현 근거.** `crates/aip-service/src/diagnostics.rs`의 `finish`(이전 `lib.rs:1787`). 헤드 같은 파이프에서 읽는 소비자가 부분 출력을 성공으로 볼 수 있다.

**선택지.** (a) 종료 0은 "정의 수용"만 뜻하고 전달은 보장하지 않는다고 문서화. (b) EPIPE에서 비영 종료.

**추천.** (a). `| head`처럼 일부러 닫는 사용이 흔해 (b)는 쉘 파이프라인을 깬다.

**코드 변경 범위.** (a)는 문서뿐. (b)는 `finish` 한 줄.

**기존 소비자 영향.** (b)만 있다.

## D-E7 읽기 확장의 무선언 읽기 (R1)

**문제.** 읽기 확장의 `ctx.data.read`는 `access read.<Resource>` 선언 없이 `expose read`가 있는 어느 resource든 읽는다(호출자 정책은 적용된다). 정의만 봐서는 그 확장이 무엇을 읽는지 알 수 없다.

**재현 근거.** `policy.md` R1과 프로브 C7·C8(우회 없음). 이번 스캔에서 FaceReview 읽기 확장 4개가 선언 없이 읽는다(`e2-extension-access.md` 2절, `e2/scan-result.md`).

**선택지.** `policy.md` R1과 같다. (a) 읽기 확장에서도 `access read.<Resource>`를 허용하고 하나라도 선언한 확장에서만 목록 밖 읽기를 거부(opt-in). (b) 선언을 필수로. (c) 정의 검사 경고만.

**추천.** (a). (b)는 FaceReview 4건이 `WIRE.ACCESS_NOT_DECLARED`로 깨져 호환이 깨진다(이전 시도 브랜치가 퇴보로 판정된 이유). (c)는 선언 문법 없이는 조치할 수 없는 경고다. 확장 ABI를 안정 계약으로 확정하는 결정이 아니다(OI-X2는 그대로).

**코드 변경 범위.** 구현했다(2라운드): `sema.rs`의 읽기 확장 분기, `spike-v4-worker/src/lib.rs`의 `Grant.reads`와 `serve_call` `"read"`, `ctx.d.ts` 주석. 시험과 회귀는 `e2-extension-access.md` 3.2절. `--dev-checks` 경고와 문법 카드는 하지 않았다(같은 문서 3.2·3.3절).

**기존 소비자 영향.** (a)는 선언하지 않은 확장에 변화가 없다. 선언한 확장만 목록 밖 읽기가 거부된다.

## D-E8 쓰기 확장의 미사용 grant 경고

**문제.** 확장이 선언한 grant를 실제로 안 써도 알 수 없다. 안 쓰는 grant는 권한 표면을 넓히기만 한다.

**재현 근거.** 스캔에서 선언했지만 코드에서 못 본 grant 6건(`e2-extension-access.md` 2절). 텍스트 스캔의 한계(공용 모듈, 계산된 이름)로 거짓 양성이 섞여 있다.

**선택지.** (a) 정적 경고. 정의와 코드가 별도 파일이라 신뢰하기 어렵다. (b) 실행 중 관측: 호출마다 쓴 grant를 기록하고 배포 단위로 한 번도 안 쓴 grant를 보고. (c) 하지 않는다.

**추천.** (b)를 낮은 우선순위로. 선언 밖 접근은 이미 강제로 막히므로 안전 결함이 아니다.

**코드 변경 범위.** (b)는 worker 호스트의 grant 사용 기록과 보고 경로. 저장 위치·보존 범위 결정이 필요하다.

**기존 소비자 영향.** 없음(관측만).
