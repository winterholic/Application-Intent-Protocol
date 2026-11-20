# Changelog

형식은 [Keep a Changelog](https://keepachangelog.com/ko/1.1.0/)를 따르고 버전은 [Semantic Versioning](https://semver.org/lang/ko/)을 따른다.

## [Unreleased]

### Added
- `release-candidate` workflow: 필수 workflow 전부를 경로 필터 없이 한 SHA에서 실행하고 모두 `success`일 때만 통과하는 집계 job (`.github/workflows/release-candidate.yml`).
- `SECURITY.md`, `CONTRIBUTING.md`, 이 파일.
- 문서: [1.0 지원 범위](product/SUPPORT.md)(지원 11, 조건부 7, 제외 1, PostgreSQL 17 한정, 공개 count, 계약 헤더 오류 코드), [표현력 한계와 확장 우회](product/LIMITS.md)(E1~E7), COMPATIBILITY의 1.0 호환 약속과 정의 검사 변경.

### Changed
- `tools/verify.sh`의 엔진 clippy·test 목록에 `spike-v11-dev-checks`를 추가했다(제품 `load` 경로가 호출하는 엔진).
- README 맨 위에 1.0 상태를 명시했다: 정식 경로는 `aip service`, `.aipoc`는 PoC, tenant 자동 격리는 1.0 제외. 시작 가이드는 저장소 checkout 없이 릴리스 묶음(`AIP_HOME`)만으로 따라 하도록 고치고, 참조 열 이름 규칙(`<필드>_id`), `Time` 출력 표기, 소스 빌드의 SDK 도구 준비, `PG*` 환경 변수와 release 빌드의 관계를 적었다.

## [1.0.0] - 미정 (초안)

> **초안**: 1.0.0은 아직 출시되지 않았다. 아래 항목은 현재 `main`의 내용이며 태그 전에 다시 확인한다.
> 패키징 결과와 출시 게이트가 정해지면 갱신한다. 출시 전 차단 목록은
> 1.0 감사(브랜치 `release/1.0-audit-20261121`의 `plan-docs/reviews/2026-11-21-1.0-release-audit.md`)를 따른다.

### Added
- 제품 서비스(`aip service`): check, gen, init, adopt, migrate, serve, principal. RS256 JWT 인증과 공개 JWKS, PostgreSQL 17의 읽기·쓰기 엔진, Node/Python 확장 worker, 설치형 SDK(`@aip/sdk`, Python).
- 일반 마이그레이션(변경 계획, 정형 backfill/convert, DDL 트랜잭션과 배포 저널)과 기존 데이터 이관.
- 앱 패키지 설치·변경 배포, Docker·네이티브 서비스 실행 경로.
- Linux worker 격리(Bubblewrap, 선택적 cgroup v2 한도)와 macOS Seatbelt 격리(수동 검증).
- FlaskBB 포팅: Topic.move, Post.unhide, PostOrder 계약.
- 1.0 릴리스 게이트 스크립트(`scripts/release-gate.sh`): 새 clone 전체 회귀, 묶음 smoke, 업그레이드·롤백 드릴.

### Changed
- **호환 변경: `actor`를 둘 이상 선언하면 `DEF.MULTIPLE_ACTORS`로 거부한다.** 이전에는 마지막 선언이 조용히 이겼다. 정의에 `actor` 한 줄만 남긴다.
- **호환 변경: 원본에 `rows read`가 있는 `callerFilter read` 집계는 `where`가 그 정책을 함의해야 한다.** 아니면 `DEF.CALLER_FILTER_POLICY_REQUIRED` 오류다(호출자의 필터로 숨은 행의 값을 셀 수 있었다). 이전에는 `where`가 없을 때만 `--dev-checks`에서 조언이 나왔다. 저장소의 `.aip` 36개는 영향이 없었다. 저장소 밖 정의는 배포 전에 `aip service check`로 확인한다.
- **`where` 없는 `count`는 `--dev-checks` 없이도 `DEF.ADVISORY_PUBLIC_COUNT` 경고를 낸다**(정의는 통과한다). 공개 count의 응답 값과 계약 지문은 바뀌지 않았다.
- 정의 검사 진단 개선(모두 이전에도 거부되던 입력): `DEF.TRAVERSE_VIA_FORM`, Decimal 리터럴의 `DEF.BAD_VALUE`, `transition`의 `from`·`allow` 누락 안내. `to` 절의 구분자는 쉼표라는 안내로 문서와 진단을 맞췄다.
- 호환 약속(같은 protocol major의 허용 변경, 계약 지문, 구버전 동시 지원 없음, 롤백은 백업 복원)과 `aip-protocol/1.0`·`aip-core/0.11`을 함께 두는 이유를 COMPATIBILITY.md에 정리했다.
- worker runtime probe가 5초 단일 예산을 쓰고, 시간 초과 시 probe 프로세스 트리를 정리한다.
- Post.unhide가 upstream의 restore 의미와 일치한다.

### Fixed
- G6 effect 보존과 source-order import 검증.
- rustix 보안 권고에 따른 lockfile 갱신.

### Security
- 의존성 권고 점검을 CI에서 주간 실행한다. 허용된 예외는 `RUSTSEC-2023-0071`(RSA 서명 검증만 사용, 사유는 `deny.toml`).

### Known limitations
- PostgreSQL 17만 1.0 지원 범위다. MySQL은 출시 묶음에 포함되지 않는다.
- 공개 count는 `rows read`를 적용하지 않는 계약을 유지한다. 호출자별로 값을 줄이는 변경(명시 공개 표기, 모든 count에 `rows read` 적용)은 소유자 결정으로 1.0에 넣지 않았다.
- tenant 자동 격리는 1.0 지원 범위 밖이다(Core PoC에만 있다). 조직 경계는 정의의 조건으로 직접 쓴다.
- 선언만으로 표현하지 못하는 요구(부모·자식 동시 생성, 가변량 증감, 계산 집계, nullable 정렬과 cursor 등)는 [표현력 한계와 확장 우회](product/LIMITS.md)를 따른다.
- 지원 범위(지원 11, 조건부 7, 제외 1)는 [1.0 지원 범위](product/SUPPORT.md)가 기준이다.

[Unreleased]: https://github.com/winterholic/Application-Intent-Protocol/commits/main
[1.0.0]: https://github.com/winterholic/Application-Intent-Protocol/releases/tag/v1.0.0
