# 통합 지침 기준 독립 문법 대조

> 2026-10-03. 검토자: Codex Luna high. 작성/판정: Codex 메인.
> 검토 범위: 새 원문, alignment/B·C 문서. source/DX 자료 수집과 이 대조는 읽기 전용으로 수행했다. Claude Code는 토큰 소진으로 이번 검토에 참여하지 않았다.

## 검토 과제

A/E/H EQ-01~11 동등성, [FOUNDER]/[DIRECTION]/[OPEN] 혼동, metadata/필수 검사 경계, read/write/aggregate/worker의 구체 반례, 미구현 예시의 실행 주장 여부를 대조했다. 전체 구조 재설계·실행 검증·창시자 승인은 범위 밖이다.

## 발견과 메인 판정

| 발견 | 메인 재확인 | 반영 |
|---|---|---|
| internalNote fieldRead 정책은 있지만 select 허용 목록에 필드가 없음 | C A expose/H read 목록과 EQ-05 대조해 누락 확인 | 두 select 목록에 필드 추가. 역할별 계약 투영·존재 비노출 검증은 남김 |
| 개인 Bookmark rows를 적용하면 총수가 본인 북마크 수가 됨. 기존 count 문법은 집계 source scope 불명확 | C Bookmark 정책과 aggregate 정의 대조해 모호성 확인 | 집계 전용 sourceAccess·groupKey·callerFilter 금지·rowOutput 금지 명시. 실제 강제/통계 추론 검증 별도 |
| stats가 미선언 Apply.aggregate/approvedCount를 참조 | C의 비교 정의에 Apply와 metric이 없는 점 확인 | Apply.approvedCount 입력·출력·관리자 scope·출처 연결. 완전 fixture는 V1 과제 |

검토자는 FOUNDER 자동 기술 승인이나 신규 문법을 구현된 것으로 표기한 문장은 추가 발견하지 못했다. 이 결과를 전체 문서 정합성·안전성 보증으로 해석하지 않는다.

## 남은 한계

- E 포장 예시는 생략형이다. A의 생략 없는 전체 블록을 넣은 실제 추출 비교는 미실행.
- C의 enum·active/managerOf·불변식 기호는 완전 정의 fixture가 필요하다. 후보 문법 parser/lowering 없음.
- W1은 실행 순서·최종/중간 정책·중복 ID·정보 노출의 구체 반례를 실행하지 않았다.
- Claude와 신규 기술 합의가 없으며, 이전 리뷰를 이번 원문 승인처럼 사용하지 않는다.

## 다음 검증

문서 상태/번호/링크 구조를 확인한 뒤 [E](../alignment/E-technical-risks.md)의 V1 의미 fixture로 진행할 수 있다. 이번 대조 결과만으로 신규 runtime 구현을 확정하지 않는다.

## 후속 독립 정합성 점검

메인 추가 대조에서 stats의 clubId를 존재하지 않는 Apply 필드 where처럼 전달하던 호출을 named aggregate 입력으로 수정했다.

Luna high가 A~E와 현재 root/결정/topics 문장을 다시 대조했다. T13의 DD-11 ‘토론 중’ 표기와 DD-08 등의 옛 합의안 전제·머리말의 이력 구분 누락을 지적했다. 메인은 해당 문장을 실제 읽어 확인하고 topics 갱신 안내를 관련 기록/B 정본 링크로 통일했으며 모든 DD 리뷰 머리말을 ‘이전 지침 토론 기록’으로 표시했다. DD-08/10의 전제도 목적 확인과 기술 후보를 나눴다.

이 후속 점검 역시 source/문서 읽기다. Claude 재검토·runtime 실행·생산성 수치 검증은 수행하지 않았다.
