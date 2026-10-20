# 프로젝트 구성원 제거·탈퇴 — WBS 7.3.3 I04 (2026-10-20)

원본은 `47246becc445869ba73077baa0a53a683e3508ce`의 MIT 파일을 판독했다. EE 파일과 원본 서버는 실행하지 않았다.

| 원본 동작 | 근거 (`backend/src/`) | 포팅 |
|---|---|---|
| 관리자 제거와 본인 탈퇴는 별도 연산. 제거 API는 본인을 포함하면 거부 | `services/project-membership/project-membership-service.ts:426-457`, `:575-673`; `server/routes/v1/project-membership-router.ts:580,708-721` | `ProjectMembership.remove`와 `.leave` |
| 제거 대상 전체가 프로젝트 구성원이어야 하고 같은 트랜잭션에서 제거 | `project-membership-service.ts:443-449,499-572` | `remove` bulk 최대 100행. 허가되지 않은 행이 섞이면 전체 rollback |
| 탈퇴는 직접 구성원만 가능. 그룹만으로 접근하면 거부 | `project-membership-service.ts:575-614` | 본인 직접 membership만 지움. 그룹은 미포팅 |
| 마지막 관리자 유지 검사는 일부 제품만 적용 | `services/membership/membership-fns.ts:132-146`; `db/schemas/models.ts:475-495` | 원본 Secret Manager는 대상 아님. 이 포팅에서도 마지막 직접 관리자가 탈퇴할 수 있음 |
| 제거·탈퇴는 private secret 행을 삭제하지 않음 | `project-membership-service.ts:499-572,622-673` | 행은 보존하고 프로젝트 읽기 정책이 접근을 차단. 재가입하면 본인의 같은 행을 다시 볼 수 있음 |

## AIP 선언과 확인 범위

`app.aip`의 고정 역할 모델을 사용한다. `remove`는 `isProjectAdmin(actor, project) and user != actor`, `leave`는 `user = actor`로 서버가 판단한다. `ProjectMembership`은 본인 행 또는 읽을 수 있는 프로젝트 행만 보인다. `NO_ACCESS` 사용자도 자기 membership을 보고 탈퇴할 수 있지만 프로젝트·다른 사용자의 membership은 보이지 않는다. 숨겨진 프로젝트를 `project.eq`로 필터하면 참조의 읽기 정책도 적용되므로 본인 행 조회에는 `user.eq`를 사용한다.

`tests/test_project_membership.py`는 전용 PostgreSQL에서 다음을 확인한다.

- 관리자·일반 구성원·viewer·no-access·다른 조직·비구성원의 제거/탈퇴 경계.
- 본인 제거와 타인 탈퇴 거부, 다른 프로젝트/본인 행이 섞인 bulk의 원자적 거부.
- 제거 뒤 같은 Python SDK client의 프로젝트·시크릿·개인 재정의 읽기 및 쓰기 차단. 개인 재정의 보존과 재가입 뒤 같은 id의 재노출.
- 제거·탈퇴 감사 before image와 after null, 같은 idempotency key 재시도 때 감사 중복 없음.
- Linux에서는 격리 worker로 reminder job 성공을 먼저 확인하고, 제거 뒤 enqueue의 `WIRE.ACCESS_DENIED`를 검사한다. 로컬 macOS 실행은 worker를 시작하지 않으며 이 시험은 명시적으로 skip한다.

로컬 RED는 신규 9개 중 8개 실패(`WIRE.NOT_EXPOSED`, 본인 no-access 행 비표시), GREEN은 10개 중 서버·DB 시험 9개 통과/격리 worker 시험 1개 skip이다. 권한 조건 5개를 약화하는 음성 검사는 각각 해당 행동 시험의 실패로 검출한다. Linux CI의 실제 job 시험 결과는 해당 커밋 실행 로그로 별도 확인한다.

## 원본과 아직 다른 점

- EE `permissionService`·역할 집합 privilege boundary·approver policy는 읽거나 이식하지 않았다. 기존 `ADMIN` 규칙과 조직 관리자 암묵 접근은 원본 전체 권한과 동등하다고 주장하지 않는다.
- 그룹, 추가 권한, application membership, reminder recipient, project key, alert recipient 표 및 cleanup은 아직 없다. 조직 관리자나 그룹 등 별도 접근 경로가 있으면 직접 membership 삭제만으로 그 접근까지 없어진다는 뜻이 아니다.
- 원본 V1 프로젝트 탈퇴 거부는 레거시 버전이 없는 포팅에 적용하지 않았다. 사용자/조직 탈퇴·삭제, CLI·UI·SDK 화면 전환, 전체 I04 동등성은 미완이다.
- 새 정의로 새 schema를 만들어 검증한다. 기존 운영 schema의 upgrade·rollback을 이 시험이 검증하지는 않는다.
