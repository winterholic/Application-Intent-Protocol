너의 작업 결과물은 파일 하나를 쓰는 것이다: `plan-docs/reviews/codex-v4-r4.md`.

# 확장 실행 구조 정확성 검토 (r4)

이 저장소는 사용자 본인이 설계 중인 오픈소스 애플리케이션 프레임워크(AIP)다. 서버가 JS/TS·Python 확장 코드를 별도 worker로 실행하면서 선언된 계약(입력·출력·허용 데이터 접근·기한)을 정확히 집행하는지, 외부 효과 전달 구조가 주장한 보장 범위를 갖는지 검토한다.

## 배경
- 기준: `plan-docs/sources/founder-integrated-directive-2026-10-03.md`(원칙 7, Q7 확장 격리), `plan-docs/alignment/E-technical-risks.md`(RK-05, RK-08), `plan-docs/alignment/C-syntax-proposal.md`(B.7).
- 결과 문서: `plan-docs/alignment/V4-official-extension-results.md`(V4-1 읽기 확장, 격리 대안, V4-2 outbox 전달).
- 코드: `spikes/spike-v4-worker/`(src/lib.rs·outbox.rs, workers/worker.mjs·worker.py, extensions/recruitment.mjs·.py, tests/). V1 facts·V2 집계 실행기를 재사용한다.
- 직전 검토 `plan-docs/reviews/codex-v3-r3.md`(네가 쓴 것)의 반영: `plan-docs/alignment/V3-standard-write-results.md` §6.6, 코드 `spikes/spike-v3-write/src/lib.rs`, `tests/v3_r3.rs`.
- 실행: 각 spike에서 `PATH="$HOME/.cargo/bin:$PATH" cargo test --offline -q -- --nocapture`. 로컬 PostgreSQL, node, python3, macOS `sandbox-exec` 사용.
- 작성자 주장(미검증으로 취급): worker는 서버가 발급한 ctx 토큰으로만 승인된 집계를 읽고, actor는 토큰에서만 온다. 출력은 선언과 정확히 일치해야 한다. 기한·토큰 수명·worker 장애가 처리된다. 기본 worker는 네트워크 격리가 없고 sandbox-exec 대안으로 DB 직접 연결이 막힌다. outbox는 커밋된 효과만, 최소 한 번 전달, 멱등 키로 한 번 효과를 낸다.

## 할 일
1. 테스트를 직접 실행하고 명령과 출력 1~3줄을 적는다.
2. r3 발견(R3-01~07) 반영을 원래 재현 입력으로 확인한다. 특히 R3-03, R3-05.
3. V4 질문. 근거(파일:줄)와 가능하면 실제 실행 입력·결과:
   - Q1. 확장 계약 집행: 확장 코드가 선언되지 않은 데이터·다른 actor 범위·다른 입력으로 결과를 얻거나, 선언되지 않은 출력을 호출자에게 전달하는 정상 형식의 코드가 있는가?
   - Q2. 토큰과 호출 경계: 같은 worker 안 동시 호출, 늦은 응답, 토큰 추측·재사용, 호출 id 혼동.
   - Q3. 기한·장애: 기한 뒤에도 계속 도는 확장 코드의 영향, worker 교착·대량 출력·잘못된 메시지에서 서버 동작.
   - Q4. 격리: 환경 변수 제거와 sandbox-exec 대안의 실제 범위(파일 접근, 자식 프로세스, 외부 API 호출 필요성과의 충돌). 문서 서술이 실행 근거와 맞는지.
   - Q5. outbox 전달: 재전달·순서·격리·동시 소비의 보장 범위가 문서와 맞는지. 소비자 중단 지점별 결과.
   - Q6. 쓰기 확장(V4 다음 단계)으로 가기 전에 정해야 할 것과, 창시자 결정이 필요한 것.
4. 발견마다 즉시 파일에 append한다. 심각도 P1(다음 단계 전 수정)/P2(권장)/P3(기록).
5. 확인하지 못한 것은 "확인 못함: <이유>". 발견이 없으면 "없음"과 확인 범위. 빈칸을 채우려고 발견을 만들지 않는다.

## 금지
- `codex-v4-r4.md` 외 파일 수정·생성 금지(`target/` 제외). 변형 실험은 임시 디렉터리 복사본에서 하고, DB는 네가 만든 임시 schema만 쓰고 지운다.
- `.env`·비밀 저장소 읽기 금지. git 명령 금지.

## 최종 응답
10줄 이하 요약 + 파일 경로.
