# V14 독립 검토와 반영

> 2026-10-04. 메인이 테스트·구현·PG/HTTP·회귀·고장 주입을 수행했다. Sol high는 설계와 실제 타입/결과 변조 반례를 독립 검증했다. Luna high는 사용 한도로 작업하지 못했고 Claude는 미참여다.

| 검토 | 실제 확인과 반영 |
|---|---|
| 읽기 없는 공개 쓰기 누락 | 읽기 Contract와 별도 ApplyContract를 같은 binding에 결합. WriteOnly.mark 생성·실제 변경·결과 Id 추론 |
| 쓰기 변경에도 같은 지문 | V13 생성기는 exposeApply를 읽지 않음 확인. V14 공개 projection을 해시에 포함하고 쓰기 bulk만 다른 첫 요청 사전 거부 |
| where의 실제 지원 범위 | V3는 eq와 일부 값만 처리. 읽기 gte 과허용 차단, 미지원 Time where는 never로 표시하고 후속 parser 과제로 기록 |
| binding Id mode 누락 | 생성 시 필수 형식 검사·값 고정. JS 객체의 누락·잘못된 값·연결 뒤 변경을 실제 반례로 확인 |
| generic subtype의 초과 키 | Sol 임시 strict tsc가 request/target/filter 오타를 허용. 메인이 음성 marker 3개 실패를 관측하고 action별 닫힌 signature 적용 |
| readonly 결과가 실제로 변경됨 | Sol Node에서 changed.push 성공과 미동결 관측. 메인 실패 대조 후 후보 settle·recovered·복구 항목/목록·사전 충돌 오류 동결 |
| 복구 결과의 의미 | recovered는 확정 오류에도 존재. retryPending는 아직 미확정인 키를 생략. union 타입과 세션 교체·재시도 검사 유지 |
| 로컬 key가 원격 추가 속성으로 교체 | 메인이 123으로 덮어써지는 Node 반례 검출. 복구 항목의 로컬 key를 마지막에 붙여 타입/귀속 유지 |

Sol은 최신 strict tsc exit 0과 Node 7개 실패 0을 직접 확인했다. 조건부 action union의 ids는 허용하고 WriteOnly가 섞인 where는 거부하는 것도 임시 타입 대조로 확인했다. 메인은 이후 빠른 멱등 충돌의 동결까지 추가해 Node 8개와 전체 PG/HTTP 회귀를 실행했다. 추가 V14 책임 경계의 재현 결함은 찾지 못했으나 최종 제품 승인으로 확대하지 않는다. [실행 결과·한계](../alignment/V14-typed-apply-results.md).
