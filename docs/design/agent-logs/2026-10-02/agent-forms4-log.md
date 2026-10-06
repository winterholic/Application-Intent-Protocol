# forms4 log

## Part A (대리 세션 재확인)
- 구현: ImpersonationSpec.operator_check(Sql) 를 플랜이 싣는다(`by` OR superuser 를 운영자에 대해 컴파일; actor 마커 -> `(SELECT operator FROM s)`, `__imp` -> NULL). 엔진 open_session 이 `WITH s AS (세션 열림 조건) SELECT operator, (operator_check) FROM s` 한 왕복. false 면 `ended_at=now()` 후 IMPERSONATION_ENDED(UNAUTHENTICATED) 로 기존 종료 세션과 같은 응답.
- e2e: cms_e2e impersonation_rechecks_the_operator (세션 시작 -> DB로 role USER 변경 -> 대리 command/query 거부 + ended_at 기록 + 데이터 불변).
- negative control: (1) 플랜 수준 operator_check = true 로 치환 -> 거부 단언 실패 대신 세션 지속(통과 확인 단언). (2) 엔진 소스 변이(검사 결과 무시 `|| true`) 시 e2e 실패 확인 후 원복+touch.
