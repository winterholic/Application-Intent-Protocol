# 진단 JSON 최소 기계 판독 계약 (설계안)

- 상태: 초안. 소유자 검토 전이며 최종 범위는 소유자가 확정한다.
- 기준: `origin/main` 0c5d108d, 측정일 2026-11-21.
- 범위: 설계 문서만이다. 진단 출력 코드, 시험, `COMPATIBILITY.md`의 호환 약속은 이 작업에서 바꾸지 않았다. 4절의 "보장"은 이 문서가 제안하는 후보이고 현재 약속이 아니다.
- 소유자 결정(작업 카드): 진단 JSON 전체 내부 구조를 1.0 호환 보장 대상으로 확정하지 않는다. 외부 도구가 안정적으로 쓸 최소 계약만 설계하고 메시지 문구와 내부 분석 정보까지 불필요하게 고정하지 않는다.
- 근거 표기: 측정은 `CARGO_TARGET_DIR=/tmp/aip-target-diag`로 만든 `aip` 바이너리(기준 커밋)에 fixture를 먹인 실제 출력이다. 코드를 읽어 추론한 것은 "추론"으로, 확인하지 못한 것은 "확인 필요"로 적었다.

## 1. 결론 요약

1. 계약의 대상은 `aip service check <file>`의 출력과, 정의를 읽는 모든 명령이 공유하는 실패 봉투(`DEF.INVALID_DEFINITION` + `diagnostics`)다.
2. 보장 후보는 일곱 가지다. 종료 코드의 의미, JSON이 놓이는 스트림, `ok`, 진단과 경고의 `code`와 `severity`, `line`과 `column`의 좌표 규칙, `message`의 존재와 타입, 스키마 버전 표지(`schema`, 없으면 1)다.
3. 변경 가능으로 두는 것은 `message`의 문구, `help`, `scope`, `explain`, `anchor`의 비 line 형식, `executionDigest`의 값, 키 순서와 공백, 배열 순서다.
4. 위치는 1 기준이고 열은 Unicode 코드 포인트(scalar value) 단위다. 현재 구현이 이미 이렇게 동작하며(5.2절 측정) 바꿀 필요가 없다.
5. 현재 구현은 이 계약을 일부 만족하지 못한다. 가장 큰 차이는 위치 없는 진단이 `line: 0, column: 0`을 내는 점, 경고에 `line`·`column`이 없고 `anchor` 문자열뿐인 점, 키 집합 정확 비교 시험이 가산 변경을 막는 점, 버전 표지가 없는 점, 레지스트리에 폐기 상태를 둘 수 없는 점이다(7절, G1부터 G10).
6. 저장소 안의 실제 소비자가 이미 메시지 문구를 매칭한다(`flaskbb-post-order.yml`의 `"expose apply" in d["message"]`). 문구를 고정하지 않으려면 이 소비자가 코드만 보고 판정할 수 있게 코드 입도를 먼저 조정해야 한다(G9).

## 2. 현재 출력 전수

### 2.1 측정한 fixture

모든 출력의 원문은 부록 A에 있다. 명령은 `aip service check <file>`이고 `aip`는 위 바이너리다.

| 이름 | 입력 | 종류 | 종료 코드 | stdout | stderr |
| --- | --- | --- | --- | --- | --- |
| ok_min | `actor`와 `resource` 하나뿐인 정의 | 정상(경고 없음) | 0 | 158B JSON | 0B |
| recruitment | `spikes/spike-v1-fixture/fixture/recruitment.aip` | 정상 + 경고 1건 | 0 | 660B JSON | 0B |
| recruitment_dev | 같은 파일, `--dev-checks` | 정상 + 경고 1건 | 0 | 660B JSON | 0B |
| flaskbb, facereview | `porting/flaskbb/app.aip`, `facereview.aip` | 경고 1건, 5건 | 0 | JSON | 0B |
| unused_pred_dev | 쓰이지 않는 predicate, `--dev-checks` | 경고(비 line anchor) | 0 | 323B JSON | 0B |
| unreachable_dev | budget 없는 `expose read`, `--dev-checks` | 경고(비 line anchor) | 0 | 417B JSON | 0B |
| f1_unknown_key | recruitment에서 `sort`를 `sorts`로 바꿈 | 실패, 진단 1건 | 1 | 0B | 497B JSON |
| f2_cascade | 타입 오타와 이름 오타 | 실패, 진단 2건 | 1 | 0B | 856B JSON |
| g1.e.ts, g2.e.py | recruitment의 내장 형식에서 같은 오타 | 실패(호스트 파일 좌표) | 1 | 0B | JSON |
| g3.h.ts | 호스트 리터럴 형식에서 같은 오타 | 실패(`scope` 없음) | 1 | 0B | JSON |
| wrong_lang | `.aipoc` 파일 | 실패(`DEF.WRONG_LANGUAGE`) | 1 | 0B | 508B JSON |
| empty | 빈 `.aip` | 실패, 위치 0:0 | 1 | 0B | 324B JSON |
| noblock | 블록 없는 `.e.ts` | 실패, 위치 0:0 | 1 | 0B | 344B JSON |
| missing, bad_utf8, unsupported_form, big | 없는 파일, 잘못된 UTF-8, `.txt`, 1 MiB 초과 | 실패, `diagnostics` 없는 봉투 | 1 | 0B | 70B 안팎 JSON |
| tok | 토큰 6만 5536개 초과 | 실패(`DEF.TOKEN_LIMIT`) | 1 | 0B | 409B JSON |
| clap 오류 | 인자 없음, 없는 플래그 | 사용법 오류 | 2 | 0B | 평문 `error: ...` |

공통 형태: JSON은 한 줄이고 끝에 개행이 하나 붙으며, 키는 알파벳 순으로 나온다(`serde_json`의 기본 정렬. 추론). 같은 입력을 세 번 돌려 출력의 SHA-256이 같았다(`f2_cascade`).

### 2.2 필드 목록

성공 출력(stdout, 종료 0). 근거는 `crates/aip-service/src/lib.rs:1064`(Check)와 `spikes/spike-v11-dev-checks/src/lib.rs`다.

| 경로 | 타입 | 존재 | 관측값 | 비고 |
| --- | --- | --- | --- | --- |
| `ok` | bool | 항상 | `true` | |
| `executionDigest` | string | 항상 | 64자 소문자 hex | 실행 facts의 SHA-256 |
| `advisories` | array | 항상 | 0건에서 5건 | 경고. 정의를 거부하지 않는다 |
| `advisories[].code` | string | 항상 | `DEF.ADVISORY_PUBLIC_COUNT` | 레지스트리에 있는 코드 9개 중 하나(`DEF.ADVISORY_*`) |
| `advisories[].severity` | string | 항상 | `"warning"` | 코드에 상수로 박혀 있다 |
| `advisories[].message` | string | 항상 | 한국어 문장 | |
| `advisories[].anchor` | string | 항상 | `line:60:col:3`, `predicate:orphan`, `resource:Tag` | 형식이 다섯 가지다(아래) |
| `workers` | object | 항상 | | |
| `workers.required` | bool | 항상 | `true`, `false` | |
| `workers.extensions` | array | 항상 | | 빈 배열일 수 있다 |
| `workers.extensions[].name` | string | 항상 | `Recruitment.stats` | |
| `workers.extensions[].implementation` | string | 항상 | `recruitment.stats` | |
| `workers.extensions[].kind` | string | 항상 | `read` | 다른 값의 전체 집합은 확인 필요 |

`anchor`의 형식은 코드로 확인한 다섯 가지다. `line:<n>:col:<m>`(컴파일러가 낸 경고), `predicate:<이름>`, `resource:<이름>`, `transition:<resource>.<전이>`, `create:<resource>`(나머지 넷은 `--dev-checks`로 켜지는 경고, `spikes/spike-v11-dev-checks/src/lib.rs:65,91,154,160`). 기본 실행에서 나오는 경고는 `ALWAYS_ON` 네 코드이고 모두 `line:` 형식이다(같은 파일 `ALWAYS_ON`, 관측: recruitment, flaskbb, facereview).

실패 출력, 정의 오류(stderr, 종료 1). 근거는 `crates/aip-service/src/lib.rs:566-587`과 `spikes/spike-v1-fixture/src/diag.rs:65-83`이다.

| 경로 | 타입 | 존재 | 관측값 | 비고 |
| --- | --- | --- | --- | --- |
| `ok` | bool | 항상 | `false` | |
| `code` | string | 항상 | `DEF.INVALID_DEFINITION` | 정의 오류 봉투에서는 고정값 |
| `diagnostics` | array | 항상(이 봉투) | 1건 이상 | |
| `diagnostics[].code` | string | 항상 | `DEF.PARSE_UNKNOWN_KEY` | |
| `diagnostics[].severity` | string | 항상 | `"error"` | 상수 |
| `diagnostics[].message` | string | 항상 | 한국어 문장 | |
| `diagnostics[].line` | integer | 항상 | 56 | 위치 없는 진단은 0 |
| `diagnostics[].column` | integer | 항상 | 5 | 위치 없는 진단은 0 |
| `diagnostics[].scope` | string | 선택 | `resource Recruitment > expose read` | 호스트 리터럴 형식과 `DEF.WRONG_LANGUAGE`에서 없었다 |
| `diagnostics[].help` | string | 선택 | 영어 문장, `fix:`로 시작하는 경우가 많다 | 코드상 선택(`diag.rs` `set_help`). 관측한 실패 fixture 전부에는 있었다 |
| `diagnostics[].explain` | string | 코드가 레지스트리에 있을 때 | `aip explain-code DEF.PARSE_UNKNOWN_KEY` | 명령어 문자열이다 |

실패 출력, 정의 오류가 아닌 것(stderr, 종료 1). `aip-service`가 `error(code, message)`로 만든다(`lib.rs:483`).

| 경로 | 타입 | 존재 | 관측값 |
| --- | --- | --- | --- |
| `ok` | bool | 항상 | `false` |
| `code` | string | 항상 | `SOURCE_IO`, `UNSUPPORTED_FORM`, `INPUT_TOO_LARGE` |
| `message` | string | 항상 | 한국어 문장 |
| `diagnostics` | 없음 | 없음 | |

이런 코드는 `aip-service/src/lib.rs`에서 서로 다른 문자열이 62개 보인다(`grep -o 'error("[A-Z_]*"'`의 중복 제거 개수). 영역 접두(`DEF.` 등)가 없고 `aip explain-code SOURCE_IO`는 `unknown code`로 종료 코드 1을 낸다(관측).

### 2.3 채널과 종료 코드 (관측)

| 상황 | 종료 코드 | 어디에 무엇이 나오는가 |
| --- | --- | --- |
| 정의 수용(경고 있어도) | 0 | stdout에 JSON 한 줄. stderr 비어 있음 |
| 정의 거부, 파일 오류, 입력 상한 | 1 | stderr에 JSON 한 줄. stdout 비어 있음 |
| 명령행 사용법 오류(clap) | 2 | stderr에 평문. JSON 아님 |
| stdout이 닫힌 경우(`BrokenPipe`) | 0 | 출력이 전달되지 않았어도 성공 처리(`lib.rs:1787`, 추론) |

경고는 종료 코드를 바꾸지 않는다(flaskbb 경고 1건에서 종료 0).

### 2.4 위치 필드가 실제로 세는 것 (측정)

같은 줄에 `@`(허용되지 않는 문자)를 두고 `DEF.LEX_UNEXPECTED_CHAR`의 `column`을 비교했다. 아래 "문자 기준 열"은 줄 안에서 `@` 앞의 코드 포인트 수에 1을 더한 값이다.

| 앞 내용 | 보고된 열 | 문자 기준 열 | 바이트 기준 열 | UTF-16 기준 열 |
| --- | --- | --- | --- | --- |
| ASCII만 | 47 | 47 | 47 | 47 |
| 한글 3자 문자열 | 61 | 61 | 67 | 61 |
| 이모지(U+1F600) 문자열 | 59 | 59 | 62 | 60 |
| 탭 들여쓰기 | 30 | 탭 1칸 | | |
| CRLF 줄바꿈 | 47 | 47 | | |
| `e` + 결합 부호 U+0301 | 60 | 코드 포인트마다 1칸 | | |
| 파일 앞 BOM(U+FEFF) | 1열, `DEF.LEX_UNEXPECTED_CHAR` | | | |

결론: 열은 1 기준, Unicode 코드 포인트 단위, 탭은 1칸이다. 줄은 1 기준이고 `\n`으로 센다. 구현은 `spikes/spike-v1-fixture/src/lexer.rs:42-56`이 `Vec<char>`를 도는 방식이다.

내장 형식은 호스트 파일 좌표다. `g1.e.ts`는 59줄 7열, `g2.e.py`는 59줄 9열로 나왔고 `grep -n`으로 본 `sorts`의 줄과 같고 열은 들여쓰기 공백 수 더하기 1과 같다. 호스트 리터럴 형식(`g3.h.ts`)은 55줄 16열이며, 이는 키 `sorts`(9열)가 아니라 값 `[`의 위치다. 즉 같은 오류도 형식에 따라 가리키는 토큰이 다르다.

## 3. 외부 도구의 사용 사례와 유지 비용

### 3.1 저장소 안에서 확인한 실제 소비자

| 소비자 | 읽는 것 | 위치 | 이 문서에 주는 의미 |
| --- | --- | --- | --- |
| flaskbb CI 단계 | 종료 코드 1, `ok is False`, `diagnostics[].code == "DEF.UNKNOWN_SYMBOL"`, 같은 항목의 `message`에 `"expose apply"` 부분 문자열(`2>&1`로 합쳐 읽음) | `.github/workflows/flaskbb-post-order.yml:208-213` | 코드와 종료 코드뿐 아니라 메시지 문구를 이미 매칭한다. 한 코드가 여러 상황을 덮어 문구로 구분하고 있다 |
| infisical 계약 시험 | 성공 시 stdout JSON의 `ok`와 `advisories[].code`, 실패 시 종료 코드 비영과 stderr 텍스트 안의 코드 문자열 | `porting/infisical/tests/test_encryption_contract.py:32-81` | `code`와 경고 코드 집합에 의존한다. `--dev-checks`의 경고 코드도 읽는다 |
| 앱 패키저 | 성공 stdout JSON의 `workers.required`, `workers.extensions[].name`, `.implementation` | `product/tools/app-package.mjs:33-47,156` | `workers`는 진단이 아니지만 같은 JSON의 소비자다. JSON 파싱 실패 시 조용히 검증을 건너뛴다(fail-open) |
| 아카이브·패키지 스크립트 | 종료 코드만 | `scripts/package-cli-archive.sh:40`, `product/tests/app-package-e2e.test.mjs:55` | 종료 코드 의미가 가장 값싸고 가장 널리 쓰인다 |
| CLI 시험 | 키 집합, 타입, `line` 56, `column` 5, 코드, severity, 종료 코드 | `crates/aip-cli/tests/service.rs:21-63` | 현재 고정의 범위 |

### 3.2 저장소 안에서 구현을 찾지 못한 사용 사례

편집기와 AI 에이전트가 쓰는 구현은 이 저장소에서 찾지 못했다(`service check`를 부르거나 `advisories`, `diagnostics`, `executionDigest`를 읽는 mjs, py, ts, yml, sh 파일을 검색한 범위에서는 위 5종뿐이었고 편집기 확장이나 LSP 서버는 없었다. 확인 필요: 저장소 밖 구현). 따라서 아래는 사례가 아니라 설계 근거가 되는 요구다.

- 편집기: 파일 경로와 `line`, `column`으로 밑줄을 그리고 `code`로 필터링한다. 필요한 것은 좌표 단위의 명확함이다. LSP는 0 기준이고 위치 인코딩을 협상할 수 있다. 코드 포인트 단위는 LSP 3.17의 `utf-32` 인코딩과 같은 뜻이다(LSP 명세 내용으로 이 세션에서는 재확인하지 않았다. 확인 필요).
- AI 에이전트: 수정할 위치와 코드를 읽고 `aip explain-code <code>`로 설명을 찾아 고친 뒤 다시 검사한다. 필요한 것은 안정된 `code`와 "첫 항목부터 고친다"는 관례다(`spec/definition-diagnostics.md`의 `DEF.INVALID_DEFINITION` 수정 항목이 같은 말을 한다). `explain` 필드는 실행할 명령어 문자열이므로 도구가 그대로 실행하면 출력 문자열을 셸에 넘기는 셈이 된다. 코드에서 명령을 직접 만들어야 한다.
- CI: 종료 코드로 게이트하고 `code`나 `severity`로 예외 목록을 둔다. 경고를 실패로 처리하는 CI가 있을 수 있음을 `COMPATIBILITY.md`도 인정한다(정책 경고 3종 문단).

### 3.3 필드를 고정할 때 드는 유지 비용

| 비용 | 내용 | 실제 근거 |
| --- | --- | --- |
| 개선 막힘 | `message`와 `help`는 사용자 개선의 주 대상이다. 고정하면 철자 제안과 문구 교정이 major 사건이 된다 | `DEF.ACCESS_MISMATCH` 문구를 1.0 준비 중에 바꿨다(`COMPATIBILITY.md` 95행). `enrich`가 `help`를 코드별로 만든다(`enrich.rs`) |
| 언어 혼재 | `message`는 한국어, `help`는 영어, 일부 `message`는 영어다(`DEF.WRONG_LANGUAGE`). 고정하면 이 혼재도 고정된다 | 부록 A |
| 내부 정보 노출 | `scope`는 선언 경로 문자열이고 내부 분석(`enrich`)의 결과다. 분석 방식이 바뀌면 값이 바뀐다 | `diag.rs` 주석: scope와 help는 `enrich`가 나중에 더한다 |
| 시험 마찰 | 키 집합 정확 비교는 선택 필드 추가마다 시험 수정을 요구한다. `severity` 추가가 이미 한 번 키 집합을 바꿨다 | `service.rs:28,33,49,55`, `COMPATIBILITY.md` 진단 JSON 항 |
| 값 안정성 | `executionDigest`의 값은 실행 facts의 정체성이라 진단 계약과 수명이 다르다 | `COMPATIBILITY.md`의 V-1(digest 불변) 서술 |

반대로 `code`, `severity`, `line`, `column`, 종료 코드는 바꿀 이유가 적고 소비자가 이미 읽는다. 고정 비용이 작고 소비자 이득이 크다.

## 4. 필드별 분류

범례: 보장 = 같은 major(1.x) 안에서 제거, 타입 변경, 의미 변경을 하지 않는다. 변경 가능 = 존재, 형식, 내용이 바뀔 수 있고 도구가 의존하면 안 된다.

| 필드 | 분류 | 보장 내용 | 근거(사용처, 비용) |
| --- | --- | --- | --- |
| 종료 코드 | 보장 | 0은 정의 수용, 1은 보고된 실패(JSON이 stderr), 2는 명령행 사용법 오류(JSON 아님) | 모든 소비자가 읽는다. 비용이 가장 낮다 |
| 스트림 위치 | 보장 | 성공 JSON은 stdout, 실패 JSON은 stderr에 한 줄. 실패 시 stdout은 비어 있다 | 시험이 이미 고정(`service.rs:47`). 성공 시 stderr 비어 있음은 보장하지 않는다(미래 사용 중단 안내용 여지) |
| `ok` | 보장 | bool. 종료 0이면 `true`, 아니면 `false` | 시험, infisical, flaskbb가 읽는다 |
| 실패 `code` | 조건부 보장 | string. 정의 오류 봉투는 `DEF.INVALID_DEFINITION` 고정. 그 밖의 값(`SOURCE_IO` 등)은 영역 접두가 없는 잠정 값이며 보장하지 않는다 | G5 |
| `diagnostics` | 보장 | 정의 오류 봉투에서 비어 있지 않은 배열 | 추론: `Err(Vec<Diag>)` |
| `diagnostics[].code`, `advisories[].code` | 보장 | `DEF.`로 시작하는 레지스트리 코드 문자열. 의미는 5.1절 규칙에 따라 고정 | CI·시험이 매칭한다. 이 문서의 중심 보장 |
| `diagnostics[].severity` | 보장(값 집합은 열림) | string. 현재 `"error"`. 새 값이 생길 수 있다 | 소비자는 모르는 값을 5.4절대로 처리한다 |
| `advisories[].severity` | 보장(값 집합은 열림) | string. 현재 `"warning"` | 같다 |
| `diagnostics[].line`, `.column` | 보장 | 정수, 1 이상, 5.2절의 좌표. 어느 토큰을 가리키는지는 보장하지 않는다 | 편집기가 필요로 한다. 단 현재 0이 나오는 경우가 있다(G1) |
| `diagnostics[].message`, `advisories[].message` | 보장은 존재와 타입뿐 | 비어 있지 않은 string. 문구, 언어, 길이는 바뀐다. 기계가 해석하지 않는다 | 표시용. 고정 비용이 가장 크다 |
| `diagnostics[].scope` | 변경 가능 | 선택 string. 없을 수 있다 | 내부 분석 정보 |
| `diagnostics[].help` | 변경 가능 | 선택 string | 개선 대상 |
| `diagnostics[].explain` | 변경 가능 | 선택 string. 실행하지 말고 `code`에서 명령을 직접 만든다 | `code`로 도출되는 중복 정보 |
| `advisories[].anchor` | 부분 보장 | 비어 있지 않은 string. 기본 실행의 경고는 `line:<n>:col:<m>` 문법이 보장된다. 그 밖의 형식은 불투명 식별자다 | G2. 기본 실행 경고는 현재 모두 `line:` |
| `executionDigest` | 형식만 보장 | 성공 시 64자 소문자 hex. 값의 동일성은 이 계약이 아니라 배포 digest 계약이 정한다 | 값 안정성은 `COMPATIBILITY.md` V-1이 다룬다 |
| `workers.required`, `workers.extensions[].name`, `.implementation` | 보장(단, 진단 계약 밖 항목으로 소유자 확인) | bool, array, string | 앱 패키저가 읽는다. 진단이 아니어서 포함 여부를 소유자가 정한다 |
| `workers.extensions[].kind` | 변경 가능 | | 소비자 없음, 값 집합 확인 필요 |
| 키 순서, 공백, 줄바꿈 | 변경 가능 | 한 줄 JSON을 JSON 파서로 읽는다 | 현재 알파벳 순 한 줄 |
| 배열 순서 | 변경 가능(관례) | 같은 엔진, 같은 입력이면 같은 순서(관측: 3회 동일). `diagnostics[0]`부터 고치라는 관례는 유지하되 보장하지 않는다 | 소유자 결정 항목(8절) |
| 알 수 없는 추가 필드 | 허용 | 5.4절 | |

## 5. 규칙 제안

### 5.1 진단 코드의 안정성

현재 규칙은 `spec/definition-diagnostics.md` 머리글의 한 문장("한 번 공개된 코드는 의미를 바꾸지 않는다")과 `COMPATIBILITY.md`의 "오류 코드 삭제는 major, 추가는 minor"다. 후자는 `spec/wire-errors.md`를 이름으로 지목하고 `DEF` 레지스트리를 지목하지 않는다. `DEF` 쪽은 시험으로 지켜지지도 않는다. 아래를 제안한다.

1. 코드 형식. `DEF.` 다음에 대문자, 숫자, 밑줄(대문자로 시작). 경고는 `DEF.ADVISORY_` 접두를 유지한다. 한 번 쓴 코드 문자열은 다른 뜻으로 다시 쓰지 않는다.
2. 새 코드 추가. 새 코드 자체는 minor에서 허용한다. 소비자는 모르는 코드를 5.4절대로 처리해야 한다.
3. 새 거부 규칙. 지금까지 수용되던 정의를 새로 거부하는 변경은 코드 추가와 별개의 호환 사건이다. patch에서는 하지 않고 minor 이상에서 `COMPATIBILITY.md`의 "출력이 달라지는 것"에 기록한다. 기본은 먼저 경고로 낸다. 이 방식은 정책 경고 3종의 선례(`p1-sema-notes.md` 2절)와 같고, 거부로 낸 선례(`DEF.CALLER_FILTER_POLICY_REQUIRED`)는 저장소 정의 36개로 영향을 확인했다(`COMPATIBILITY.md`).
4. 의미 변경. 같은 major 안에서 코드가 가리키는 조건을 바꾸지 않는다. 거짓 양성을 줄이는 좁힘은 허용하고 minor에 기록한다. 넓힘은 3번으로 다룬다. 의미를 바꾸고 싶으면 새 코드를 추가하고 옛 코드를 폐기한다.
5. 폐기. 폐기는 "더 이상 내지 않음"이고 "레지스트리에서 삭제"가 아니다. 폐기된 코드는 `status: retired`와 `since`, 가능하면 `replacedBy`를 달고 레지스트리에 영구히 남는다. 내지 않게 되는 사유가 (가) 입력이 이제 수용됨이면 minor에서 허용한다. (나) 같은 입력이 다른 코드로 거부됨이면 코드를 매칭하는 소비자가 깨지므로 major다. 레지스트리에서의 삭제와 코드 문자열 재사용은 하지 않는다.
6. 심각도 변경. 경고에서 오류로 올리는 것은 3번(새 거부)으로 다룬다. 오류에서 경고로 내리는 것은 minor에서 허용한다.
7. 문구. `message`, `help`, `scope`의 변경은 patch에서도 허용한다. 문구 변경으로 알리는 별도 약속은 하지 않는다.
8. 코드 입도. 한 코드가 서로 다른 조치를 요구하는 상황을 덮고 있어 소비자가 문구로 구분하고 있다면(3.1절의 flaskbb CI) 코드를 나누는 것이 문구를 고정하는 것보다 싸다. 나누기는 4번의 의미 변경이므로 새 코드를 추가하는 방식으로 하고, 이 전환은 한 번만 major에서 한다(G9).

### 5.2 위치의 기준

| 항목 | 규칙 | 근거 |
| --- | --- | --- |
| 기준 | `line`과 `column` 모두 1 기준 정수. 보고하는 값은 항상 1 이상 | 현재 출력과 `render`(`file:line:col`)가 같은 기준. 위치 없는 진단만 0(G1) |
| 줄 | `\n`으로 센다. `\r\n`도 한 줄 | 2.4절 CRLF 측정 |
| 열 | 줄 시작부터의 Unicode 코드 포인트 수 더하기 1. 탭은 1칸. 결합 부호도 1칸 | 2.4절 측정 |
| 좌표계 | 인자로 받은 파일의 좌표. 내장 형식(`.e.ts`, `.e.py`)은 호스트 파일의 줄과 열 | 2.4절 `g1`, `g2` |
| 위치의 정밀도 | 문제 구성요소 안이나 근처. 정확히 어느 토큰인지는 보장하지 않는다. 형식(A, E, H)마다 가리키는 토큰이 다를 수 있다 | 2.4절 `g3`가 값의 위치를 가리킴 |
| 끝 위치 | 범위(`endLine`, `endColumn`)는 계약에 넣지 않는다. 나중에 선택 필드로 더할 수 있다 | 5.4절의 가산 규칙 |
| 위치 없음 | 위치가 정의되지 않는 진단은 1줄 1열로 낸다 | 아래 기각 항목 참고. 현재는 0:0이다(G1) |
| 파일 이름 | 필드로 싣지 않는다. 계약 대상 명령이 파일 하나를 받으므로 호출자가 안다 | 6절 |

경고의 위치는 현재 `anchor` 문자열(`line:<n>:col:<m>`)로만 나오며 같은 좌표 규칙을 따른다. 경고에도 정수 `line`과 `column`을 선택 필드로 더하는 것을 제안한다(G2).

### 5.3 버전 표시

1. 최상위에 `schema` 문자열을 하나 더한다. 값은 `aip-diagnostics/1`이다. 숫자는 호환 경계인 major뿐이다.
2. 성공 출력과 실패 출력 양쪽에 둔다. 같은 형식이 아닌 실패 봉투(`error()`)에도 둔다.
3. 필드가 없으면 `aip-diagnostics/1`로 해석한다. 그래서 1.0 바이너리와 이 필드를 더한 뒤의 1.x 바이너리가 같은 소비자 규칙으로 읽힌다. 이 필드 추가는 5.4절의 가산 변경이다.
4. major는 보장 필드의 제거, 타입 변경, 의미 변경, 5.1절 5번 (나) 같은 코드 의미 변경이 있을 때만 올린다. 그런 변경은 엔진 major와 함께 하는 것을 원칙으로 한다.
5. `PROTOCOL_VERSION`과 `CORE_IR_VERSION`은 이 값과 별개다. 엔진, 프로토콜, IR, 매니페스트, 패키지 형식 표지가 이미 따로 움직인다는 점이 `COMPATIBILITY.md`의 구조다.

### 5.4 알 수 없는 필드와 값의 처리

생산자(엔진):

- 같은 major 안에서 어느 객체에든 선택 필드를 더할 수 있다. 보장 필드를 제거하거나 타입을 바꾸지 않는다.
- 필드가 없을 수 있다고 적은 필드(`scope`, `help`, `explain`)는 계속 없을 수 있다.

소비자(CI, 편집기, 에이전트):

- 알 수 없는 필드를 무시한다. 닫힌 스키마(`additionalProperties: false`)로 검증하지 않는다.
- 알 수 없는 `code`는 `severity`와 `message`로 다룬다. 오류 컨테이너(`diagnostics`)에 있으면 오류로, `advisories`에 있으면 경고로 취급한다.
- 알 수 없는 `severity`는 담긴 컨테이너에 따라 오류 또는 경고로 취급한다. 모르는 값을 성공으로 해석하지 않는다.
- `message`, `help`, `scope`, `explain`을 파싱하지 않는다. `explain`은 실행하지 않는다.
- JSON 파싱에 실패하면 실패로 취급한다. 파싱 실패를 건너뛰는 fail-open을 하지 않는다(앱 패키저의 현재 동작은 반대다. 7절의 범위 밖 발견 참고).
- 키 순서와 공백에 의존하지 않는다.

우리 쪽 시험(제안): 필수 키와 타입은 부분집합으로 검사하고 정확 일치 비교를 없앤다. 선택 필드를 더해도 계약 시험이 깨지지 않아야 한다. 값이 바뀌어도 되는 필드(`message` 등)의 골든은 별도 갱신 가능한 스냅샷으로 둔다(G3).

## 6. 기각한 대안과 이유

| 대안 | 기각 이유 |
| --- | --- |
| 진단 JSON 전체 구조를 호환 보장 대상으로 고정 | 소유자 결정과 충돌한다. 또 `message`와 `help`는 개선 대상이라 고정 비용이 큰 데 비해 도구가 의존할 이유가 없다(3.3절) |
| 현재의 키 집합 정확 비교 시험을 그대로 호환 약속으로 승격 | 선택 필드를 더할 때마다 시험 수정과 호환 논의가 따른다. `severity` 추가가 이미 한 번 그 비용을 치렀다. 제거와 타입 변경 금지만 약속하면 같은 안전을 더 싸게 얻는다 |
| `message` 문구 고정 또는 문구 매칭 허용 | 개선과 번역을 막는다. 저장소 안 소비자가 이미 문구를 매칭하지만(3.1절) 이는 코드 입도가 거친 탓이므로 코드를 나누는 쪽이 낫다(5.1절 8번) |
| 열을 바이트 오프셋으로 | 한글 3자만 있어도 6칸이 어긋난다(2.4절). 사람 표시와 편집기에 불리하고, 파이썬 등 코드 포인트로 색인하는 언어에서 변환이 필요하다. 바이트 오프셋은 필요하면 선택 필드로 더하는 편이 낫다 |
| 열을 UTF-16 단위로(LSP 기본) | 비 BMP 문자에서 현재 출력(코드 포인트)과 값이 달라져 이미 나간 출력의 의미를 바꾼다. 클라이언트 쪽 변환은 소스 텍스트가 있어야 하는 편집기에서 쉽다 |
| 0 기준 | 현재 출력과 `file:line:col` 렌더가 모두 1 기준이다. 컴파일러 관례(1 기준)와 같고 LSP로의 변환은 클라이언트에서 1을 빼면 된다 |
| 위치 없음을 `null`이나 필드 생략으로 | 시험과 소비자가 `line`, `column`을 항상 정수로 읽는다(`service.rs:58-59`). 정수를 가정한 소비자가 0 아래 값이나 없음을 만나면 편집기에서 -1 줄 같은 값이 나온다. 1:1은 "파일 시작"으로 표시해도 무해하다. 단 실제 1:1 진단과 구분되지 않으며 구분이 필요하면 `code`로 한다 |
| 위치 없음을 현재처럼 0:0으로 유지하고 문서로만 약속 | 5.2절의 "1 이상" 규칙을 가장 값싸게 깬다. 편집기 변환에서 -1이 된다. 다만 값을 바꾸지 않는 이점이 있어, 소유자가 `0 = 위치 없음` 약속을 원하면 이 안으로 갈 수 있다(8절) |
| `PROTOCOL_VERSION`을 진단 스키마 버전으로 재사용 | protocol은 HTTP 호출자 계약의 major이고 CLI 진단과 수명이 다르다. 한쪽의 major가 오르면 다른 쪽을 끌고 간다 |
| `aip --version`만으로 버전 판단 | 저장된 JSON(CI 로그, 아티팩트)만 보고는 어느 스키마인지 알 수 없다. 프로세스를 다시 띄워 버전을 물어야 한다 |
| 진단마다 스키마 버전 | 문서 하나에 한 번이면 충분하다. 항목마다 두면 출력만 늘어난다 |
| 버전 필드 없이 알 수 없는 필드 허용만으로 진화 | 제거나 의미 변경이 일어났을 때 소비자가 알 방법이 없다. 단 필드가 없는 출력은 v1로 읽게 해서 이 안의 장점(기존 출력 유지)은 가져왔다 |
| `severity`를 닫힌 enum으로 | 나중에 `info`, `hint`를 더하려면 major가 된다. 열린 값 집합과 컨테이너별 기본 처리로 충분하다 |
| 숫자 코드(PoC의 `E104` 방식) | 이미 `영역.이름` 하나로 통합했다(D-A24, 2026-10-13, `plan-docs/reviews/2026-10-13-ai-friendliness-runtime-reliability-audit.md`). 되돌리지 않는다 |
| 파일 경로 필드를 모든 진단에 추가 | 대상 명령이 파일 하나를 받으므로 최소 계약에는 불필요하다. 여러 파일을 받게 되면 선택 필드로 더할 수 있다 |
| 끝 위치(범위)를 계약에 포함 | 현재 구현에 끝 위치가 없고 모든 진단에 정확한 범위를 주려면 파서 변경이 필요하다. 최소 계약에는 과하다 |
| 연쇄 관계를 구조화 필드(`causedBy`)로 계약에 포함 | 지금은 `help` 문장의 `cascade:` 안내뿐이다. 구조화는 유용하지만 최소 계약 밖이고 선택 필드로 나중에 더할 수 있다 |

## 7. 현재 구현이 최소 계약을 만족하지 못하는 지점 (이번에는 구현하지 않음)

규모는 변경 줄 수가 아니라 영향 범위의 어림이다. "가산"은 소비자가 깨지지 않는 변경, "동작 변화"는 기존 출력 값이 달라지는 변경이다.

| ID | 지점 | 근거 | 필요한 변경 | 성격 |
| --- | --- | --- | --- | --- |
| G1 | 위치 없는 진단이 `line: 0, column: 0`을 낸다. 5.2절의 "1 이상"을 깬다 | 관측: `empty.aip`, `noblock.e.ts`. 발생지: `Span::default()`를 쓰는 `sema.rs:283`(`DEF.MISSING_ITEM`), `limits.rs:12`, `hlit.rs:217-218`, `extract.rs:69,120-121`, `sema.rs:182`, `sema.rs:3003`의 `span_of` 대체값 | `Diag::json`에서 두 값을 1 이상으로 맞추거나 `Span` 기본값을 1:1로 바꾼다. `render`의 `0:0`도 같이 본다 | 동작 변화(값 0이 1로) |
| G2 | 경고는 `line`·`column` 없이 `anchor` 문자열뿐이다. 컴파일러가 낸 경고는 위치를 알면서 `scope`, `help`, `explain`을 버린다. `--dev-checks` 경고의 anchor는 위치가 없다 | `spikes/spike-v11-dev-checks/src/lib.rs:171`, 부록 A-2 | line 앵커 경고에 정수 `line`, `column`을 더한다. `anchor`는 남긴다. 위치를 모르는 경고는 생략을 허용 | 가산 |
| G3 | 정확한 키 집합 비교가 가산 변경을 막는다. 선택 필드 `scope`·`help`가 없는 실제 출력(H 형식, `DEF.WRONG_LANGUAGE`)은 시험의 단일 fixture에 걸리지 않는다 | `service.rs:28,33,49,55`, 관측 `g3.h.ts`, `wrong_lang` | 필수 키와 타입은 부분집합 검사로, 선택 필드는 "있으면 string"으로 바꾼다. 내부 정보 골든은 갱신 가능한 스냅샷으로 분리한다. 모르는 필드가 있어도 통과하는 시험을 둔다 | 시험 변경 |
| G4 | 스키마 버전 표지가 없다 | `COMPATIBILITY.md` 진단 JSON 항("별도 스키마 버전 필드는 없다") | `schema: "aip-diagnostics/1"` 추가, 없으면 1로 해석 | 가산 |
| G5 | 실패 봉투가 두 모양이다. 정의 오류가 아닌 실패의 코드(`check` 외 명령 포함 62종)는 영역 접두가 없고 `explain-code`가 모른다. `spec/definition-diagnostics.md`는 "모든 AIP 오류 코드는 `영역.이름` 형식"이라고 쓴다 | 관측 `SOURCE_IO`, `INPUT_TOO_LARGE`, `explain-code SOURCE_IO`(종료 1), `lib.rs:483` | 선택지 둘. (가) 이 코드들을 `CLI` 같은 영역으로 등록하고 `explain-code`에 싣는다. (나) 최소 계약은 이들을 "잠정, 보장 없음"으로 두고 `diagnostics` 유무로 두 모양을 구분하게 한다. 이 문서는 (나)를 전제로 쓰고 (가)를 후속으로 권한다 | (가)는 코드 이름 변경이라 동작 변화, (나)는 문서뿐 |
| G6 | 레지스트리에 `severity`, `status`, `since`가 없고 "발생지 없는 코드가 있으면 시험 실패"라서 폐기 코드를 레지스트리에 남길 수 없다. 경고 코드인지는 이름 접두로만 안다 | `codes.rs`의 `Info`, `aip diagnostics --json` 키(`area, code, example, explain, fix, kind, title`), `diagnostics.rs`의 `every_code_is_registered_and_none_is_dead` | `Info`에 `severity`, `status`(active/retired), `since`, `replacedBy`를 더하고 시험을 retired 허용으로 바꾼다. 코드 삭제를 막는 목록 비교 시험을 둔다 | 가산(레지스트리 출력), 시험 변경 |
| G7 | DEF 코드 안정성 규칙이 문서 한 문장뿐이고 `COMPATIBILITY.md`는 `wire-errors.md`만 지목한다. 또 `COMPATIBILITY.md`의 진단 JSON 표는 항목 키에 `scope`, `help`, `explain`이 늘 있는 것처럼 읽히는데 실제로는 없는 경우가 있다 | `COMPATIBILITY.md` 8행(읽는 법의 protocol 항)과 진단 JSON 형식 절, 관측 `g3.h.ts`, `wrong_lang` | 소유자 승인 후 5절의 규칙을 `COMPATIBILITY.md`에 옮기고 표를 "선택"으로 고친다. 이번 작업에서는 고치지 않았다 | 문서 |
| G8 | `DEF.SOURCE_TOO_LARGE`는 레지스트리에 있지만 `service check`가 파일을 읽을 때 같은 1 MiB 상한이 먼저 걸려 `INPUT_TOO_LARGE`(접두 없음)가 나온다. 파일 경로로는 도달하지 않는 것으로 보인다 | 관측: 1048579B 파일이 `INPUT_TOO_LARGE`. 상한 상수가 같다(`lib.rs:21`, `limits.rs:3`). 도달 불가는 추론 | 읽기 단계에서 `DEF.SOURCE_TOO_LARGE`를 진단 봉투로 내거나 레지스트리 항목의 상태를 정한다 | 동작 변화 |
| G9 | 저장소 안 소비자가 `message` 부분 문자열로 구분한다 | `.github/workflows/flaskbb-post-order.yml:213`. `DEF.UNKNOWN_SYMBOL`은 최소 두 곳에서 다른 뜻으로 난다(`sema.rs:281`의 actor 대상, `sema.rs:520`의 enum 값). 이 CI가 보려는 `expose apply` 경우의 정확한 발생지는 확인하지 않았다(확인 필요) | `expose apply` 경우에 전용 코드를 추가하고 CI를 코드 비교로 옮긴다(5.1절 8번) | 소비자 변경 + 코드 추가 |
| G10 | `BrokenPipe`를 성공(종료 0)으로 처리하므로 "종료 0이면 JSON이 전달됨"이 깨질 수 있다 | `lib.rs:1787` | 계약에서 "종료 0은 정의 수용"만 약속하고 전달 보장은 하지 않는다고 적거나 EPIPE에서 비영 종료로 바꾼다 | 문서 또는 동작 변화(낮음) |

범위 밖에서 발견해 고치지 않은 것(한 줄씩):

- 앱 패키저는 `workers`를 읽을 때 JSON 파싱에 실패하면 조용히 `return`한다(`product/tools/app-package.mjs:35`). 5.4절의 소비자 규칙과 반대다.
- 파일 앞에 BOM이 있으면 `DEF.LEX_UNEXPECTED_CHAR`가 1열에서 난다(2.4절). 위치 계약과 무관하지만 Windows 편집기 사용자에게 혼란을 줄 수 있다.
- `help`는 영어이고 `message`는 한국어로 섞여 있다. 언어 정책은 이 계약이 정하지 않는다.
- `aip-cli`의 시험 바이너리는 `AIP_TEST_DATABASE_URL`이 없으면 빌드 단계(`crates/aip-test-support/build.rs:13`)에서 중단된다. 진단 계약 시험은 DB를 쓰지 않는데도 PG 설정 없이는 실행할 수 없다(8.1절).

## 8. 소유자 결정이 필요한 항목

1. 위치 없음의 표현: 1:1로 바꿀지(권장), 현재 0:0을 "위치 없음" 약속으로 문서화할지(5.2절, 6절).
2. `workers` 필드를 이 계약에 넣을지. 앱 패키저가 읽지만 진단은 아니다.
3. `diagnostics[0]`을 "먼저 고칠 항목"으로 보장할지, 관례로만 둘지.
4. `schema` 필드를 1.0 전에 넣을지, 1.x에서 가산으로 넣을지(5.3절 3번에 따라 둘 다 호환된다).
5. 정의 오류가 아닌 실패 코드 62종을 등록할지(G5의 가)와 시점.
6. 새 거부 규칙을 patch 금지로 못 박을지(5.1절 3번).

### 8.1 검증 기록

| 항목 | 명령 | 결과 |
| --- | --- | --- |
| 빌드 | `CARGO_TARGET_DIR=/tmp/aip-target-diag cargo build -p aip-cli --bin aip` | `Finished dev profile ... in 24.78s` |
| 정상 | `aip service check ok_min.aip` | 종료 0, stdout 158B, stderr 0B, `"ok":true,"advisories":[]` |
| 경고 | `aip service check recruitment.aip` | 종료 0, `advisories[0].anchor`는 `line:60:col:3`, `severity`는 `warning` |
| 실패 | `aip service check f1_unknown_key.aip` | 종료 1, stdout 0B, `DEF.PARSE_UNKNOWN_KEY`, 56줄 5열 |
| 위치 단위 | 한글, 이모지, 탭, CRLF 입력 | 열 61, 59, 30, 47(2.4절 표) |
| 결정성 | `f2_cascade.aip`를 3회 | 출력 SHA-256이 세 번 모두 `f9ccdd65...ffad` |
| 기존 시험 | `cargo test -p aip-cli --test service service_check_json_keeps_success_and_failure_diagnostic_contracts -- --exact` | 실행하지 못함. `crates/aip-test-support/build.rs:13`이 `AIP_TEST_DATABASE_URL is required; refusing PostgreSQL defaults`로 빌드를 중단했다. 이 시험이 고정하는 키와 값은 대신 위 fixture 출력으로 직접 대조했다 |

미실행: `service gen`, `serve` 등 다른 명령의 실패 봉투. 코드상 `load()`를 공유해 같은 봉투를 내지만 실행해 보지 않았다(추론).

## 부록 A. fixture 출력 원문

모든 JSON은 한 줄이고 가독을 위해 여기서만 줄을 나눈다. 키 순서는 실제 출력 그대로(알파벳 순)다.

### A-1 정상, 경고 없음 (`ok_min.aip`, 종료 0, stdout)

```json
{"advisories":[],"executionDigest":"7ad0e907efc052925490aeaa9c4cdf9f0b386750e3466b4627e434bbcbdb8315","ok":true,"workers":{"extensions":[],"required":false}}
```

### A-2 정상 + 경고 (`recruitment.aip`, 종료 0, stdout)

```json
{"advisories":[{"anchor":"line:60:col:3","code":"DEF.ADVISORY_PUBLIC_COUNT","message":"aggregate `bookmarkCount`는 where 없는 공개 count: ...(문장 일부 생략)","severity":"warning"}],"executionDigest":"3801434d2b06755465a046d47709ff620f51086b0acb429103cf861596a42d82","ok":true,"workers":{"extensions":[{"implementation":"recruitment.stats","kind":"read","name":"Recruitment.stats"}],"required":true}}
```

`--dev-checks` 경고의 anchor 형식(종료 0, stdout):

```json
{"advisories":[{"anchor":"predicate:orphan","code":"DEF.ADVISORY_UNUSED_PREDICATE","message":"실행 facts에 이 predicate의 직접 호출 참조가 없음","severity":"warning"}], ...}
{"advisories":[{"anchor":"resource:Tag","code":"DEF.ADVISORY_UNREACHABLE_READ","message":"`Tag`의 expose read에 budget이 없어 ...","severity":"warning"}], ...}
```

### A-3 실패, 진단 1건 (`f1_unknown_key.aip`, 종료 1, stderr)

```json
{"code":"DEF.INVALID_DEFINITION","diagnostics":[{"code":"DEF.PARSE_UNKNOWN_KEY","column":5,"explain":"aip explain-code DEF.PARSE_UNKNOWN_KEY","help":"did you mean `sort`? `expose read` accepts: select, refIds, filter, sort, traverse, budget. fix: Use one of the keys listed in the help. Every block and key is described in spec/definition-syntax.md.","line":56,"message":"expose read 안 알 수 없는 항목 `sorts`","scope":"resource Recruitment > expose read","severity":"error"}],"ok":false}
```

### A-4 실패, 진단 2건 (`f2_cascade.aip`, 종료 1)

`DEF.UNKNOWN_TYPE` 3줄 42열 `resource Post > fields`, `DEF.UNRESOLVED_NAME` 3줄 78열 `resource Post`. 둘 다 `help` 있음.

### A-5 실패, `scope` 없음 (`wrong_lang`, 종료 1)

```json
{"code":"DEF.INVALID_DEFINITION","diagnostics":[{"code":"DEF.WRONG_LANGUAGE","column":1,"explain":"aip explain-code DEF.WRONG_LANGUAGE","help":"`aip service` reads product definitions ... rename it to `.aip`.","line":1,"message":"`/tmp/diag-fx/w.aipoc` has the extension `.aipoc` of PoC programs, not of product definitions","severity":"error"}],"ok":false}
```

호스트 리터럴 형식(`g3.h.ts`)도 `DEF.UNKNOWN_KEY` 55줄 16열이고 `scope`가 없다.

### A-6 실패, 위치 0:0 (`empty.aip`, 종료 1)

```json
{"code":"DEF.INVALID_DEFINITION","diagnostics":[{"code":"DEF.MISSING_ITEM","column":0,"explain":"aip explain-code DEF.MISSING_ITEM","help":"fix: Add the part the message names. For a transition that should be allowed on every row write `from true`.","line":0,"message":"actor 선언 필요","severity":"error"}],"ok":false}
```

### A-7 실패, 정의 오류가 아닌 봉투 (종료 1, stderr)

```json
{"code":"SOURCE_IO","message":"파일을 열 수 없음","ok":false}
{"code":"UNSUPPORTED_FORM","message":"지원하는 작성 형식이 아님","ok":false}
{"code":"INPUT_TOO_LARGE","message":"파일의 제품 입력 상한 초과","ok":false}
```
