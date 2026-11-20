# Changelog

형식은 [Keep a Changelog](https://keepachangelog.com/ko/1.1.0/)를 따르고 버전은 [Semantic Versioning](https://semver.org/lang/ko/)을 따른다.

## [Unreleased]

### Added
- `release-candidate` workflow: 필수 workflow 전부를 경로 필터 없이 한 SHA에서 실행하고 모두 `success`일 때만 통과하는 집계 job (`.github/workflows/release-candidate.yml`).
- `SECURITY.md`, `CONTRIBUTING.md`, 이 파일.

### Changed
- `tools/verify.sh`의 엔진 clippy·test 목록에 `spike-v11-dev-checks`를 추가했다(제품 `load` 경로가 호출하는 엔진).

## [1.0.0] - 미정 (초안)

> **초안**: 1.0.0은 아직 출시되지 않았다. 아래 항목은 현재 `main`의 내용이며 태그 전에 다시 확인한다.
> 버전 문자열, 정책(count) 결정, 패키징 결과가 정해지면 갱신한다. 출시 전 차단 목록은
> 1.0 감사(브랜치 `release/1.0-audit-20261121`의 `plan-docs/reviews/2026-11-21-1.0-release-audit.md`)를 따른다.

### Added
- 제품 서비스(`aip service`): check, gen, init, adopt, migrate, serve, principal. RS256 JWT 인증과 공개 JWKS, PostgreSQL 17의 읽기·쓰기 엔진, Node/Python 확장 worker, 설치형 SDK(`@aip/sdk`, Python).
- 일반 마이그레이션(변경 계획, 정형 backfill/convert, DDL 트랜잭션과 배포 저널)과 기존 데이터 이관.
- 앱 패키지 설치·변경 배포, Docker·네이티브 서비스 실행 경로.
- Linux worker 격리(Bubblewrap, 선택적 cgroup v2 한도)와 macOS Seatbelt 격리(수동 검증).
- FlaskBB 포팅: Topic.move, Post.unhide, PostOrder 계약.
- 1.0 릴리스 게이트 스크립트(`scripts/release-gate.sh`): 새 clone 전체 회귀, 묶음 smoke, 업그레이드·롤백 드릴.

### Changed
- worker runtime probe가 5초 단일 예산을 쓰고, 시간 초과 시 probe 프로세스 트리를 정리한다.
- Post.unhide가 upstream의 restore 의미와 일치한다.

### Fixed
- G6 effect 보존과 source-order import 검증.
- rustix 보안 권고에 따른 lockfile 갱신.

### Security
- 의존성 권고 점검을 CI에서 주간 실행한다. 허용된 예외는 `RUSTSEC-2023-0071`(RSA 서명 검증만 사용, 사유는 `deny.toml`).

### Known limitations
- PostgreSQL 17만 1.0 지원 범위다. MySQL은 출시 묶음에 포함되지 않는다.
- 공개 count 집계의 `callerFilter` 동작은 정책 결정 대기 중이다(감사 P0-2).
- 출시 산출물의 버전 문자열은 패키징 작업에서 정한다(감사 P0-1).

[Unreleased]: https://github.com/winterholic/Application-Intent-Protocol/commits/main
[1.0.0]: https://github.com/winterholic/Application-Intent-Protocol/releases/tag/v1.0.0
