# 실행 검증과 미검증 경계

2026-10-09. 제품 코드 기준 `ecf3369`. macOS·로컬 PG·Node·Python 환경의 실행 결과다. 라이브 PSP/메일/cloud storage/broker 서비스나 다른 OS에 대한 검증이 아니다. 검토자의 소스 판독과 부모가 실행한 실험을 구분한다.

## T01 — 업무 SQL 없는 계산의 실제 경계

```sh
export PATH="$HOME/.cargo/bin:$PATH"
python3 plan-docs/backend-capability/probes/run-sql-free-boundary.py
```

[원시 출력](evidence/sql-free-boundary.txt), [Rust probe](probes/sql-free-boundary.rs), [Node](probes/compute.mjs), [Python](probes/compute.py).

| 언어 | 실험 | 실제 결과 |
|---|---|---|
| Node | 빈 access, Unicode 텍스트 길이 계산 | OK, length=3 |
| Python | 같은 입력/계약 | OK, length=3 |
| Node / Python | 같은 순수 계산에 연결 불가 DB 설정 | 양쪽 DB_UNAVAILABLE |
| Node | 9초 선언, 6초 계산 | OK, 6098ms |
| Python | 9초 선언, 6초 계산 | OK, 6083ms |
| Node / Python | 1초 선언, 6초 계산 | 양쪽 DEADLINE_EXCEEDED |

8개 언어×상황 assertion과 빈 access/입출력 검사를 실행했다. namespace용 resource가 CREATE TABLE DDL을 생성하는지도 검사했지만 **그 DDL은 적용하지 않았다**. 이 probe는 PG에 clock 조회만 하며 공유 DB를 중단하지 않는다. port 1 연결 실패로 DB 불가 경로를 재현한다. 상용 장애 주입 결과는 아니다.

```text
Node | sync-over-five-seconds | OK | 6098ms
Python | declaration-deadline | DEADLINE_EXCEEDED | 1015ms
resource namespace produces table DDL: true (DDL not executed)
```

실제 V1 loader→V5 fingerprint→V6 listener→MacNetDeny Node/Python worker를 사용했다. test Keyring의 서명 세션을 사용하며 **운영 IdP/JWKS·principal DB 매핑·제품 serve preflight/fence 전체를 새로 검증한 것은 아니다**. 해당 제품 경계는 P07/P08 코드 및 아래 service smoke가 별도 근거다. 인증 우회 경로나 제품 코드를 추가하지 않았다.

관찰: SQL을 안 쓰는 계산 자체는 가능하다. PG 없는 제품 서비스는 별개이며 현재 불가하다. REQUEST_TIMEOUT=5초를 전체 worker 실행 상한으로 해석하면 틀린다. 시간 값은 이 반례의 관찰일 뿐 성능 벤치마크가 아니다.

첫 scratch 시도는 actor 선언을 빼 `MISSING_ITEM: actor 선언 필요`로 거부됐다. fixture에 명시 actor를 추가한 뒤 실행했으며, 이 거부도 현재 계약의 제약이다. 제품의 필수 actor 규칙을 완화하지 않았다.

## 제품 문법·DB 업무 probe 재실행

```sh
cargo test --offline --manifest-path spikes/spike-v3-write/Cargo.toml \
  --test oss_business_domains --test oss_product_variants \
  --test oss_helpdesk_visibility --test cardinality_capacity
```

[원시 출력](evidence/product-probes.txt).

| binary | 결과 | 검증 범위 |
|---|---|---|
| cardinality_capacity | 1 passed / 0 failed | 동시 활성화·그룹 정원·숨긴 행/nullable grouping |
| oss_business_domains | 1 / 0 | 티켓 capacity·전자서명 unique·예약 check/rollback·재고 floor |
| oss_helpdesk_visibility | 1 / 0 | 공개·비공개 답변·문자열 보존·반복 효과 방지 |
| oss_product_variants | 1 / 0 | 순환 FK·가시성·unique·활성화와 부모 갱신/outbox rollback |

총 4개 테스트. 제품 V1/V2/V3 실행 경로의 probe이며 전체 제품 HTTP 업무 E2E와 구분한다. variant/예약 알림 테스트의 outbox 행 존재는 실제 알림 발송을 뜻하지 않는다.

## 별도 root PoC 시나리오 재실행

```sh
cargo test --offline -p aip-cli \
  --test shop_e2e --test ariari_e2e --test saas_e2e \
  --test outbound_e2e --test subscribe_e2e --test cms_e2e
```

[원시 출력](evidence/poc-scenarios.txt).

| binary | 결과 | 감사에 쓰는 근거 |
|---|---|---|
| shop_e2e | 1 passed / 0 failed | 재고 경합/멱등/권한/version/웹훅 서명·중복 |
| ariari_e2e | 4 / 0 | 승인/quorum·기한, 파일·CSV Job/status·가시성·결과 삭제 예약(실제 expiry 미실행) |
| saas_e2e | 2 / 0 | tenant 격리와 해당 층을 제거한 negative control |
| outbound_e2e | 5 / 0 | 서명/retry/실패 endpoint·주소·tenant·negative control |
| subscribe_e2e | 5 / 0 | WS 인증/권한·가시성·영향 table wakeup·상한 |
| cms_e2e | 8 / 0 | 공개본/삭제/동의/대리 세션·HTTP·negative control |

총 25개 테스트. 이 결과는 PoC 재사용 후보의 동작 근거다. **제품에서 Job/파일/webhook/WS를 지원한다는 증거가 아니다.** 실제 외부 provider 대신 로컬 테스트 server/모사 효과를 쓰는 경계를 유지한다.

## 기존 작은 수정 배치의 검증

방향 전환 전에 진행하던 Ref 대상 identity·nullable 생성 후보 수정은 `ecf3369`에 따로 커밋했다.

```sh
bash tools/verify.sh
node --test product/tests/service-smoke.test.mjs
```

이번 작업 중 실행한 출력 발췌:

```text
AIP core verification passed. Worker and relocated-service checks are separate.
ℹ pass 1
ℹ fail 0
```

전체 core 명령과 실제 설치 SDK service smoke의 범위를 구분한다. smoke는 두 Id wire와 Node/Python 확장·읽기/쓰기 경로를 포함하지만 새 durable Job/provider의 검증은 아니다. 전체 로그는 작업 환경의 `/tmp/aip-identity-nullable-verification.log`, `/tmp/aip-identity-nullable-service-smoke.log`에 있고, 위 인용은 종료 결과 발췌다.

## 산출물·재현 소스 검증

- 원문 0–12절, A–J 105개 최소 bullet, A–F 여섯 시나리오의 존재를 대조했다. matrix CSV의 기능 행 순서/값을 원문 bullet과 정확히 비교한다. 이는 기능 지원율이 아니다.
- 여섯 시나리오의 1–8항목, A–F 결과물 링크, local link·source ID·라이선스 pinned hash·probe 출력·실제 문법 발췌를 점검한다.
- probe의 Python 문법·Node 문법·Rust formatting을 검사한다. 가상 JSON/YAML이 실제 AIP 문법으로 컴파일된다고 주장하지 않는다.
- 독립 reviewer는 문서·코드만 읽었고 실행은 하지 않았다. 검토 정정은 [cross-review](cross-review.md)에 있다.

## 남은 미검증

새 durable 실행·cancel/checkpoint/crash 복구, 실제 PSP/OAuth/mail/storage/Kafka, Linux 격리, 대용량 streaming, 새 realtime resume/gap, 모든 제품 tenant 실행 경로는 미구현 또는 미실행이다. 여섯 시나리오는 **제품 부분 실행 + PoC 비교 + 미구현 계약 평가**이며 전체 업무 완성 주장이 아니다.
