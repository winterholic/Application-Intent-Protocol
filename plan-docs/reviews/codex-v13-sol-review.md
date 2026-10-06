# V13 독립 검토와 반영

> 2026-10-04. 메인이 구현·PG/HTTP 실행·판정, Luna high가 단위 테스트와 V2/V3/V5/V11 회귀, Sol high가 설계 비평과 새 컨텍스트 검증을 담당했다. Claude는 참여하지 않았다.

| 검토 | 실제 확인과 반영 |
|---|---|
| Sol: 숫자 제한과 문자열 표현 구분 | 같은 seed로 두 후보를 실행. 숫자 후보의 거부를 i64 전체 지원으로 표시하지 않음 |
| Sol: 처음부터 where apply하면 read 계약 검사 우회 | 메인이 잘못된 생성 binding의 첫 쓰기로 실제 커밋을 재현. SDK 기대 헤더·서버 DB 전 거부로 보강하고 같은 반례를 재실행 |
| 메인: 이미 커밋한 key의 다른 wire 재생 | principal namespace에 후보 wire를 포함. 같은 actor/key/본문의 숫자·문자열 결과와 실제 DB 변경을 대조 |
| 메인: 커밋 뒤 출력 거부 | changed/unchanged 변환을 멱등 기록·커밋 앞에 배치. 숫자 상한 밖 where 쓰기는 DB 불변 단언 |
| Sol: 이전 유실 뒤 mismatch | 이번 거부로 앞선 실행을 확정하지 않음. `attempts > 1` pending 보존·캐시 저장 보류를 Node 반례와 고장 주입으로 확인 |
| Sol: 타입 범위 과장 | optional brand는 숫자/문자열 기반 타입만 구분. 정규 십진 형식·안전 정수 범위는 런타임 필수 검사로 유지 |
| 메인·Luna: baseline 기대 불일치 | Legacy는 mode export를 추가하지 않아 기존 산출물 바이트 유지. 실제 scalar 출력은 wrapper가 아닌 scalar. 기대를 생산 코드와 대조 후 행동 red 확인 |

최신 guard에서 Sol이 `node --test v13-binding.test.ts`를 직접 실행해 3개 통과·0개 실패를 확인했다. 인증 뒤 DB 접근 전 계약 거부, read 캐시 비움, 이전 인증 세대 우선 처리와 pending 보존 책임 경계에서 추가 실제 결함은 찾지 못했다. raw 헤더 생략과 아직 타입화되지 않은 apply는 범위 한계다. 전체 프로토타입·최종 프로토콜 승인으로 확대하지 않는다. [실행 결과·한계](../alignment/V13-id-boundary-results.md).
