# 문자열 리터럴 표현력 실험

> 상태: 제품 문법의 최종 결정이 아닌 PoC 보완. 독립 AIP 정의(A), TS/Python의 E 블록, TS/Python의 H 객체 정의가 같은 문자열 값과 facts를 만들 수 있는지 검증한다. `.aip` 파일 필요성과 최종 작성 형식은 OPEN이다.

## 현재 경계와 후보

V1 A lexer는 문자열 안의 모든 역슬래시와 실제 LF를 거부한다. H 리터럴 파서도 동일하다. E 추출기는 TS tagged template와 Python 삼중 따옴표의 본문을 호스트 언어로 해석하지 않고 원문 슬라이스로 돌려주며, 역슬래시를 통째로 거부한다. 따라서 따옴표·역슬래시·줄바꿈이 필요한 서버 선언 상수는 현재 다섯 형식에서 자연스럽게 옮길 수 없다.

후보는 두 단계가 필요한 곳만 두 번 해석한다. A는 AIP 문자열 escape를 한 번 해석한다. E는 호스트 문자열의 cooked 의미를 제한된 정적 부분집합으로 한 번 해석한 뒤 AIP lexer에 넘긴다. H는 객체의 호스트 문자열을 한 번 해석하고, 정책·효과의 식 문자열은 기존처럼 AIP 식 파서에서 다시 해석한다. 정적 호스트 리터럴의 실제 런타임 값과 정적 추출 결과가 다르면 거부한다. TS의 raw tagged template 계약이나 Python raw-prefix를 이번 후보의 전제로 삼지 않는다.

AIP 문자열 단계는 `\"`, `\\`, `\/`, `\b`, `\f`, `\n`, `\r`, `\t`, `\uXXXX`와 유효한 UTF-16 surrogate pair를 받는다. 알 수 없는 escape, 불완전한 16진수, 단독 surrogate와 NUL은 거부한다. 기존에 escape 없이 들어오던 문자(한글과 raw tab 포함)는 유지하고, 문자열 안의 실제 LF는 계속 거부한다. 호스트 단계는 TS template와 Python 일반 삼중 따옴표 양쪽에서 같은 cooked 값을 만드는 escape만 받는다. 호스트 `\/`처럼 TS/Python 결과가 다른 표기는 거부한다. 호스트 `\u`의 단독 surrogate는 Rust 문자열과 Python/JS 문자열의 모델이 달라 거부하며, 원하는 보충 평면 문자는 AIP 문자열 단계의 surrogate pair나 원문 Unicode로 표현한다.

호스트의 보간, 함수·spread, 결합, 동적 평가 금지는 유지한다. TS E에서 `${...}`는 계속 거부한다. Python E에서 f-string과 접두사는 기존처럼 거부한다. Python 일반 삼중 따옴표의 cooked 값은 정적 공통 decoder로 계산한다. 소스와 디코딩된 문자열 모두 기존 길이·토큰·AST 상한을 적용한다.

E와 H의 식 문자열 진단 위치는 아직 정확한 source map이 아니다. E의 기존 `Block(line_base,col_base)`와 H의 식 시작 `Span`은 시작점만 기록하므로, 호스트 escape가 실제 줄바꿈이나 더 짧은 Unicode 문자로 cooked 된 뒤의 후속 파서 진단은 원본 호스트 열·줄과 달라질 수 있다. 이 실험은 의미와 값의 일치를 검증하고, 원문 위치의 완전한 대응은 후속 source-map 과제로 남긴다.

## 설계 관문

1. **원칙:** 반복되는 업무용 문자열을 백엔드 별도 코드 없이 선언할 수 있어 §2의 1·2·3·4에 기여한다. 호스트 문자열 규칙을 흉내 내는 구현 복잡도는 검증 가능성과 긴장한다. 정적 공통 부분집합과 교차 형식 테스트로 범위를 제한한다.
2. **표현 범위:** 따옴표, 역슬래시, 개행·제어문자, Unicode를 가진 문자열 상수를 표현할 수 있다. 기존 무escape 문자열과 raw tab은 그대로 유효하다. 호스트 언어별 서로 다른 escape는 거부한다.
3. **서버 작성량:** 새 전이·정책 API를 만들지 않고 기존 선언의 상수만 정확하게 쓸 수 있어 우회 코드가 줄어든다. 새 서버 설정은 없다.
4. **최종 권한과 신뢰:** 문자열은 여전히 서버 정의에서만 오며 호출자 SQL이 아니다. 컴파일러가 escape의 유효성·길이·NUL을 검사하고, 실행기는 기존 bind·타입·권한·불변조건 검사를 유지한다. 호스트 소스를 실행하지 않는다.
5. **생태계:** TS tagged template와 Python 일반 삼중 따옴표의 cooked 의미를 따라 작성자가 익숙한 표기를 쓴다. H 객체도 일반 호스트 문자열 리터럴을 해석한다. 공통 밖 문법은 명시 오류를 내며 Python raw-prefix/TS raw 해석을 새 표준으로 확정하지 않는다.
6. **중복 표현:** 원문 문자와 escape 표기가 같은 값으로 정규화되는 일반 문자열 성질은 허용한다. 다섯 작성 형식의 결과 facts는 같은 값 하나로 정규화한다. 호스트 디코딩과 DSL 디코딩을 묵시적으로 섞지 않는다.
7. **수단과 목적:** decoder는 기존 AST·facts로 이동하기 위한 경계 도구다. 새 IR이나 공개 실행 표면을 만들지 않는다.
8. **OPEN:** `.aip`·E·H 중 최종 작성 형식, JS/Python 패키지 구현·배포, escape 전체 언어 사양은 확정하지 않는다. 여기서는 기존 다섯 PoC 형식이 실제 호스트 값과 모순되지 않는 공통 리터럴만 검증한다.

## 검증 경계

- A/E/H 다섯 형식에서 서로 다르게 이중 인코딩된 전이 상수와 정책 식이 동일 facts를 내야 한다. 호스트 단계와 AIP 단계가 각각 정확히 한 번만 해석되는 경우를 포함한다.
- 실제 Node/Python 리터럴 값과 추출기의 cooked 값이 같은지 비교한다. 실행기가 없으면 해당 비교는 미검증으로 명시한다.
- 알 수 없는 escape, NUL, 단독 surrogate, 불완전한 Unicode, TS 보간, Python f-string을 거부한다. 기존 raw tab과 무escape 문자열은 회귀한다.
- PostgreSQL에서 escape로 만든 Text 상수를 전이·효과에 써도 bind 값이 정확하고 기존 권한·제약·롤백 경계를 유지하는지 확인한다.

## PoC 확인 결과

`spike-v1-fixture`의 `cargo test --quiet` 전체와 `spike-v3-write`의 `cargo test --quiet --test string_literals`가 통과했다. V1의 새 테스트는 다섯 형식의 facts 동등성, 실제 Node/Python cooked 문자열과 정적 추출의 일치, 기존 raw tab·한글, CRLF, 호스트 문자열 안 raw CR, Unicode escape로 만든 따옴표, 잘못된 escape·NUL·단독 surrogate를 검사한다. 호스트 단계의 *유효한* surrogate pair도 Python과 JS의 cooked 문자열 모델이 달라 거부하며, AIP 문자열 단계에서는 이를 보충 평면 문자 하나로 결합한다. V3 PostgreSQL 테스트는 탈출된 Text 상수를 읽기 정책·전이 대입·create 효과·notify topic에 통과시키고 비인가 요청을 거부한 뒤, 실제 DB 값을 비교한다.
