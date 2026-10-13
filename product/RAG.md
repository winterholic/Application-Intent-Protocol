# 이름 기반 RAG 실행

아래 설정과 TypeScript API는 현재 구현된 문법이다. 서버가 선언한 corpus와 제공자를 이용해 문서 분할, embedding, 영속 색인, 검색, 출처를 포함한 답변을 실행한다. 프론트는 모델 URL·credential·다른 owner·SQL 필터를 지정하지 않는다.

## 실제 서버 설정

기존 서비스 설정에 추가하는 필드다. endpoint는 **전체 요청 경로**다. 아래 모델 이름과 차원은 배포할 제공자의 계약에 맞춰 교체한다.

```json
{"rag":{
  "client_index":false,
  "embedding_providers":[{
    "name":"documents","endpoint":"https://models.example.com/v1/embeddings",
    "model":"deployment-embedding-model","api_key_env":"AIP_EMBEDDING_KEY",
    "timeout_ms":10000,"max_batch_items":16
  }],
  "answer_providers":[{
    "name":"assistant","endpoint":"https://models.example.com/v1/chat/completions",
    "model":"deployment-answer-model","api_key_env":"AIP_ANSWER_KEY","timeout_ms":15000
  }],
  "corpora":[{
    "name":"manuals","embedding_provider":"documents","answer_provider":"assistant",
    "dimensions":768,"chunk_chars":1024,"overlap_chars":128,
    "max_document_bytes":524288,"max_document_chunks":1000,"max_corpus_chunks":10000,"max_owner_chunks":2000,
    "max_top_k":10,"max_context_chars":16000,"max_prompt_tokens":24000,
    "max_answer_tokens":1024,"timeout_ms":30000
  }]
}}
```

OpenAI-compatible embedding/chat JSON API를 사용한다. HTTPS와 인증서 검증을 적용하고 proxy 환경 변수·redirect를 사용하지 않는다. loopback HTTP는 로컬 모델 서버용으로 허용한다. endpoint는 신뢰한 배포 설정이며 DNS를 고정하는 외부 HTTP broker의 정책과 다르다. 사설 모델 접근과 egress는 운영 네트워크 정책으로 통제한다. 제공자 이름 중복, 없는 제공자, 잘못된 차원·크기·기한은 시작 시 거부한다. answer_provider를 생략하면 검색만 제공한다.

## 실제 client API

제품 connect(...)가 반환하는 client에서 호출한다.

```ts
const hits = await client.rag.search("manuals", "반품 처리 절차", {topK:5});
const result = await client.rag.answer("manuals", "반품 승인은 누가 하나요?", {topK:5});
console.log(result.answer, result.citations);
```

HTTP 경로는 POST /rag/search, /rag/answer, /rag/index, /rag/delete다. Bearer와 x-aip-contract가 필요하다. 활성화와 client 색인 허용 여부는 인증된 capability catalog에서 탐색한다. 고정 입력 필드만 허용하고 응답의 계약 지문도 검사한다.

색인·삭제의 client 접근은 기본 false다. client_index:true를 설정하면 인증된 사용자가 자신의 corpus 문서를 수정할 수 있다. 이때 모든 corpus에 `max_owner_chunks`(principal 하나가 쓸 수 있는 chunk 수, `max_document_chunks` 이상 `max_corpus_chunks` 이하)를 지정해야 시작한다. 한 사용자가 corpus를 채워 다른 사용자의 색인을 막는 일을 막고 검색이 메모리에 올리는 chunk 수도 이 값으로 묶인다. `max_corpus_chunks`는 전체 상한으로 남으므로 `예상 사용자 수 × max_owner_chunks` 이하로 두어야 한 사용자의 몫이 보장된다. 업무 승인 정책이 필요하면 이 옵션을 열지 않고 신뢰한 서버에서 RagEngine::index_document와 delete_document를 호출한다. 허용한 배포에서만 다음 API를 쓴다.

```ts
await client.rag.index("manuals", "return-policy-v1", "사내 반품 정책", plainText);
await client.rag.delete("manuals", "return-policy-v1");
```

## 영속성과 실행 경계

기본 저장소는 PostgreSQL JSONB vector와 실행 엔진의 exact cosine 검색이다. pgvector 설치가 필요 없으며 owner·corpus 조건을 저장소 조회에 적용한다. 문서 재색인은 기존 chunk와 transaction으로 교체한다. corpus 용량 확인과 교체는 advisory transaction lock으로 직렬화한다. embedding profile이 달라진 문서는 재색인 전 검색에서 거부한다.

서비스는 앱 schema 이름에서 파생한 aip_infra_<digest> schema에 저장하여 앱 migration 구조 지문에 인프라 테이블을 섞지 않는다. 별도 schema의 백업·삭제와 전용 RAG pool의 연결 예산은 [인프라 운영 경계](INFRASTRUCTURE.md)를 따른다.

엔진 상한은 차원 4096, corpus 100,000 chunks와 총 20,000,000 vector 값, topK 100, chunk 2048 Unicode 문자, 문서 4MiB다. 실제 설정은 더 작게 둔다. HTTP 본문은 기존 1MiB, SDK 색인 text는 900KiB다. JSON escaping이 많은 내용은 HTTP 한도에 먼저 도달할 수 있다. embedding batch 최대 32개, 제공자 응답 8MiB, 답변 토큰 최대 4096이다. max_prompt_tokens는 tokenizer 측정치가 아니라 고정 template 여유 1024와 UTF-8 byte 수를 사용하는 보수적 예산이다.

검색은 owner 영역의 제한된 전체 chunk를 읽고 점수를 계산한다. 점수 계산은 64개마다 scheduler에 양보하고 기한을 검사하며 상위 topK만 유지해 전체 corpus 정렬을 피한다. ANN, hybrid/BM25, reranker, GraphRAG, 자동 데이터 동기화는 구현하지 않았다. 큰 corpus나 특수 모델에는 RagStore, EmbeddingProvider, AnswerProvider, RagAuthorizer를 서버에서 교체한다. JS/TS·Python 파서나 모델은 신뢰한 서버 bridge에 연결할 수 있지만 sandbox worker에 ctx.rag를 자동 주입하지는 않는다.

HTTP 실행은 worker 동시 quota와 제공자·전체 기한을 적용한다. 실행 전후 principal과 배포 상태를 검사해 처리 중 폐기된 신원에 결과를 반환하지 않는다. 색인·삭제가 커밋된 뒤 인증 폐기나 응답 유실이 일어날 수 있다. 외부 embedding 비용과 DB commit은 단일 transaction이 아니며 외부 요청의 exactly-once를 보장하지 않는다.

검색 문서는 신뢰하지 않는 JSON data로 모델에 전달하고 tool 실행 권한을 주지 않는다. 출처는 검색한 문서 참조이며 답변의 사실성·인용 정확성·prompt injection 완전 차단을 증명하지 않는다. 실제 사용자 데이터 평가와 제공자의 데이터 보존 정책은 배포에서 확인한다.

## 검증과 근거

rag_engine, rag_pg_store, rag_providers 검사는 owner 격리, 교체 원자성, 용량 경쟁, 잘못된 vector·profile·응답, 기한·입력 크기를 확인한다. v6 rag_transport는 실제 HTTP·PostgreSQL에서 권한 폐기와 인프라 활성화 뒤 migration preflight를 검사한다. 통제된 provider HTTP fixture와 SDK 검사는 실제 유료 모델 계정 호출을 대신하지 않는다.

공식 계약은 [embedding](https://platform.openai.com/docs/api-reference/embeddings/create), [chat completion](https://platform.openai.com/docs/api-reference/chat/create), [PostgreSQL advisory lock](https://www.postgresql.org/docs/current/explicit-locking.html#ADVISORY-LOCKS)을 참고했다. 타 프로젝트 구현을 복사하지 않았다.
