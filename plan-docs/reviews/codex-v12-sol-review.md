# V12 독립 검토와 반영

> 2026-10-04. Codex 메인 구현·실행·판정, `gpt-6-luna high` 타입/HTTP/계약 테스트와 V5/V11 회귀 대조, `gpt-6-sol high` 설계·실제 우회 재현·수정 후 독립 재검증. Claude는 참여하지 않았다.

| 검토 | 근거와 반영 |
|---|---|
| Sol: 타입 단언만 붙이는 연결 | 앱별 생성 Contract를 binding으로 결합하고 공통 generic core 사용. 서버와 같은 생성 helper의 읽기 식별값을 캐시 저장 전에 검사 |
| Sol: 해시 입력·권한 경계 | binding은 해시 뒤에 추가. 내부 정책 변경과 공개 타입 변경의 식별값 민감도를 실제 Rust 테스트로 대조. 권한/호환/전체 행 검증으로 표시하지 않음 |
| Sol: 캐시 hit와 구세대 응답 | 캐시 hit는 요청하지 않는 한계 명시. 구세대 확인을 식별값 검사/전체 캐시 비움보다 앞에 둠. 늦은 구세대 응답 경합 및 고장 주입 검출 |
| Sol·메인: binding 누락 시 검사 해제 | `{}`/undefined 식별값에서 잘못된 서버 계약이 저장되는 실제 반례. 생성 시 필수 형식 검사 후 Node 실패 대조 재실행 |
| Sol: 변경 가능한 cache 주입 | `cache.read`로 임의 행을 넣으면 typed read가 hit로 반환. 메인의 실패 대조 후 typed facade는 `cache.size`만 노출. runtime API 및 tsc 음성 반례로 고정 |
| Luna: variant 회귀 | 옛 adapter 한 파일 복사로 generic import가 끊어져 V5 실패. 메인이 상위 공통 core를 참조하도록 fixture 수정하고 전체 Rust 6개 재실행 |
| 메인: 미확정 수·readonly primitive | 불일치 후 pending 보류와 재시도 확정 후 저장 재개 검사. branded number Id를 객체로 펼치지 않는 DeepReadonly와 선택 결과의 정확 타입 대조 |

최신 코드에서 Sol이 직접 `node --test v12-contract.test.ts`를 실행해 10개 실패 0, strict tsc 정상/음성 파일 각각 exit 0을 확인했다. 앞서 재현한 두 우회가 닫혔고 추가 V12 범위의 재현 결함은 찾지 못했다. 이후 메인은 pending 복구 단언과 생성 모듈의 Id import를 보강하고 실제 HTTP/PG 통합 테스트를 재실행했다. 이 판정은 전체 프로토타입이나 최종 제품 승인으로 확대하지 않는다. [실행 결과·한계](../alignment/V12-typed-transport-results.md).
