# E. 기술적 이견·위험과 검증 관문

> 상태: 검증 필요. [통합 지침](../sources/founder-integrated-directive-2026-10-03.md) §0·13~16 기준.
> 기존 Claude/Codex 합의는 이전 지침에서의 검토 이력이다. 이번에는 Claude Code 토큰 소진으로 참여하지 못했다. Codex 메인 + Luna high의 제한된 독립 대조를 수행했으며 Claude 합의/전체 안전성 검증으로 표시하지 않는다.

## 1. 지금 정리할 것

읽기·표준 쓰기·공식 확장·서버/SDK·metadata/검사 분리는 제품 기준이다. 아래 위험은 구체 설계 후보가 그 기준을 충족하는지 확인하기 위한 것이다. ‘위험이 있다’는 이유만으로 표현 자유를 줄이거나 제어 흐름 지원을 영구 기각하지 않는다.

| ID | 실제 근거·남은 가정 | 실험·정상/실패 사례 | 통과 조건·기각할 주장 |
|---|---|---|---|
| RK-01 | 현 HTTP/tsgen은 이름 호출. caller read tree 미구현. 정형 조합이 실제 화면 요구를 덮는지 미확인 | 모집 필드·logo·정렬을 열린/닫힌 capability로 비교, 잘못된 field/type/operator·과다 depth | 열린 계약이면 화면 요청만 변경, 닫힌 것은 명시 계약 변경. 모든 새 화면 ‘서버 0변경’ 주장 금지 |
| RK-02 | select/filter/sort/traverse/aggregate 정책 합성이 미구현. 공개 총수는 개인 행과 다른 승인 scope 필요 | 같은 데이터에서 타 학교·미공개 필드 필터·정렬·관계·임의 회원 집계·count/오류 추론 대조 | 정당 요청 결과 정확, 비인가 경로와 원본 행/분해 집계 거부. 집계 결과만 공개한다고 안전성 입증한 것은 아님 |
| RK-03 | 일반 intent 보편 시간/비용 상한 확인 못함. 미등록 읽기 운영 비용 미측정 | DB index 유무·관계 fan-out·동시 요청·큰 결과·plan 시간/statement timeout, 등록/동적 동등 의미 비교 | 비용 제한을 실행 전·실행 중·출력 단계로 검증. 등록 hash가 권한 부여를 대신하거나 오래된 권한 cache 사용 금지 |
| RK-04 | 쓰기 조합의 중간 상태·최종 불변식·잠금·부분 실패 안전성 미확인 | W0/W1/W2 승인·자기/동시 위임·누락/중복 ID·교차 tenant·빈 목록·상한 초과·재시도 | DB에 허용된 최종 상태만 commit, 대상 누락을 성공 처리 안 함. commit 불변식만 맞으면 중간 권한 위반 허용 주장 금지 |
| RK-05 | 공식 host worker/ABI 없음. 프로세스 분리만으로 자격증명·네트워크 접근 막을 수 없음 | worker가 ctx 외 DB/내부망 접근, actor 위장, timeout 후 쓰기, tx handle 재사용, 과다 반환 | 승인 access·actor/tenant·효과·기한을 서버가 강제하고 출력 검사. 규약·감사만으로 구조적 우회 불가 주장 금지 |
| RK-06 | .aip/호스트 block/정형 데이터 추출과 최소 의미 모델 비용 미측정 | C EQ-01~11 동등 facts·scope·source map, enum/predicate fixture·보간/임의 코드 거부, 기존 Core IR 대비 최소 모델 | 같은 의미의 필드/정책/집계/확장 빠짐없음. 메타데이터 digest·실행 digest 분리 비교. 방식 수 증가를 무조건 장점으로 보지 않음 |
| RK-07 | V1 resource docs 분리, V11 optional 조언/필수 검사 후보 실행. 검사 전체·field/command docs·공개 범위 미정 | docs 없는/다른/잘못 귀속된 선언, 검사 꺼짐, 없는 옵션, type/권한/비용 오류 | 설명 유무·내용은 execution facts 불변. 없는 옵션 실패. optional 검사 비실행해도 서버 필수 오류 거부 |
| RK-08 | 현재 dispatch prefix provider와 실제 외부 전달이 다름. 효과 원자성·retry 안전성은 새 모델 미검증 | DB rollback·commit 직후 worker crash·중복 전달·공급자 오류·기한 초과·보상 실패 | DB commit과 외부 효과 구분, outbox/전달 결과 관찰·멱등. 외부 호출까지 DB 트랜잭션 원자성이라고 보장 금지 |
| RK-09 | V5/V6/V9/V10/V12/V13/V14/V15에서 캐시·응답 유실·수명·세션·타입 연결 실행. tenant·즉시 통보·재연결 복구 미검증 | 새 회원 생성·집계 변경·별도 알림 tx·로그아웃/계정/tenant 변경·응답 유실·재연결 | 태그 안전 상위집합·실패 후 재조회, actor/tenant별 캐시 분리·제거. 행별 정확·낙관 상태 강제 주장 금지 |
| RK-10 | 마이그레이션 의도 선언과 승인 혼동 가능. 공개 계약/오류·호환 기간 미정 | 권한 확대·축소·기존 데이터 위반·필드 rename·roll-forward/back·구버전·없는/타 tenant 리소스 오류 | 데이터·정책·앱 복원 한계 명시. AI removed 선언은 사람 승인 아님. 보안 축소를 호환 유예로 늦추지 않음 |

신규 필드 기본 비공개, 승인·롤백·재조회·오류 상세도는 [DIRECTION]이다. 위 통과 조건은 기술 제안이며 정책 세부 채택의 창시자 승인으로 승격하지 않는다.

## 2. 이번 독립 비평의 실제 결과

기록: [통합 지침 문법 대조](../reviews/INTEGRATED-codex-luna-r1.md).

- C EQ-05는 관리자의 internalNote 열람을 요구했으나 선택 목록에 필드가 없었다. 실제 목록을 확인하고 A/H에 추가했다.
- bookmarkCount의 개인 read 정책과 총수 scope 관계가 선언에서 모호했다. sourceAccess·고정 groupKey·callerFilter/원본행 금지 후보를 명시했다. 새 scope의 안전성은 RK-02 미검증이다.
- stats가 없는 Apply 집계 계약을 참조했다. Apply.approvedCount와 동아리 관리자 scope를 A/H에 연결했다. enum/predicate의 완전 fixture와 실제 lowering은 RK-06 미검증이다.

이 수정은 의미 누락을 바로잡은 문서 변경이다. parser/planner/worker가 실제 강제한다는 실험 결과는 아니다.

## 3. 기술적 이견의 현재 상태

| 문제 | 비교할 입장 | 현재 판단과 되돌아올 신호 |
|---|---|---|
| 한 작성 문법 vs 여러 형식 | A/E 같은 문법의 포장, H host data, 여러 공식 방식 | A/E를 먼저 대조하는 에이전트 제안. 자동완성·진단·수정 비용에서 H가 낫다면 재검토. 어느 방식도 창시자 확정 아님 |
| 최소 typed tree vs 현재 Core IR | 요청·계약·기호의 최소 모델, 기존 Core IR 재사용 | 기존 자산 유지 후보. 변환 손실/불필요 단계/중복 facts가 크면 최소 모델. IR 자체 고도화가 목적은 아님 |
| 동적 읽기 vs 등록 제한 | 런타임 정책/비용 검증, 선택적 등록·registered-only 배포 | 동적 조합 수용 방향. 등록 강제가 화면별 서버 작업을 되살리는지 실측. 자원 통제 없이 자유만 넓히지 않음 |
| 조합 표준 vs 업무 동작/확장 | W0 단독 연산, W1 bounded composition, W2 표준 form/host extension | W0만 영구 표준으로 확정하지 않음. W1 안전성·표현력이 업무 실험을 통과하면 확대. 명확한 표준으로 안 덮는 업무는 공식 확장 |
| worker 신뢰·격리 | 제한된 ctx/프로세스/네트워크 경계, 신뢰 host 코드 | 새 runtime 경계 필요. 앱이 own DB credential을 제공하면 구조적 우회 불가 보장 범위에서 벗어남; 실제 threat model을 명시 |
| 태그 무효화 vs 넓은 재조회 | 서버 의존 태그·변경 집합, 활성 읽기 넓게 갱신 | 기본 재조회 방향 유지. 태그 누락·복구 비용이 크면 안전한 넓은 재조회부터; 실시간은 선택 후보 |

위 표는 기술 후보 간 미해결 문제다. Claude가 이번 후보에 반대한 것처럼 서술하지 않는다. 실제 Claude 재검토가 없다는 한계를 남긴다.

## 4. 작은 실험의 선후 관계

| 순서 | 범위 | 확인할 결과 | 다음으로 진행할 조건 |
|---|---|---|---|
| V0 이번 문서 점검 | A/B/C/D/E, 원문 연결, 번호·상태·파일 링크 | 누락·중복·stale F/G를 구조 검사/독립 읽기로 확인 | 문서 구조와 기준 충돌 수정 |
| V1 의미 fixture | 완전한 모집/Club/Bookmark/Apply 모델·기호·정책, A/E/H 추출 prototype | 같은 typed facts, 잘못된 기호/동적 평가 거부, metadata 분리 | 후보 문법의 최소 실행 가능성 확인 |
| V2 caller read vertical slice | isolated spike에서 정책 포함 1개 resource+1개 관계+고정 집계 | 열린/닫힌 capability, 출력 타입, SQL 바인딩, 비용 거부 | RK-01~03 정상·실패 동등성 |
| V3 표준 쓰기·조합 | 알림 read, 승인/위임 W0/W1/W2 대조 | 불변식·잠금·누락 ID·atomic/batch·rollback | RK-04 안전성과 반복 감소 원자료 |
| V4 공식 worker | read 확장 후 write/외부효과 | ctx scope·취소·수명·출력·실패 기록 | RK-05·08의 보장 범위를 증명 |
| V5 SDK·유지보수 | selected 타입·캐시·계약 diff·migration | 수정 파일/계약/AI 선택·성능 원자료 | RK-06~10 및 D의 측정으로 제품 선택 제안 |

V0는 문서 검증이다. 작은 실험이 현재 PoC를 덮어쓰거나 생산 DB를 변경하지 않도록 별도 spike에서 진행한다. 이번 요청은 A~E를 먼저 만드는 단계이며 대규모 구현 착수는 하지 않는다.

2026-10-04 실행 현황: V1 [결과](V1-semantic-fixture-results.md)(Codex r1·r2 반영), V2 [결과](V2-caller-read-results.md)(로컬 PG, Codex r2b 반영), V3 [설계](V3-standard-write-plan.md)·[결과](V3-standard-write-results.md)(V3-1 단일 전이, V3-2 승인 W0/W1/W2), V4 [결과](V4-official-extension-results.md)(V4-1 읽기 확장·격리 대안·V4-2 outbox), V5 [결과](V5-sdk-results.md)(V5-1 결과 타입 추론). RK-09는 읽기 의존 태그 건전성과 SDK 캐시 런타임(무효화·actor 분리·결과 미확정·늦은 응답)까지 실험했다. 전송 계층과 응답 유실 복구는 [V6](V6-transport-results.md)에서 최소 범위로 실험했다. 즉시 변경 통보는 미착수다. [V9](V9-cache-lifetime-results.md)에서 세션 수명 이하의 캐시 재사용, 시간 의존 조회 미캐시, 다른 연결의 변경·권한 회수 후 수명 만료 재조회, 미확정 쓰기 복구와 인증 입력 상한을 실행했다. [V10](V10-session-recovery-results.md)은 서버 principal 확인·같은 actor 토큰 교체 후 pending 자동 복구·인증 세대 경합·signed -1/익명 멱등 분리를 실행했다. 실제 로그인·refresh-token 제공자, 정확한 시간 유효 시각, 연결이 끊긴 동안 놓친 변경 복구는 남는다. RK-07은 [V11](V11-optional-checks-results.md)에서 옵션 off/on·metadata/생성 TS 불변·필수 타입/권한/비용 거부·PG 24조합·고장 주입을 대조했다. 설명의 인간 의도나 전체 제품 lint를 판정한 것은 아니다. [V12](V12-typed-transport-results.md)는 생성 읽기 타입을 HTTP·캐시·세션 경로에 결합하고 계약 불일치를 저장 전에 거부했다. 전체 행 구조 검증은 남는다. [V13](V13-id-boundary-results.md)은 큰 Id의 숫자 제한·십진 문자열 후보와 첫 쓰기의 계약 불일치 사전 거부를 실행했다. Id 최종 선택·일반 Int·bundle/compose/worker 변환은 남는다. [V14](V14-typed-apply-results.md)는 direct 공개 전이의 입력/결과 타입·응답 검사·불변 결과·공개 쓰기 지문을 실행했다. [V15](V15-filter-value-results.md)는 공개 scalar read/where 검사·리터럴 prefix·PG 시간 반올림·NUL 거부를 연결했다. 실행 가능한 설치/SDK/서버 흐름과 공식 확장의 통합은 남는다. RK-10은 [V7](V7-migration-results.md)에서 사전 검사(변경 분류·정책 확대 측정·데이터 위반 계산)까지 실험했다. 각 결과 문서의 spike 규칙은 기술 후보이며 이 표의 통과 조건을 충족했다는 승인으로 읽지 않는다.

## 5. 창시자 판단이 남는 경우

기술 비교 뒤 최초 클라이언트 지원 범위·초기 TS/Python 기능표·프론트 framework 출시 경험·구버전 지원 기간·라이선스는 가치/운영 선택이 남을 수 있다. 기술적 자료 없이 다시 25개 질문을 전부 묻지 않는다.

라이선스는 자동 선택하지 않았다. 이번 문서는 법적 조건 비교를 수행하지 않으며 오픈소스 공개/상업화/서드파티 무제한 접근을 같은 결정으로 취급하지 않는다.

## 변경 이력

- 2026-10-03 기술 후보 10개 위험·실패 사례·통과 조건과 V0~V5 분리. Claude 미참여 및 source 검사/실행 검증 한계 명시.
- 2026-10-04 V1·V2 실행 결과와 V3 설계 연결.
