# 진단 JSON 필드 명세: 기계 필드와 설명 필드 (Track E1)

- 기준: `origin/main`에서 분기한 `stab/e-20261122`, 측정일 2026-11-22. 구현은 `spikes/spike-v1-fixture/src/diag.rs`, `crates/aip-service/src/diagnostics.rs`, `spikes/spike-v11-dev-checks/src/lib.rs`, `spikes/spike-v1-fixture/src/codes.rs`.
- 상태: **구현이 하는 일의 기술이다. 호환 약속이 아니다.** 어느 필드를 1.x 동안 보장할지, 버전 표지를 둘지는 소유자 결정이다(`decisions/e.md` D-E1, D-E2). 이 문서가 "기계 필드"라 부르는 것은 "시험이 고정하고 문구와 무관하게 읽을 수 있게 만든 필드"라는 뜻이다.
- 방향: 코드와 의미는 안정, 문구는 개선 가능, 위치는 일관된 좌표 규칙. 설계 근거는 `plan-docs/hardening-1.0/diagnostics-contract.md`(G1~G10).
- 원칙: **기존 필드는 제거·개명·의미 변경하지 않았다.** 아래 "추가"로 표시한 것은 모두 선택 필드다. 읽는 쪽이 기존 필드만 읽어도 동작한다(`spikes/spike-v1-fixture/tests/diagnostic_fields.rs`의 `a_reader_of_the_old_fields_only_still_reads_every_diagnostic`, `crates/aip-cli/tests/service.rs`의 `contract` 검사).

## 1. 출력의 세 모양

| 모양 | 언제 | 스트림 | 종료 코드 |
|---|---|---|---|
| 수용 | 정의가 통과(경고가 있어도) | stdout, JSON 한 줄 | 0 |
| 정의 거부 | `{"ok":false,"code":"DEF.INVALID_DEFINITION","diagnostics":[..]}` | stderr, JSON 한 줄 | 1 |
| 그 밖의 실패 | `{"ok":false,"code":"SOURCE_IO",..,"message":..}`(`diagnostics` 없음) | stderr, JSON 한 줄 | 1 |
| 명령행 사용법 오류 | clap 평문(JSON 아님) | stderr | 2 |

종료 코드는 기계 필드다. 0 수용, 1 보고된 실패(JSON이 stderr), 2 사용법 오류. 시험: `the_exit_code_of_check_says_accepted_rejected_or_misused`. `BrokenPipe`(stdout이 닫힘)는 여전히 0이다(G10, 소유자 결정 D-E6).

## 2. 정의 오류 `diagnostics[]`

| 필드 | 분류 | 존재 | 설명 |
|---|---|---|---|
| `code` | 기계 | 항상 | `DEF.<이름>`. 의미는 바뀌지 않는다. `aip explain-code <code>`로 설명을 얻는다 |
| `severity` | 기계 | 항상 | 현재 `"error"`뿐. 값 집합은 열려 있다 |
| `line`, `column` | 기계 | 항상 | 정수. 1 기준, 열은 Unicode 코드 포인트, 탭은 1칸, `\r\n`은 한 줄. **위치가 없으면 0과 0**(기존 동작 그대로) |
| `located` | 기계, **추가** | 항상 | 위치가 있으면 `true`, 정의 전체에서 나와 위치가 없으면 `false`. `false`일 때만 `line`·`column`이 0이다 |
| `anchor` | 기계, **추가** | `located`가 `true`일 때 | `line:<줄>:col:<열>`. 경고의 `advisories[].anchor`와 같은 문법이라 오류와 경고의 위치를 같은 방식으로 읽는다 |
| `scopePath` | 기계, **추가** | `scope`가 있을 때 | `scope`를 `>` 기준으로 자른 배열(`["resource Recruitment","expose read"]`). 조각의 첫 단어는 문법 키워드다. 소스 텍스트에서 중괄호를 따라 얻으므로 호스트 리터럴 형식(`.h.ts`, `.h.py`)에는 없다 |
| `message` | 설명 | 항상 | 문구는 바뀐다. 기계가 해석하지 않는다 |
| `help` | 설명 | 선택 | 가까운 철자, 블록이 받는 키, 고치는 방법. 영어. 같은 코드의 반복에서는 수리법을 생략한다 |
| `scope` | 설명 | 선택 | 위치한 선언 경로 문장. 기계는 `scopePath`를 쓴다 |
| `explain` | 설명 | 코드가 레지스트리에 있을 때 | 명령어 문자열. 실행하지 말고 `code`에서 만든다 |

위치 없음의 표현은 "0:0 유지 + `located:false` 추가"로 구현했다. 1:1로 바꾸는 안은 기존 소비자의 값을 바꾸므로 구현하지 않았고 `decisions/e.md` D-E3에 선택지를 남겼다.

위치를 알 수 있는 진단은 실제 좌표를 갖는다(2라운드에서 채움): 리터럴의 잘못(`DEF.BAD_VALUE`, 조건이 Bool이 아닌 `DEF.TYPE_MISMATCH` 등)은 리터럴, `DEF.SOURCE_TOO_LARGE`는 상한을 넘기는 첫 문자, `DEF.MULTIPLE_BLOCKS`는 두 번째 `aip`/`define`, `DEF.WRONG_BINDING`은 남는 import 또는 첫 사용. 지금도 0:0인 것은 가리킬 텍스트가 없는 `DEF.MISSING_ITEM`(actor 선언 없음), `DEF.NO_BLOCK`, `DEF.ENCRYPTION_RUNTIME_UNAVAILABLE`(실행 facts에서 나옴)이다. 시험: `diagnostic_fields.rs`의 `a_bad_literal_points_at_the_literal`, `a_source_over_the_size_limit_points_at_the_byte_that_crosses_it`, `a_second_block_in_an_embedded_file_points_at_the_second_block`, `a_diagnostic_about_the_whole_definition_stays_without_a_position`.

## 3. 경고 `advisories[]`

| 필드 | 분류 | 존재 | 설명 |
|---|---|---|---|
| `code` | 기계 | 항상 | `DEF.ADVISORY_*` |
| `severity` | 기계 | 항상 | 현재 `"warning"`뿐 |
| `anchor` | 기계 | 항상 | `line:<n>:col:<m>`(컴파일러가 낸 경고) 또는 `predicate:<이름>`, `resource:<이름>`, `transition:<resource>.<전이>`, `create:<resource>`(`--dev-checks`) |
| `line`, `column` | 기계, **추가**(G2) | 컴파일러가 위치를 보고한 경고 | 오류와 같은 좌표. `--dev-checks`의 선언 단위 경고에는 없다(실행 facts에 위치가 없다) |
| `explain` | 설명, **추가** | 코드가 레지스트리에 있을 때 | 오류와 같다 |
| `message` | 설명 | 항상 | |

경고에는 `scope`·`help`가 없다(경고는 `enrich`를 거치지 않는다). 이번에 추가하지 않았다.

## 4. 정의 오류가 아닌 실패

| 필드 | 분류 | 존재 | 설명 |
|---|---|---|---|
| `ok` | 기계 | 항상 | `false` |
| `code` | 기계(잠정) | 항상 | `SOURCE_IO`, `UNSUPPORTED_FORM`, `INPUT_TOO_LARGE` 등. 영역 접두가 없고 레지스트리에 없다(G5, D-E5) |
| `reason` | 기계, **추가** | 한 코드가 여러 종류를 덮는 곳 | 파일 읽기 실패: `OPEN_FAILED`, `NOT_A_REGULAR_FILE`, `READ_FAILED`, `NOT_UTF8`, 상한 초과 `OVER_LIMIT`(코드 `INPUT_TOO_LARGE`). 문구를 읽지 않고 종류를 가른다 |
| `message` | 설명 | 항상 | |

`SOURCE_IO` 하나가 네 가지 실패(열 수 없음, 일반 파일 아님, 읽기 실패, UTF-8 아님)를 덮고 있었고 구분은 문구뿐이었다. 코드를 바꾸면 기존 소비자가 깨지므로 `reason`을 더했다. `diagnostics`가 있으면 정의 거부, 없으면 이 모양이다.

## 5. 코드 레지스트리 (`aip diagnostics --area def --json`, G6)

기존 키(`area`, `code`, `example`, `explain`, `fix`, `kind`, `title`)는 그대로이고 다음이 추가됐다.

| 필드 | 존재 | 설명 |
|---|---|---|
| `severity` | 항상 | `DEF.ADVISORY_*`는 `warning`, 나머지는 `error` |
| `status` | 항상 | `active` 또는 `retired`. 폐기된 코드는 삭제하지 않고 레지스트리에 남는다 |
| `since` | 알 때만 | 처음 낸 릴리스. 이 필드가 생기기 전 이력은 기록이 없어 지금은 어느 코드에도 없다 |
| `replacedBy` | 알 때만 | 폐기된 코드를 대신하는 코드 |

시험: `every_registered_code_has_a_severity_and_a_status`, `a_published_code_is_never_removed_from_the_registry`(`tests/published_codes.txt`는 현재 레지스트리에 있는 코드 목록이고 시험은 목록의 코드가 레지스트리에서 사라졌는지만 본다). `every_code_is_registered_and_none_is_dead`는 `retired` 코드를 "발생지 없음" 검사에서 뺀다.

## 6. 소비자가 지킬 규칙(제안)

- 알 수 없는 필드는 무시한다. 닫힌 스키마(`additionalProperties: false`)로 검증하지 않는다. `service.rs`의 `contract` 검사가 이 규칙을 시험으로 고정한다(알 수 없는 필드 추가에는 통과, 필수 필드 제거·타입 변경에는 실패).
- 분기에는 `code`, `severity`, `reason`, `located`, `anchor`, `scopePath`, 종료 코드를 쓴다. `message`, `help`, `scope`, `explain`은 사람에게 보이는 용도다.
- 알 수 없는 `code`는 담긴 컨테이너(`diagnostics`는 오류, `advisories`는 경고)로 다룬다.

## 7. 문구를 읽어야만 구분되는 오류 종류 (남은 것)

처리한 것: 파일 읽기 실패 4종(`reason`, 위 4절), G9의 `DEF.ACCESS_TARGET_NOT_EXPOSED`.

남은 것. 아래 코드는 한 코드가 서로 다른 조치를 요구하는 상황을 덮는다. 발생지 수는 `grep -o '"DEF\.…"'`로 센 값이다(2026-11-22).

| 코드 | 발생지 수(파일) | 비고 |
|---|---|---|
| `DEF.TYPE_MISMATCH` | 33(sema) | sema.rs |
| `DEF.DUPLICATE` | 33(sema 27, parser 5, hlit 1) | sema.rs 포함 |
| `DEF.UNKNOWN_SYMBOL` | 24(sema, G9의 두 곳 제외) | 철자 제안은 `help`에 있다 |
| `DEF.MISSING_ITEM` | 26(sema 22, hmap 3, parser 1) | 시험이 문구로 구분한다(`diagnostics.rs`의 `message.contains("bulk maxRows")` 등). sema.rs 포함 |
| `DEF.NON_LITERAL` | 24(hlit 19, extract 5) | E 소유 파일. 소비자 없음 |
| `DEF.UNSUPPORTED` | 22(sema) | sema.rs |
| `DEF.UNKNOWN_FIELD` | 22(sema) | 철자 제안은 `help`에 있다 |
| `DEF.H_SHAPE` | 22(hmap) | 호스트 리터럴 형식. 소비자 없음 |

저장소 안에서 문구로 분기하던 소비자는 `.github/workflows/flaskbb-post-order.yml`(`"expose apply" in d["message"]`) 하나였다. 그 발생지(쓰기 확장과 job의 access가 `expose apply`로 공개되지 않은 전이를 가리킴) 두 곳은 전용 코드 `DEF.ACCESS_TARGET_NOT_EXPOSED`를 낸다(2라운드, D-E4(a)). `DEF.UNKNOWN_SYMBOL`은 나머지 발생지에서 의미가 같다. workflow 전환은 F1 몫이다(`track-e.md` 7절의 diff). 소비자가 없는 나머지(`DEF.NON_LITERAL`, `DEF.H_SHAPE` 등)에 코드를 새로 쪼개지는 않았다. 소비자 없이 코드 공간을 늘리면 의미 고정 비용만 늘기 때문이다.
