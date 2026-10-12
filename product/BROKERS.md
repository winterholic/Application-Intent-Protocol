# Kafka와 RabbitMQ 연결

기본 메시지 저장소는 PostgreSQL이다. 선택적 external-brokers feature는 서버가 허용한 route에서 실제 Kafka와 AMQP 0-9-1을 송수신한다. 프론트나 worker 입력으로 broker 주소·topic·exchange·다른 owner를 지정하지 않는다.

```sh
cargo build -p aip-cli --features external-brokers --locked
```

Kafka librdkafka 소스 빌드에는 C/C++ compiler, CMake, pkg-config와 OpenSSL 개발 파일이 필요하다. 기본 빌드에 broker 설정을 넣으면 feature 활성화를 요구하며 시작을 거부한다.

## 실제 서버 설정

기존 서비스 설정의 broker 필드다. queue는 AIP 정의에 access messages.<queue>로 선언되어야 한다. owner는 서버가 확인한 **실제 메시지 owner key**로 교체한다. 예시 문자열은 자동 tenant 선택 문법이 아니다.

```json
{"broker":{
  "providers":[{
    "kind":"kafka","name":"events","bootstrap_servers":["kafka.example.com:9093"],
    "security":{"kind":"sasl_tls","mechanism":"SCRAM-SHA-512","username":"aip","password_env":"AIP_KAFKA_PASSWORD"},
    "timeout_ms":5000
  },{
    "kind":"rabbit_mq","name":"tasks","uri_env":"AIP_RABBITMQ_URI","timeout_ms":5000
  }],
  "outbound":[{
    "owner":"principal-owner-key","queue":"notifications",
    "destination":{"kind":"kafka","provider":"events","topic":"app.notifications"}
  }],
  "inbound":[{
    "provider":"tasks","owner":"principal-owner-key","queue":"partner_events",
    "source":{"kind":"rabbit_mq","queue_name":"partner.incoming","dead_letter_exchange":"partner.dead","dead_letter_routing_key":"invalid"},
    "max_deliveries":3,"ttl_ms":86400000
  }],
  "lease_ms":15000,"poll_ms":250
}}
```

Kafka inbound source는 {"kind":"kafka","topic":"partner.events","group_id":"aip-import","dead_letter_topic":"partner.events.dead"}다. DLQ와 입력 topic은 달라야 한다. RabbitMQ outbound destination은 {"kind":"rabbit_mq","provider":"tasks","exchange":"app.events","routing_key":"notifications"}다. 기본 exchange는 빈 문자열이다. topic·exchange·queue와 DLQ routing은 운영자가 먼저 생성한다. importer는 topology를 자동 생성하지 않는다.

TLS가 기본이다. Kafka tls 또는 sasl_tls에 필요하면 ca_location을 설정한다. RabbitMQ URI 환경 변수는 amqps://...를 사용한다. 사설 주소는 provider의 allow_private_networks:true로 명시한다. 로컬 fixture 평문은 allow_insecure_transport:true까지 추가로 요구한다. credential 값은 설정·catalog·Debug 출력에 넣지 않는다.

연결 전 설정한 주소의 DNS 결과를 검사하지만 SDK가 연결할 때의 DNS를 고정하지 않는다. Kafka metadata의 advertised broker도 SDK가 사용한다. 신뢰한 서버 설정용이며 외부 HTTP broker의 DNS 고정 SSRF 방어와 같은 보장이 아니다. egress·TLS·broker ACL로 접근 범위를 제한한다.

## 전달과 장애 의미

outbound는 PostgreSQL lease를 획득하고 Kafka acks=all/idempotent producer 또는 RabbitMQ publisher confirm 뒤 같은 lease를 ACK한다. 원격 발행과 DB ACK 사이 crash는 같은 message.id 재발행으로 이어질 수 있다. Kafka record key와 AMQP message-id, envelope id가 중복 제거 키다. consumer가 키와 업무 부작용을 함께 멱등 처리해야 한다.

inbound는 JSON을 서버 route의 owner·queue에 PostgreSQL transaction으로 저장한 후 offset 또는 delivery를 ACK한다. payload의 owner/queue는 실행 scope를 바꾸지 않는다. Kafka 중복 키는 provider/topic/partition/offset의 digest이므로 같은 application key인 별개 record를 합치지 않는다. RabbitMQ는 안정적 message-id를 요구한다. 없거나 잘못된 JSON·초과 크기·같은 ID의 다른 payload는 DLQ 발행 확인 뒤 원본을 ACK한다. DLQ에는 오류 JSON envelope, 원본 크기·SHA256·최대 4KiB의 hex preview와 truncated 표시를 저장한다. 초대형 원문 전체를 보관하지 않으므로 원문 보존이 필요한 배포는 source retention이나 별도 quarantine 저장소를 사용한다. 기본 exchange의 동일 입력 queue 재전달은 설정에서 거부하고 다른 exchange binding의 loop는 운영자가 막아야 한다. DLQ 확인이 불명확하면 원본을 ACK하지 않는다.

분산 transaction은 없다. offset/ACK 불확실성은 재전달로 이어질 수 있으며 메시지 TTL 뒤 중복 키 기록이 정리되므로 영구 중복 방지를 보장하지 않는다. lease_ms는 broker timeout과 batch에 맞춰 여유 있게 둔다. 서버는 tick당 route별 최대 20개를 순차 처리하고, 비어 있는 inbound route는 최대 1ms만 기다린다. 발행 실패 뒤에는 해당 batch를 즉시 중단한다. 여러 route의 지연과 Kafka poll interval을 함께 고려한다.

일시적 outbound 발행 실패는 현재 lease를 CAS로 해제하면서 해당 claim의 delivery 예산을 환불한다. 원격 반영이 불명확해도 ACK하지 않고 동일 ID로 재발행할 수 있다. broker·importer·DB 연결 장애는 현재 소비자와 DB 연결을 폐기하고 최대 30초의 backoff로 재연결한다. PostgreSQL에 커밋한 입력은 재수신 시 중복 키로 처리하고, 정상 HTTP 요청은 유지한다. 재연결 실패와 이유는 credential 없는 진단 로그로 남긴다. 최초 설정·연결 실패는 시작을 거부하고, 예기치 않은 worker task 종료는 서버 오류로 처리한다. topology 자동 생성은 제공하지 않는다. 종료 대기 상한은 5초이며 이후 task를 취소한다. 시작·재연결의 broker 설정 전체 기한은 10초, DB 연결 기한은 5초다.

provider 최대 32개, inbound/outbound 각각 256 route, batch 최대 100개, payload 512KiB, outbound envelope 1MiB, timeout 최대 30초다. broker의 fetch/message 크기·retention·quota·DLQ 보존도 제한한다. route는 고정 owner/queue 단위이며 모든 tenant를 자동 순회하지 않는다. 같은 owner/queue를 client consumer와 relay가 동시에 claim하면 경쟁하므로 소비 경로를 하나로 운영한다.

## 검증과 근거

[External brokers workflow](../.github/workflows/brokers.yml)는 임시 PostgreSQL·Kafka·RabbitMQ에서 실제 publish/consume/confirm/lease·DLQ, 초대형 불량 입력, 같은 key의 다른 Kafka offset, RabbitMQ ID 충돌, worker DB 연결 종료 후 복구를 검사한다.

```sh
cargo test -p aip-execution --features external-brokers --test brokers --locked
# 전용 fixture 환경 변수와 실제 broker가 필요하며 누락하면 실패한다.
cargo test -p aip-execution --features external-brokers --test brokers_live --locked -- --ignored
```

평문 fixture는 운영 SASL·TLS 인증, cluster failover, 장시간 부하를 검증하지 않는다. 공식 의미는 [Kafka delivery semantics](https://kafka.apache.org/documentation/#semantics), [RabbitMQ confirm과 ACK](https://www.rabbitmq.com/docs/confirms), API는 [rust-rdkafka](https://github.com/fede1024/rust-rdkafka), [lapin](https://github.com/amqp-rs/lapin)을 따른다. 타 프로젝트 소스 복사 없이 해당 client library를 선택적으로 사용한다.
