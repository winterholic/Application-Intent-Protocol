# V11 독립 검토와 반영

> 2026-10-04. Codex 메인 구현·PG 테스트·고장 주입·판정, `gpt-6-luna high` 단위 실패 대조 작성, `gpt-6-sol high` 설계와 실제 구현 독립 비평. Claude는 참여하지 않았다. 제품 전체 검사 범위 승인으로 표시하지 않는다.

| 검토 | 근거와 반영 |
|---|---|
| Sol: 실제 미검증 경계 | V1 docs digest 분리는 이미 검증했지만 optional on/off 도구 경로는 없었다. 기존 Output을 보존하는 작은 V11 adapter로 검사와 조언을 분리 |
| Sol: 옵션 무시 반례 | 잘못된 root JSON·devChecks 타입·모르는 옵션을 명시 오류로 테스트. raw read에서 devChecks 옵션을 보내는 우회도 UNKNOWN_KEY |
| Sol: docs 문자열과 사용 참조 | typed `call` 노드만 사용으로 세도록 제한. 설명에 orphan 이름을 적어도 조언 1인 음성 대조 |
| Sol: 위치·도달성 과장 | 조언은 predicate 이름 anchor만 제공. V1 docs span과 구분. 직접 호출받지만 전체 실행에서 미도달일 수 있는 predicate 한계를 테스트로 명시 |
| Luna: baseline 테스트 | 5형식 facts/TS 불변, docs 소속/위치, 옵션·정의 오류, raw read 거부를 작성. 메인이 테스트 helper의 Debug 제약과 cost fixture 선택 오류를 확인해 Luna가 수정한 뒤 동작 실패를 실행 |
| 메인: 조언 있는 정상 실행 | 실제 고립 predicate와 docs 원문/없음/거짓 권한 설명 × off/on × 4 actor를 PG에서 24조합 대조. 조언 때문에 실행을 막는 고장도 검출 |
| Sol: private filter 누락 | 초기 V11 raw 거부 표에는 비공개 filter의 직접 사례가 없었음. 메인이 코드 대조 후 private internalNote.eq·닫힌 periodEnd.eq·title sort를 추가하고 재실행 |

최종 Sol 코드 대조에서는 선택적 조언이 Output.execution을 읽기만 하고 별도 반환하며, V1 필수 검사와 V2 실행 경계를 우회하는 추가 반례를 찾지 못했다. 검토가 지적한 private filter 커버리지 간극은 메인이 테스트로 닫았다. 명령과 한계는 [V11 결과](../alignment/V11-optional-checks-results.md)를 따른다.
