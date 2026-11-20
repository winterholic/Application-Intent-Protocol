# 보안 정책

## 취약점 신고

공개 issue나 pull request에 취약점의 세부 내용을 올리지 않는다. GitHub의 비공개 신고 기능을 쓴다.

1. 저장소의 **Security** 탭에서 **Report a vulnerability**를 연다. (비공개 취약점 신고는 저장소 설정 *Code security*에서 켜져 있어야 한다. 꺼져 있으면 아래 2번을 쓴다.)
2. 신고 기능을 쓸 수 없으면 저장소 소유자(`winterholic`)에게 GitHub 프로필의 연락처로 비공개 연락을 요청한다. 연락이 닿기 전까지 세부 내용은 공개하지 않는다.

신고에 포함할 것: 영향을 받는 commit SHA 또는 버전, 재현 절차(최소 정의 파일, 요청, 설정), 기대한 동작과 실제 동작, 영향 범위(어떤 행·권한·데이터가 노출되거나 변경되는지).

## 대응 목표

저장소는 개인 프로젝트이며 SLA를 약속하지 않는다. 다만 다음을 목표로 한다.

| 단계 | 목표 |
|---|---|
| 접수 확인 | 7일 이내 |
| 재현 여부와 심각도 판정 | 14일 이내 |
| 수정 또는 완화책 공개 | 심각도에 따라 판정 후 30~90일. 일정이 어긋나면 신고자에게 알린다 |

수정이 공개되면 CHANGELOG의 `Security` 항목에 기록한다. 신고자가 원하면 공로를 밝힌다.

## 지원 범위

1.0 출시 전에는 `main`의 최신 commit만 지원한다. 1.0 출시 후에는 가장 최근의 1.x 릴리스와 `main`을 지원한다. 지원 범위(PostgreSQL 17, 제품 `.aip` 경로 등)는 1.0 지원 범위 감사의 기능 매트릭스(브랜치 `release/1.0-audit-20261121`, `plan-docs/reviews/2026-11-21-1.0-release-audit.md`)를 따르며, 감사가 `main`에 병합되면 그 문서를 따른다.

범위에 들어가는 예: 정책 우회(`callerFilter`·권한 규칙이 거부해야 할 읽기·쓰기가 통과), 인증 토큰 검증 우회, worker 격리 탈출, SQL 주입, 비밀 값 노출, 서비스 거부를 유발하는 입력.

범위 밖: 지원하지 않는다고 문서화한 feature(MySQL 등 출시 묶음에서 빠진 것), PoC 명령(`aip run` 등), 로컬 개발용 `--dev-checks` 우회, 이미 알려진 한계로 문서에 적힌 항목([`product/OPERATIONS.md`](product/OPERATIONS.md), [`spikes/spike-v1-fixture/PRODUCTION-LIMITS.md`](spikes/spike-v1-fixture/PRODUCTION-LIMITS.md)).

## 의존성 보안 점검

`Dependencies` workflow가 `cargo audit`, `cargo deny`, `npm audit`를 PR, `main` push, 매주 월요일에 실행한다. 허용한 예외는 [`deny.toml`](deny.toml)의 `ignore`에 사유와 함께 있다. 새 권고가 나오면 CHANGELOG에 남긴다.

## 책임 있는 공개

수정이 나오거나 90일이 지나면(둘 중 이른 쪽, 합의로 연장 가능) 세부 내용을 공개할 수 있다.
