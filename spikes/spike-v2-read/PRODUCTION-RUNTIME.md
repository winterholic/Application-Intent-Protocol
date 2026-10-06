# 정책 lowering 런타임 예산

이 문서는 제품 `aip service`가 재사용하는 V2 실행 엔진에서 V1 typed facts를 SQL로 lowering할 때 적용하는 hard ceiling과 설계 근거를 기록한다. 숫자는 AIP 문법이나 wire protocol의 최종 호환성 결정이 아니다.

| 예산 | Hard ceiling | 검사 위치와 의미 |
|---|---:|---|
| 재귀 깊이 | 64 frames | `cond` 재귀 진입 전 검사하고, 성공·오류 모두 frame 수를 복구한다 |
| lowering 작업량 | 65,536 units | `cond`, `value`, path root, path field step을 누적 계측한다 |
| predicate body·값 복사 payload | 8 MiB | predicate body를 복제하기 전, literal·enum 값을 parameter에 복제하기 전에 누적 검사한다 |
| 단일 정책 SQL | 1 MiB | 각 lowering 결과와 path step SQL이 hard ceiling을 넘으면 거부한다 |
| SQL 생성·복사량 | 8 MiB | `cond` 반환 문자열과 관계 path의 누적 생성 문자열 길이를 계측한다 |
| 관계 경로 길이 | 64 field steps | SQL path를 순회하기 전에 검사한다 |

## §4 설계 관문

1. **실현하는 원칙과 긴장:** 서버가 정책을 실행할 때 입력 크기만 제한하지 않고 확장·복사 비용도 제한해 서버 권한과 안정성을 지킨다. 반복 확장되는 큰 상수는 원본이 작아도 실행 거부될 수 있어 표현 범위와 긴장한다.
2. **호출자 표현 범위:** 문법은 바꾸지 않는다. 단일 정의가 V1 source ceiling 안에 있더라도 predicate diamond가 literal payload를 반복 복사해 8 MiB를 넘으면 실행 단계에서 거부한다. 오류는 의미 오류가 아니라 lowering 자원 한도다.
3. **애플리케이션 개발자 부담:** 새 선언이나 서버별 설정을 요구하지 않는다. 기존 정의가 hard ceiling을 넘으면 명시 오류를 반환한다.
4. **서버 권한과 신뢰:** V1 검증 후의 typed facts만 정책으로 처리한다. 그래도 SQL lowering 경로는 hard ceiling을 독립 검사한다. body 크기는 predicate 이름별 최초 사용에서 iterative JSON walk로 한 번 계측해 cache하고, 매 확장 호출은 clone 전에 누적 payload 예산을 확인한다. 값 문자열도 parameter 소유 복사 전에 길이를 charge한다.
5. **JS/TS와 Python 경험:** 두 작성 형식이 같은 V1 facts와 V2 lowering을 사용하므로 런타임 상한은 작성 형식과 무관하다.
6. **한 의도, 한 표현:** 기본 ceiling은 고정되어 있고 호출자 입력으로 변경하지 않는다.
7. **수단과 목적:** 이 한도는 SQL compiler 구조를 언어 목표로 승격시키지 않고, 현재 구현의 재귀·문자열·payload 비용을 제한한다.
8. **창시자 결정 / Open:** `docs/DECISIONS.md`에 숫자별 제품 호환성 기준은 아직 없다. 64 depth/path, 65,536 work, 1 MiB SQL, 8 MiB 복사량은 현재 제품 실행 경로에 강제되는 운영 상한이다. 장기 호환성이나 모든 배포 환경의 최적값을 확정한 것은 아니다.

## 숫자 근거와 검증

V1 fixture는 source 1 MiB, parser expression 4,096 nodes/root, predicate 확장 16,384 units를 이미 제한한다. V2는 recursion frame과 path step을 각각 64로 제한하고, 한 SQL fragment를 1 MiB, 한 lowering context의 SQL 생성량 및 소유 payload 복사량을 각각 8 MiB로 제한한다. 8 MiB는 source hard ceiling의 8배를 두어 한 번의 유효 입력이 여러 번 확장되더라도 제한된 여유를 주면서, definition graph와 별도로 runtime 측정이 누락된 payload 복사를 차단하는 제안값이다.

회귀 시험은 1 MiB보다 작은 실제 A definition에서 두 개의 128 KiB 문자열 literal 비교를 leaf predicate로 만들고, 여섯 단계 diamond가 이를 64회 반복하도록 한다. V1은 node budget 안에서 이 facts를 수락한다. 기존 V2는 SQL placeholder 결과가 작다는 이유로 많은 body/value copy를 진행하고도 허용하는 RED가 재현됐다. 현재 V2는 body payload 크기를 한 번 cache해 계산하고 각 body clone 전에 charge하며, 각 parameter string clone 전에 다시 charge하므로 누적 8 MiB를 넘기기 전에 명시 오류를 반환한다.
