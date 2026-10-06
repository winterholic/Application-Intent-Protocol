너의 작업 결과물은 파일 하나를 쓰는 것이다: `plan-docs/reviews/codex-v1-r1.md`.

# V1 의미 fixture 비판 검토 (r1)

## 배경
- 기준 문서(우선순위 순): `plan-docs/sources/founder-integrated-directive-2026-10-03.md`, `plan-docs/alignment/E-technical-risks.md` §4 V1, `plan-docs/alignment/C-syntax-proposal.md` B.1~B.3·B.8·E(SY-1, SY-5, SY-6).
- 검토 대상: `spikes/spike-v1-fixture/` 전체(src/, fixture/, tests/v1.rs). 본 crates/는 대상 아님.
- 실행 방법: `cd spikes/spike-v1-fixture && PATH="$HOME/.cargo/bin:$PATH" cargo test --offline -q` 와 `./target/debug/spike-v1-fixture facts fixture/recruitment.aip`.
- 작성자 주장(미검증으로 취급하라): A·E-TS·E-Py·H-TS·H-Py 5형식이 같은 execution/metadata digest를 낸다. 음성 대조 다수가 기대 코드로 거부된다. docs 변경/삭제는 execution digest를 바꾸지 않는다.

## 할 일
1. 테스트와 CLI를 직접 실행하고, 명령과 출력 1~3줄을 파일에 적는다.
2. 아래 질문마다 근거(파일:줄)와 함께 비판한다. 동의하면 동의 근거를, 반대하면 반례(가능하면 실제로 실행한 변형 입력과 결과)를 적는다.
   - Q1. "같은 typed facts"가 EQ-01~EQ-11의 의미를 충분히 담는가? facts에 빠졌거나 잘못 정규화된 의미(예: select/sort를 집합으로 정렬한 것, 식 인자 순서, enum 값 순서, Ref/Id 호환 규칙)가 있는가?
   - Q2. 동등성 판정이 공허하지 않은가? digest가 같아도 의미가 다를 수 있는 경우, 또는 의미가 같은데 digest가 다른 경우를 찾아라.
   - Q3. E 추출기가 모듈 실행 없이 정적 리터럴만 읽는다는 주장에 구멍이 있는가? (주석/문자열 안의 `aip`, 별칭, re-export, 다른 이름 import, 두 번째 블록, 문자열 안 백틱 등)
   - Q4. H 리터럴 파서/매핑의 구멍. 모르는 키 무시, 실행 가능한 값 통과, 키 순서 의존(predicate 매개변수) 위험.
   - Q5. spike가 임의로 정한 의미 규칙(행 정책 없으면 denyAll, budget 없는 expose는 traverse 전용, 정책 안 exists는 대상 행 정책 없이 평가, atMost 1 → partial unique index, Ref/Id 호환)이 창시자 지침이나 C/E 문서와 충돌하는가? 창시자 결정이 필요한 것이 섞여 있는가?
   - Q6. E §4의 V1 완료 기준 대비 빠진 것. V2(호출자 읽기 수직 slice)로 넘어가기 전 막아야 할 것.
3. 발견마다 즉시 파일에 append한다. 끝에 몰아 쓰지 않는다.
4. 확인하지 못한 것은 "확인 못함: <이유>"로 적는다. 발견이 없으면 "없음"과 확인한 범위를 적는다. 빈칸을 채우려고 발견을 만들지 않는다.
5. 각 발견에 심각도 P1(V2 전에 반드시 수정)/P2(수정 권장)/P3(기록)를 붙인다.

## 금지
- `codex-v1-r1.md` 외 어떤 파일도 수정·생성하지 않는다(`target/` 빌드 산출물 제외). 변형 실험은 임시 디렉터리 복사본에서 한다.
- `.env`·비밀 저장소 읽기 금지. git 명령 금지.

## 최종 응답
10줄 이하 요약 + 파일 경로.
