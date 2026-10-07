# `atMost N` 집행 확장 설계

> 상태: 구현·회귀 확인됨. 기존 `limit ... = atMost N where ...` 문법의 집행 범위를 N≥2까지 확장한다. 새 문법이나 쓰기 API를 추가하지 않는다.
> 기준: [AIP 원칙 §4](../../docs/PRINCIPLES.md), [최근 패턴·보안 점검](2026-10-07-pattern-security-audit.md), [PoC 이식 지도 B3](2026-10-07-poc-porting-map.md).

## 설계 관문

1. **실현·긴장 원칙:** 앱별 백엔드 반복을 줄이고(§2-1), 서버가 최종 권한·무결성을 집행한다(§2-2·4). 집계 잠금은 제한된 쓰기 처리량과 추가 검증 비용을 만든다.
2. **호출자 표현 범위:** 호출자 요청 모양과 권한은 그대로다. 서버 계약에서 보장할 수 있는 그룹별 행 수의 범위만 넓어진다.
3. **서버 작성량:** 개발자는 기존 limit/invariant 선언에서 N을 정한다. 앱별 endpoint·DTO·잠금 코드를 추가하지 않는다.
4. **신뢰 경계:** 호출자가 정원, 그룹, 조건을 지정하지 않는다. 서버 facts의 `per` 참조 필드, 조건, N을 의미 검사하고 AIP 쓰기 트랜잭션에서 집행한다. 잠긴 그룹의 전체 행 수를 검사하며 `rowRead`로 숨은 행을 빼지 않는다. 오류에는 그룹 id나 실제 행 수를 노출하지 않는다.
5. **생태계:** 다섯 정의 형식이 동일 execution facts로 내려간다. 언어별 SDK·런타임을 추가하지 않는다.
6. **표기 중복:** 기존 `limit ... atMost N ...` + `invariant ... per ref`를 그대로 사용한다. 기능별 별칭이나 새 문법을 만들지 않는다.
7. **수단과 목적:** `lockedCountCheck`, advisory lock, count SQL은 서버 무결성 보장을 위한 내부 집행 세부사항이다.
8. **미결정 침범:** create/update/delete 조합, W0/W1/W2, 부분 성공 정책을 정하지 않는다. 현재 OPEN 쓰기 결정과 독립적이다.

## 집행 경계와 경쟁 쓰기

N=1은 기존 부분 unique index로 유지해 기존 facts와 DDL을 바꾸지 않는다. N≥2는 `lockedCountCheck` fact로 나타내고 스키마 DDL은 생성하지 않는다. 조건은 기존 부분 인덱스와 같은 `index_safe` 범위, 즉 해당 행의 필드와 리터럴만 허용한다. 시간·actor·하위 조회 의존 조건은 거부한다.

각 쓰기 트랜잭션에서 제한된 resource에 변경 행이 있으면 resource별 PostgreSQL transaction advisory lock을 얻고, 별도 SQL command에서 제한 조건을 만족하는 행의 그룹 수를 검사한다. resource 이름의 정렬 순서로 잠금을 얻어 여러 resource를 함께 쓰는 요청의 잠금 순서를 고정한다. 모든 그룹 행을 대상으로 집계하고 `per IS NULL`은 그룹 제한에서 제외한다. 초과 시 트랜잭션 전체를 rollback하고 일반적인 `INVARIANT_VIOLATED`만 반환한다.

잠금 후 별도 count SQL을 실행해야 앞선 쓰기의 커밋을 새 스냅샷으로 관찰할 수 있으므로 모든 write transaction은 시작 직후 READ COMMITTED를 강제한다. 경계는 V3 `begin`, V4 worker transaction, V6 `/apply`, V6 write extension transaction을 포함한다. `checks_in`의 호출 위치에서 확장 쓰기 결과도 함께 검사한다. AIP 제품이 보장하는 쓰기는 공개 service/extension 경로다. 운영 DB 계정을 따로 제한하라는 제품 안내에 따라 직접 SQL 쓰기를 무결성 보장 범위에 넣지 않는다. 기존 `check`도 v3 쓰기 경로의 현재 트랜잭션 변경 행만 검사한다.

## 마이그레이션

제품 migration 정본은 `crates/aip-migrate`다. spike-v7 판정만으로 제품 migration을 대체하지 않는다. 새·변경 `lockedCountCheck`는 `SecurityReview`로 표시한다. 기존 DB의 모든 그룹을 새 조건과 N으로 검사해 초과 그룹이 있으면 `Blocked` 처리한다. 제약 제거도 무결성 완화이므로 `SecurityReview`로 남긴다. 이 제약은 DDL에 없으므로 catalog 지문을 바꾸지 않으며 facts digest/journal 변경으로 배포 정본을 갱신한다.

같은 migration 변경에서 Email·Date 필드 추가와 백필의 물리 타입·값 검증도 확인한다. Text→Email처럼 물리 타입이 같아도 검증 규칙이 강해지는 변경은 저장된 값을 새 validator로 검사하고 부적합 데이터가 있으면 `Blocked`로 분류한다.

## 확인 계획

- N=2에서 같은 그룹에 두 쓰기가 동시에 한 행씩 더해질 때 하나만 커밋되는지 확인한다.
- 그룹 이동과 조건이 거짓에서 참으로 바뀌는 경우 destination group count를 검사하는지 확인한다.
- nullable `per`의 NULL 그룹이 제한에서 제외되는지 확인한다.
- V3 표준 쓰기, V4 write extension, V6 `/apply`, V6 write extension 모두에서 실제 DB 트랜잭션 격리 수준이 READ COMMITTED인지 확인한다.
- N=1 기존 facts·DDL이 바뀌지 않는지, 제품 `aip-migrate`가 초과 기존 그룹을 Blocked 처리하고 새·변경·제거를 SecurityReview 하는지 확인한다.
- Email·Date backfill과 Text→Email의 기존 값 검사를 실제 PostgreSQL migration으로 확인한다.

N=2 경쟁 쓰기와 그룹 이동, 숨겨진 qualifying 행, 조건 false→true, NULL 그룹, V3/V4/V6 표준 쓰기, V6 write extension, 제품 migration add/change/Blocked, N=1 facts·DDL 보존, Email·Date backfill 및 Text→Email Blocked 경로를 통합 테스트로 확인한다. 전체 spike suite와 직접 SQL 쓰기는 이 범위의 실행 보장에 포함하지 않는다.
