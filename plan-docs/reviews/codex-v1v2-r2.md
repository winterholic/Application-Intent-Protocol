# V1 반영 확인 + V2 호출자 읽기 slice 비판 검토 (r2)

기준: 창시자 통합 지침 → E(RK-01~03·§4) → C(EQ-01~11·SY-2·SY-3). 검토 대상은 두 spike뿐이다. 원본 코드·다른 문서·git·.env·비밀 저장소는 수정하거나 접근하지 않는다. 변형은 임시 복사본에서만 수행한다. 발견은 확인 즉시 아래에 append한다.

## 직접 실행 기록

명령: `cd spikes/spike-v1-fixture && PATH="$HOME/.cargo/bin:$PATH" cargo test --offline -q`
```text
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.17s
```

명령: `cd spikes/spike-v2-read && PATH="$HOME/.cargo/bin:$PATH" cargo test --offline -q -- --nocapture`
```text
로컬 PostgreSQL 연결: Error { kind: Connect, cause: Some(Os { code: 1, kind: PermissionDenied, message: "Operation not permitted" }) }
planner rejection cases: 19
test result: FAILED. 3 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

확인 못함: 이 환경의 샌드박스가 localhost PostgreSQL 연결을 차단한다. DB 실행에 의존하는 원 작성자의 성공 주장은 이 검토에서 재확인되지 않았다. 연결 전에 실패했으므로 이 실행에서 schema 생성·삭제는 발생하지 않았다. 권한 우회나 다른 DB 접속은 시도하지 않는다.

## 발견 및 확인 기록

### R2-01 · P1 · F01 재발 — Python raw 삼중 문자열의 escaped quote를 닫는 따옴표로 오인한다

영향: 실행 가능한 AIP 선언이 없는 E-Py/H-Py 파일이 기준 facts로 승인된다. F01의 원래 주석/일반 문자열 반례는 거부되지만, 호스트 문자열 경계 전체가 닫혔다는 주장은 성립하지 않는다.

근거: `spikes/spike-v1-fixture/src/host.rs:172-197`, 특히 raw 문자열에서 backslash 뒤 한 byte만 건너뛰는 `:190`. Python raw 문자열도 quote를 escape할 수 있다. Python 표준 `ast.parse`로 아래 소스에 호출 AST가 없음을 직접 확인했다. 모듈 실행은 하지 않았다.

반례 구조(E-Py): 공식 import 뒤 `TEXT = r'''\'''`를 쓰고 다음 줄에 `FAKE = aip("""<기준 A>""")`, 마지막 줄에 `# '''`를 둔다. 실제 Python에서 FAKE 줄은 TEXT 문자열의 내용이다. H-Py는 같은 raw 문자열 안에 기준 `define({...})` 선언을 넣는다.

명령: Python에서 `ast.parse(src)` 후 `subprocess.run([<원본 V1 binary>, 'facts', <임시 fixture>])`.
```text
f01-raw.e.py python AST: top-level= ['ImportFrom', 'Assign'] calls= 0; CLI exit=0
f01-raw.h.py python AST: top-level= ['ImportFrom', 'Assign'] calls= 0; CLI exit=0
두 executionDigest=17612191e77a3a1c… (기준과 동일)
```

조치: raw 문자열의 quote escape 규칙을 정확히 처리하거나 그 호스트 구문을 명시 거부하고 E/H 양쪽에 음성 대조를 고정한다. V3 전에 필요한 것은 호스트 전체 지원 확대가 아니라 가짜 선언 승인 차단이다.

### R2-02 · P1 · F09 부분 미종결 — A/E 입출력·predicate 매개변수 이름 중복은 남는다

영향: 같은 이름의 계약이 여전히 승인된다. H 객체의 중복 키는 거부하지만 A/E params 배열은 이름 중복을 유지하므로 작성 형식의 수용 범위가 갈린다. V3와 이후 확장 계약에서 위치 인자와 이름 바인딩이 서로 다른 계약을 보게 된다.

근거: `spikes/spike-v1-fixture/src/parser.rs:225-250`은 매개변수를 중복 검사 없이 push한다. `src/sema.rs:236-246`도 그대로 facts에 남긴다. V2의 이름 환경은 `spikes/spike-v2-read/src/sqlgen.rs:246-255`의 HashMap insert로 동일 이름의 앞 인자를 덮어쓴다. 원래 필드 정책/from/budget 반례는 거부되었다.

실험: 기준 A의 `input { clubId: Club.Id }`를 `input { clubId: Club.Id; clubId: Club.Id }`로, 출력의 `approvedApplicants: Int`를 두 번으로 각각 치환했다. 별도 변형은 `predicate dup(m: Member, m: Member) = m.id = m.id`를 추가했다.
```text
f09-input.aip exit=0 executionDigest=25ddafed7bd28c73…
f09-output.aip exit=0 executionDigest=2a3d232ffe52148f…
f09-predicate.aip exit=0 executionDigest=0ed7d8a6c199bbc6…
```

조치: 각 params/input/output의 이름 유일성을 필수 의미 검사로 강제한다. 같은 transition의 `to status = DRAFT, status = CLOSED` 변형은 거부되었으므로 그 경로의 재발로 넓혀 쓰지 않는다.

### R2-03 · P2 · F11 부분 미종결 — 65개 predicate 순환은 V1의 탐색 한도를 벗어난다

영향: 자기 재귀·2개 상호 재귀는 거부되지만 긴 cycle은 잘못된 facts로 승인된다. V2는 별도의 식 깊이 32 한도로 실패하게 되어 무한 SQL 생성이나 데이터 노출을 관찰한 발견은 아니다.

근거: `spikes/spike-v1-fixture/src/sema.rs:145-162`의 `trail.len() < 64`는 한도를 넘으면 진단 없이 탐색을 멈춘다. V2의 방어는 `spikes/spike-v2-read/src/sqlgen.rs:191-199`이다.

실험: active(r)를 p0(r)로 치환하고 `p0 → p1 → … → p64 → p0` 선언을 추가했다. 원본 CLI facts 실행 결과:
```text
f11-self.aip exit=1; f11-mutual.aip exit=1
f11-cycle65.aip exit=0 executionDigest=6a012571b54af176…
```

조치: cycle 검사를 완전한 그래프 검사로 바꾸거나 분석 한도 초과를 명시 오류로 거부한다. V3에서 이 facts를 재사용할 때 순환 허용으로 오해하지 않는다.
