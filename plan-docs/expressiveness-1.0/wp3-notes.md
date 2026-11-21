# WP3 (P0-C 확장 create) 작업 중 알게 된 것

- **분기는 `run_write_in` 한 곳**: `ctx_create_in`이 `job_create_request` → `spike_v3_write::bundle_in` → `{"created":[{resource,id}]}`. 같은 파일의 `invoke_write_action`(MySQL·job 경로)에는 create가 이미 있었고 PG 경로(`run_write_in`)만 `ctx.create.unsupported`였다(E7).
- **`write_needs_serializable`**: access 키를 `Resource.transition`로만 읽던 것을 `create.<R>`이면 `resources[R].exposeCreate.isolation`을 보게 했다. `create`라는 이름의 resource는 없으므로 기존 전이 판정과 섞이지 않는다.
- **검사 순서**: v3 `bundle_run`은 `ensure_refs_readable`을 `create_row`의 allow 평가보다 먼저 한다. 읽을 수 없는 참조(남의 Member)를 쓰면 allow가 거짓이어도 `MISSING_TARGET`이 먼저 나온다. allow 거절을 시험하려면 참조는 읽을 수 있고(actor 본인) allow만 거짓인 입력이 필요하다(시험은 `actor.org > 0`인 org 0 멤버).
- **ALREADY_EXISTS**: PG 경로에서는 유일 제약 위반이 `WIRE.EXTENSION_ERROR`, 태그 `ctx.create.already_exists`로 끝난다. `invoke_write_action`(job)처럼 `WIRE.ALREADY_EXISTS`로 바꾸는 분기는 PG 경로에 없다(이번 범위 밖, test-plan K-4의 "원인"은 이 태그로 읽었다).
- **시험 구조**: `sqlgen::set_schema`는 프로세스당 한 번이라 한 test binary 안의 `#[tokio::test]`가 둘 이상이면 스키마가 충돌한다. `expr_ext_create.rs`는 시나리오 함수 넷을 한 테스트가 순서대로 부른다.
- **바깥 key 재생(C0-4)은 v4 시험으로 못 본다**: 재생은 v6 `apply_attempt`에 있고 v6가 v4에 의존한다. serve 실행으로 확인했다(아래).
- **serve 재현 절차**(임시 앱): `/apply` 확장 호출은 `x-aip-contract: <listening 줄의 fingerprint>` 헤더가 필요하다(없으면 `WIRE.CONTRACT_MISMATCH`). 접근 토큰 수명은 800초로 냈을 때 통과했고 3000초는 `WIRE.UNAUTHENTICATED`였다(상한은 코드로 확인하지 않음, 확인 필요). `principal bind`는 actor 행이 먼저 있어야 한다(`PRINCIPAL_FAILED`). JWKS를 다시 만들면 serve를 다시 띄워야 한다.
- **미반영(범위 밖)**: `ctx_request`의 `input` 키 전달은 WP1 병합 뒤 일이다. v3 쪽 수정은 필요하지 않았다.
