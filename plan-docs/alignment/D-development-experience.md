# D. 핵심 개발 경험 비교와 검증 범위

> 기준: [통합 지침](../sources/founder-integrated-directive-2026-10-03.md) §1.3·13.
> 결과 수준: 실제 소스 대조 + 설계 변경 영향 분석. 2026-10-04 기준 문법·읽기·쓰기·공식 worker·SDK를 독립 spike에서 실행했다([V1](V1-semantic-fixture-results.md)·[V2](V2-caller-read-results.md)·[V3](V3-standard-write-results.md)·[V4](V4-official-extension-results.md)·[V5](V5-sdk-results.md)). 전송·수명·세션 복구는 [V6](V6-transport-results.md)·[V9](V9-cache-lifetime-results.md)·[V10](V10-session-recovery-results.md) 결과를 따른다. [V12](V12-typed-transport-results.md)는 선택형 read 타입과 실제 HTTP·캐시를 연결했다. [V13](V13-id-boundary-results.md)은 공통 서버 경계에서 숫자 제한·십진 문자열 Id 왕복을 비교해 화면별 변환 없이 같은 행을 변경하는 후보를 검증했다. [V14](V14-typed-apply-results.md)는 direct 공개 전이 입력·결과·복구 타입을 생성 binding에 연결했다. [V15](V15-filter-value-results.md)는 공개 scalar 필터와 prefix 실행을 연결했다. 설치부터 정의 변경까지 실제 개발자 흐름을 잇는 통합 검증은 남는다. 이 문서의 DX·생산성 비교 자체는 미실행이다. 아래 ‘줄어든다’는 측정 결과가 아니라 전제부 설계 예측이다.

## 1. 비교 기준과 실제 표본

아리아리 원본은 `ariari/ariari` 한 트리만 사용했다. TEST·backup 복제본은 측정 분모에 섞지 않았다. 기존 endpoint/줄 수 총계는 재측정하지 않는다.

| 근거 | 이번에 확인한 내용 |
|---|---|
| [RecruitmentController](../../../ariari/ariari/ariari-backend/src/main/java/com/ariari/ariari/domain/recruitment/recruitment/RecruitmentController.java) 72행, [RecruitmentListService](../../../ariari/ariari/ariari-backend/src/main/java/com/ariari/ariari/domain/recruitment/recruitment/RecruitmentListService.java) 34행 | 요청 조건·페이지 전달, 사용자 학교 조회·교내 인증 |
| [RecruitmentRepositoryImpl](../../../ariari/ariari/ariari-backend/src/main/java/com/ariari/ariari/domain/recruitment/recruitment/RecruitmentRepositoryImpl.java) 38·50·59행 | 가시성 조건·count 조건, 호출자 sort 문자열의 PathBuilder 사용 |
| [RecruitmentData](../../../ariari/ariari/ariari-backend/src/main/java/com/ariari/ariari/domain/recruitment/recruitment/dto/RecruitmentData.java) 31·75행, [프론트 api](../../../ariari/ariari/ariari-frontend/app/api/recruitment/api.ts), [프론트 타입](../../../ariari/ariari/ariari-frontend/app/types/recruitment.ts) | 평면 응답 DTO·명시적 호출 래퍼·대응 TS 타입. 동아리 로고/총 북마크 수 필드는 해당 DTO에 없음 |
| [MemberAlarmService](../../../ariari/ariari/ariari-backend/src/main/java/com/ariari/ariari/domain/member/alarm/MemberAlarmService.java) 68행 | ID+소유자 조회, 미읽음만 읽음 변경. [mutation hook](../../../ariari/ariari/ariari-frontend/app/hooks/notification/useNotificationMutation.tsx) 27행에서 알림 읽기 캐시 무효화 |
| [ApplyService](../../../ariari/ariari/ariari-backend/src/main/java/com/ariari/ariari/domain/recruitment/apply/ApplyService.java) 94행 | 트랜잭션, 같은 동아리·MANAGER 이상·거절 상태·기존 회원 검사, 회원 생성·알림 이벤트 |
| [승인 mutation hook](../../../ariari/ariari/ariari-frontend/app/hooks/apply/useApplicationMutation.tsx) 30행 | 지원 목록·각 상세 무효화. 이 hook에서는 새 회원으로 인한 회원 목록 갱신을 찾지 못함. 앱 전체의 최종 UI 오동작은 미재현 |
| [ClubMemberService](../../../ariari/ariari/ariari-backend/src/main/java/com/ariari/ariari/domain/club/clubmember/ClubMemberService.java) 74행 | 위임 대상 ADMIN, 요청자 GENERAL 두 쓰기. 자기 위임의 실제 오류는 미재현 |
| [KakaoAuthManager](../../../ariari/ariari/ariari-backend/src/main/java/com/ariari/ariari/commons/auth/oauth/KakaoAuthManager.java) 58·87·121행 | 외부 OAuth HTTP 연동 코드 경로 존재. 외부 호출 성공·보안·성능은 미검증 |
| [현 AIP 모집 query](../../examples/ariari/app.aip) 425행, [tsgen](../../crates/aip-contract/src/tsgen.rs) 238행 | 서버 select 고정·정렬 enum·이름 호출. 프론트 선택을 바꾸는 새 모델은 구현되지 않음 |

‘파일 수정 필요’는 검토해야 할 계층과 구분한다. 현재 요청·결과·정책으로 충분하면 controller나 service가 반드시 바뀌는 것은 아니다. 기존 코드가 허용하는 임의 sort는 AIP가 새로 만든 유연성으로 계산하지 않는다.

## 2. 요구된 10개 시나리오

| ID | 요구 | 기존 API에서 검토·수정할 부분 | 새 AIP 후보의 작성/변경 위치 | 없어질 반복 / 유지할 서버 책임 | 확인 수준 |
|---|---|---|---|---|---|
| SC-01 | 목록에 새 필드 | 결과 DTO·mapper·TS 타입·화면. 필드가 없으면 서버 응답 구현 변경 | 이미 공개된 필드는 화면 select만. 미공개면 리소스 capability+생성 타입 갱신 | 화면별 DTO 복사 감소 후보. 새 데이터/노출 정책 변경은 남음 | 실제 DTO/PoC 대조. [V2](V2-caller-read-results.md) spike에서 열린 필드는 요청만 변경, 닫힌 필드는 `FIELD_NOT_EXPOSED` 실행 확인(fixture 범위) |
| SC-02 | 관계 조회 | DTO 관계 매핑·조회 fetch 전략·FE 타입. Controller 추가는 요청 방식에 따라 다름 | 이미 허용된 club.name/logo traverse를 요청 | 화면별 조인/형태 래퍼 감소 후보. 간선/목적지 행 정책·N+1·비용은 엔진 책임 | 실제 flat DTO 확인. V2 spike에서 관계 select·대상 행 정책 재적용 실행 확인. N+1·비용은 미측정 |
| SC-03 | 검색·정렬 추가 | 조건 DTO·predicate·정렬. 현재 일부 임의 sort는 이미 지원 | 공개 filter/sort 연산을 프론트 where/orderBy로 표현 | 유사 엔드포인트와 필터 분기 복사 감소 후보. index·비용·filter/sort별 권한 정의는 남음 | 실제 PathBuilder 확인. V2 spike에서 허용 목록 밖 filter/sort 거부 실행 확인. index 비용 미측정 |
| SC-04 | 사용자별 접근 | list service 학교·인증, repository 가시성/count 조건 | 서버 행 정책 한 정의를 요청의 모든 scan/관계/집계에 합성 | 경로별 가시성 복사 감소 후보. actor/tenant/재평가 책임 유지 | 실제 service/repository 확인. V2 spike에서 학교·익명·관계·집계 경로 정책 합성 실행 확인(fixture 범위) |
| SC-05 | 내 알림 읽음 | Controller·Service·FE 래퍼·mutation의 단건 처리 | 한 transition 계약 + 화면 apply. 모두 읽음이면 bulk capability 사용 | 화면마다 command/일괄 service 반복 감소 후보. 소유자·이전 상태·상한·원자성 유지 | 실제 readAlarm 확인. [V3-1](V3-standard-write-results.md) spike에서 단건·where·상한·원자성·동시성 실행 확인 |
| SC-06 | 여러 데이터 업무 변경 | ApplyService의 상태·회원·권한·알림 및 FE 무효화 | W1 표준 조합 또는 표준 approval/공식 확장 | DB plumbing/공통 불변식 처리 감소 후보. 업무 의미·동시성·누락 ID·알림 계약은 반드시 남음 | 실제 승인 코드/캐시 표본 확인. V3-2 W0/W1/W2·V4 쓰기 확장·V10 응답 유실 복구 실행. 생산성 비교 미측정 |
| SC-07 | 복잡한 업무 로직 | 관리자 위임 서비스·권한·동일 club·두 행 변경 | 표준 transfer 후보와 host 확장 동등 비교 | 모든 복잡 업무를 ‘서버 코드 0’으로 계산 못함. 최종 유일 관리자 불변식·잠금 유지 | 실제 위임 source 확인. V3-3 spike에서 자기·동시 위임 및 불변식 반례 실행. 원본 앱의 오류는 미재현 |
| SC-08 | 외부 API 연동 | KakaoAuthManager 외부 호출·응답·실패 관리 | official host extension/provider + 효과·자원 계약 | 외부 SDK를 다시 Rust로 구현할 필요 감소 후보. 공급자 특유 입력/오류 로직은 남음 | 실제 HTTP 경로 확인. V4 generic Node/Python worker는 실행했지만 Kakao OAuth provider 연동은 미검증 |
| SC-09 | 기존 요구 변경 | API 계약·서버 DTO·TS 타입·호출자 버전·캐시 | 화면 선택 변화와 서버 계약 변화 분리, 공개 계약 diff·버전 처리 | 화면 형식 변경의 이중 유지보수 감소 후보. 새 필드/정책/업무 의미·마이그레이션 유지 | 현 AIP compat/evolve는 재사용 후보. V7 계약 diff·DB 위반 사전 검사, V12 생성/서버 읽기 계약 불일치 거부 실행. 구버전 요청/SDK 호환은 미검증 |
| SC-10 | 잘못된 AI 코드 | host compiler/검증·테스트·리뷰로 찾는 유형별 오류 | 선택적 구조 검사 + 필수 server 타입/인가/비용 검사 | 동일 참조/계약 오류 탐색의 반복 감소 후보. 업무 요구 자체 오류는 자동 판정한다고 약속 못함 | 현 check 파이프라인 source 확인. V11 조언 off/on·필수 실행 검사 분리 실행. 새 lint 전체·AI 수정 비용 비교 미검증 |

## 3. 구체적인 변경 추적 예

### 새 모집 카드

기존 DTO에는 clubName은 있지만 logo·총 북마크 수가 없다. 화면 코드만 바꿔서 이 값을 새 응답으로 받는 것은 현재 계약상 불가능하다. 응답 DTO/mapper와 조회 비용, TS 타입/화면을 검토해야 한다. 정렬은 현재 문자열 경로로 될 수 있으므로 반드시 새 endpoint가 필요하다고 주장하지 않는다.

현재 AIP도 서버 query의 club select에는 name·affiliation만 있다. 기존 PoC로는 logo를 프론트가 추가 선택하지 못한다. 새 후보는 club.logo select/traverse와 집계 capability가 **이미 열려 있을 때만** 화면 요청 변화로 해결한다. 닫힌 계약을 처음 여는 일은 실제 서버 변경으로 센다.

### 알림 모두 읽음

한 번 정의한 ‘본인의 미읽음→읽음’ 계약을 단건과 제한된 대상 집합에 재사용하는 후보다. 프론트가 owner 조건을 쓰지 않아도 write 정책을 서버가 적용해야 한다. 기존 서비스 메서드를 그대로 이름 호출하는 것이 아니라 대상과 표준 전이를 표현한다.

DB bulk 상한을 넘으면 성공으로 잘라 처리하지 않는다. atomic 의미와 batch job 의미를 구분한다. 수행 규모와 재시도·멱등·오류 의미는 별도 검증한다.

### 지원자 승인과 캐시

실제 승인 서비스는 새 ClubMember 행을 만든다. 표본 mutation은 Apply 목록/상세만 무효화한다. 새 후보는 읽기 의존 태그와 커밋한 변경 집합을 엔진이 제공해 SDK가 갱신 대상을 고르는 것이다.

태그가 있다고 정확해지는 것은 아니다. 합계·viewer 파생 값·새 행 삽입·별도 알림 트랜잭션·응답 유실에 대한 안전한 과다 무효화/재조회 경로가 필요하다. ‘행 단위 정확 자동 동기화’라는 주장은 하지 않는다.

## 4. 측정 설계

| 측정 축 | 기록할 원자료 | 비교에서 지킬 조건 |
|---|---|---|
| 코드 작성량 | 새 hand-written 선언/함수, 삭제된 반복, 생성 산출물 | 생성 코드를 ‘개발자가 안 쓰므로 0’으로 은폐하지 않고 구분. 줄 수는 보조 |
| 변경 파일 | 실제 diff 파일과 역할 | 검토한 파일과 실제 바뀐 파일 구분. 초기 설정/계약을 실험 전 비용에 포함 |
| 서버 계약 | 신규 resource/capability/operation·변경·없음 | 기존에 열린 capability와 새 정책 확장을 같은 케이스로 섞지 않음 |
| 계약 중복 | 타입/검증/mapper/정책을 두 곳 수동 표현한 항목 | source 정의 하나 + 생성물은 중복 수동 유지보수와 구분 |
| AI 선택 | 같은 요구의 생성 결과, 사용 패턴, compile/repair 시도 | 동일 모델·입력·도구 조건을 기록. 이번 문서에서 AI 개선 수치 생성하지 않음 |
| 정적 검증 | 알려진 오류/정상 표본에 대한 정확 진단·위치 | unknown field·잘못된 연산·중복 정의·없는 옵션·잘못된 scope를 실패 사례로 둠 |
| 성능 | query 수·p50/p95·CPU/메모리·worker IPC | 같은 데이터·권한·관계·결과 의미·동시성. 측정 전 우월성 주장 금지 |
| 안전성 | 정당/부당 요청 결과·동시성·rollback·효과 기록 | 개인/공개 집계·filter/sort·tenant·취소·재시도·DB 제약을 각각 확인 |
| 운영·디버깅 | 기동 단계·로그/trace 연결·worker failure·복구 | compiler/SDK/worker/planner 유지 비용과 최초 설치 비용 함께 기록 |

### 4.1 작성량 원자료 (2026-10-04, 부분)

같은 업무에 대해 아리아리 백엔드에서 손으로 쓴 메서드와, spike에서 서버에 쓴 선언을 나란히 적는다. 줄 수는 보조 지표다. 아리아리 쪽은 해당 메서드 본문만 셌고 DTO·예외 클래스·인증 공통·테스트·프론트 래퍼는 세지 않았다. AIP 쪽은 엔진·SDK·마이그레이션 비용을 세지 않았다. 어느 쪽도 유지보수 시간 측정이 아니다.

| 업무 | 아리아리(손으로 쓴 것) | AIP spike(서버에 쓴 것) | 호출자 |
|---|---|---|---|
| 내 알림 읽음 | 컨트롤러 메서드 5줄 + 서비스 메서드 9줄 + 저장소 메서드 1줄 + 엔티티 메서드 3줄 | 전이 6줄 + `expose apply` 1줄(조건 대상까지 쓰려면 `expose read` filter 1줄) | `apply(MemberAlarm.read, {ids})` |
| 일괄 승인 | 컨트롤러 메서드 약 11줄 + 서비스 메서드 30줄(`ApplyService` 95~124행) + 저장소 쿼리 2줄 | W2: 전이 6줄 + 공개 1줄 + unique 1줄 | `apply(Apply.approve, {ids})` |

차이가 나는 곳은 줄 수보다 **같이 따라오는 보장**이다. AIP spike 쪽 선언만으로 누락 id 전체 실패, 권한 먼저, 잠금, 회원 중복 DB 제약, 커밋과 함께 남는 알림이 생겼다(V3 §6). 아리아리는 이 중 일부를 직접 구현하지 않았다. 반대로 AIP 쪽은 그 보장을 만드는 엔진 코드(spike 기준 수천 줄)가 있어야 하며, 그 비용은 프레임워크 쪽으로 옮겨진 것이다.

비교는 초기 계약 작성과 일상 화면 변경을 따로 기록한다. 프레임워크 개발자가 부담하는 compiler·planner·adapter 비용도 별도 기록한다. 앱 코드 감소가 비용을 다른 팀으로 옮겼을 뿐인지 판단할 수 있어야 한다.

## 5. 검증 순서와 이번 결과의 한계

1. 이번 source 표본 분석을 고정하고 시나리오별 의미·공개 계약을 정한다.
2. [C](C-syntax-proposal.md)의 A/E/H를 EQ 기준으로 정규화할 작은 실험을 설계한다.
3. 열린/닫힌 capability 케이스를 분리한 read/apply 실험을 수행한다.
4. 업무 조합/공식 확장/캐시 실패를 실제 서버·DB로 시험한다.
5. 원자료 diff·진단·query/trace를 남긴 뒤 생산성·성능을 평가한다.

이 문서는 1의 소스 대조와 2~5의 비교 설계다. 후속 spike의 기능 실행 근거는 위 결과 문서에 연결했지만, 제품 SDK/planner 통합과 기존 API 대비 절감률·유지보수 시간 개선은 입증하지 않았다. [E](E-technical-risks.md)에서 미수행 실험과 판단 관문을 관리한다.

## 변경 이력

- 2026-10-03 아리아리 원본 표본과 현재 AIP source 대조, 요청된 10개 시나리오·9개 측정 축 정리. 런타임·생산성 수치 미측정 명시.
- 2026-10-04 SC-01~05 확인 수준에 V2·V3-1 spike 실행 결과 연결. 생산성·성능 수치는 여전히 미측정.
- 2026-10-04 V3~V7·V9·V10 기능 실행 근거와 DX 비교 미측정 상태를 분리. SDK/worker 미실행이라는 낡은 설명 갱신.
