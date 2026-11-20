# 기여 안내

AIP는 개인 저장소이며 기여는 pull request로 받는다. 변경 전에 [원칙](docs/PRINCIPLES.md)과 [결정 기록](docs/DECISIONS.md)을 읽는다.

## 개발 환경

- Rust(`product/GETTING-STARTED.md`의 검증 버전), Node, Python 3, 시험용 PostgreSQL 17.
- 시험 DB는 전용 인스턴스를 쓴다. `AIP_TEST_DATABASE_URL`, `AIP_TEST_ADMIN_URL`은 개발용 DB가 아닌 시험 전용 서버를 가리켜야 한다. 기존 로컬 DB나 컨테이너를 재사용하지 않는다.
- 처음 설정과 반복 검증 명령은 [`product/VALIDATION.md`](product/VALIDATION.md)를 따른다.

## 변경을 올리기 전

```sh
bash tools/verify.sh
```

`tools/verify.sh`는 포맷, clippy, workspace 시험, 독립 엔진(`spikes/spike-v1`~`v6`, `spike-v11-dev-checks`) 시험, 클라이언트 시험을 모두 실행한다. 변경한 엔진 하나만 확인할 때는 해당 `Cargo.toml`에 `cargo test --manifest-path spikes/<engine>/Cargo.toml --locked`를 쓴다.

의존성을 바꿨다면 `bash tools/dependency-check.sh`(cargo-audit, cargo-deny, npm audit 필요)도 실행한다.

## 규칙

- 시험을 약화하지 않는다. 기한·기대값을 완화하거나 범위를 줄여 실패를 없애지 않는다. 간헐 실패는 재현 조건을 기록한다.
- 버그를 고치면 고치기 전 코드에서 실패하는 회귀 시험을 함께 올린다.
- 주석은 이유를 적는다. 코드가 하는 일을 반복하지 않는다.
- 경로 필터가 있는 workflow(`database-contracts`, `worker-jobs-linux`, `flaskbb-*`, `deploy-checks`, `dependencies`)는 해당 경로를 건드리지 않은 PR에서는 돌지 않는다. 출시 후보 SHA에서는 [`release-candidate`](.github/workflows/release-candidate.yml) workflow가 필수 workflow 전부를 경로와 무관하게 실행한다.
- 보안 취약점은 issue로 올리지 말고 [`SECURITY.md`](SECURITY.md)를 따른다.

## 커밋과 PR

- 커밋 메시지는 `type(scope): 요약` 형식을 쓴다(`fix(worker): ...`, `test: ...`, `docs: ...`, `ci: ...`).
- PR 본문에 실행한 명령과 결과를 적는다. 실행하지 못한 검증은 `미실행: 이유`로 적는다.
- 사용자에게 보이는 변경은 [`CHANGELOG.md`](CHANGELOG.md)의 `Unreleased`에 한 줄 추가한다.
- `main`에는 직접 push하지 않고 PR로 병합한다(권장 설정은 [`docs/BRANCH-PROTECTION.md`](docs/BRANCH-PROTECTION.md)).

## 릴리스

1. 출시 후보 브랜치 `release/<버전>`을 만들면 `release-candidate`가 자동으로 실행된다(수동 실행도 가능: `gh workflow run release-candidate.yml --ref <브랜치>`).
2. `required (all workflows green on this SHA)` job이 성공이고 로그의 SHA가 태그할 SHA와 같을 때만 `v*` 태그를 만든다.
3. 태그 push에서도 같은 workflow가 다시 실행된다. 태그 SHA에서 실패하면 릴리스를 게시하지 않는다.
