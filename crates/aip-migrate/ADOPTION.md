# 기존 프로토타입 schema의 제품 이관

## 구현 전 설계 관문

1. 원칙 1·2·4·6: 기존 데이터를 버리지 않고 공통 운영 인증과 배포 이력으로 옮긴다. 운영자의 검토 절차가 추가된다.
2. read/direct apply 표현은 그대로이며 이관으로 호출자 권한을 늘리지 않는다.
3. 앱은 기존 정의를 제출한다. 앱별 복사 SQL이나 새 서버 endpoint를 요구하지 않는다.
4. 기존 DDL digest와 실제 catalog marker를 검증하고 계획을 현재 정의·구조에 고정한다. 토큰·actor 연결은 별도로 운영자가 설정하며 개발 토큰을 승격하지 않는다.
5. 다섯 형식에서 얻은 동일 execution facts를 사용한다. JS/Python 확장의 구현은 복제하지 않는다.
6. 제품 저널로 전환한 후 기존 prototype marker를 함께 정본으로 사용하지 않는다.
7. 기존 V2 생성 DDL과 catalog 검사를 재사용한다. 같은 트랜잭션에 운영 metadata를 만들고 앱 데이터와 멱등 결과를 유지한다.
8. 이관은 사용자가 승인한 제품 통합의 구현 수단이다. 최종 인증 제공자·구버전 계약 공존·Id wire는 영구 확정하지 않는다.

`plan`은 읽기 전용이며 `apply`는 계획 digest를 요구한다. 이관 대상은 현재 prototype의 구조 marker가 있는 schema다. marker가 없거나 실제 구조가 달라진 schema를 추측해서 채택하지 않는다. 원래 prototype marker는 정책 본문을 저장하지 않으므로 이관 계획의 execution digest가 새 정책 정본이다. 운영자가 이를 검토해야 한다. 새로운 제품 저널·principal 연결·불변성 trigger를 생성하고 prototype marker만 제거한다. 앱 행과 `aip_idem` 결과는 보존하며, 목표 fresh 구조와 같은지 검증한다.

제품 CLI는 `plan_with_wire`/`apply_with_wire`를 사용한다. 계획에 wire와 기존 멱등 기록 검사를 포함하고, 제품 metadata·wire 고정·저널 기록·prototype marker 제거를 같은 트랜잭션으로 묶는다. 다른 wire를 요청하면 기존 marker와 데이터를 유지한 채 거부한다. 같은 계획 재시도는 최신 저널·facts·실제 구조·wire를 대조해 `alreadyApplied`로 확인하며 저널을 중복 생성하지 않는다.

`crates/aip-cli/tests/adoption_flow.rs`는 실제 이전 V6 서버가 decimal ID로 커밋한 쓰기를 JWT 제품 서버에서 같은 키로 재생한다. 직접 심은 임의 principal 행의 보존만으로 이관을 보증하지 않는다. 이 테스트는 이전 prototype과 같은 V1/V2/V6 엔진 경로를 사용하고, prototype CLI binary 자체를 실행하는 검증과는 구분한다.
