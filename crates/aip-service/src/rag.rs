use aip_execution::rag::{
    AnswerProvider, CorpusSpec, EmbeddingProvider, OpenAiCompatibleEmbedder, OpenAiCompatibleGenerator, OwnerAuthorizer, PgJsonVectorStore, RagEngine,
};
use serde::Deserialize;
use std::{collections::HashMap, sync::Arc, time::Duration};

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub client_index: bool,
    pub corpora: Vec<Corpus>,
    #[serde(default)]
    pub embedding_providers: Vec<EmbeddingProviderConfig>,
    #[serde(default)]
    pub answer_providers: Vec<AnswerProviderConfig>,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Corpus {
    pub name: String,
    pub embedding_provider: String,
    pub answer_provider: Option<String>,
    pub dimensions: usize,
    pub chunk_chars: usize,
    #[serde(default)]
    pub overlap_chars: usize,
    pub max_document_bytes: usize,
    pub max_document_chunks: usize,
    pub max_corpus_chunks: usize,
    pub max_top_k: usize,
    pub max_context_chars: usize,
    pub max_prompt_tokens: usize,
    pub max_answer_tokens: u32,
    pub timeout_ms: u64,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmbeddingProviderConfig {
    pub name: String,
    pub endpoint: String,
    pub model: String,
    pub api_key_env: String,
    #[serde(default = "default_provider_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default = "default_batch_items")]
    pub max_batch_items: usize,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnswerProviderConfig {
    pub name: String,
    pub endpoint: String,
    pub model: String,
    pub api_key_env: String,
    #[serde(default = "default_provider_timeout_ms")]
    pub timeout_ms: u64,
}

fn default_provider_timeout_ms() -> u64 {
    30_000
}

fn default_batch_items() -> usize {
    16
}

/// Reads the provider key named by `api_key_env`. The failure names the provider and the setting, never the variable
/// name or the key, because the name of a secret variable is not printed (see `secret_is_loaded_only_from_explicit_environment_reference`).
fn key_from_env(provider: &str, name: &str) -> Result<String, String> {
    if !super::valid_env_name(name) {
        return Err(format!("provider '{provider}'의 api_key_env는 대문자·숫자·'_'로 된 환경 변수 이름이어야 함"));
    }
    let value =
        std::env::var(name).map_err(|_| format!("provider '{provider}'의 api_key_env가 가리키는 환경 변수가 없음(변수 이름은 출력하지 않음)"))?;
    if value.is_empty() || value.len() > 8192 {
        return Err(format!("provider '{provider}'의 api_key_env 값이 비어 있거나 8192바이트를 넘음"));
    }
    Ok(value)
}

pub async fn build(
    config: &Config,
    database_url: &str,
    schema: &str,
    pool_options: spike_v6_transport::pool::PoolOptions,
) -> Result<(Arc<RagEngine>, bool), String> {
    let mut embeddings: HashMap<String, Arc<dyn EmbeddingProvider>> = HashMap::new();
    for provider in &config.embedding_providers {
        let key = key_from_env(&provider.name, &provider.api_key_env)?;
        let timeout = Duration::from_millis(provider.timeout_ms);
        let value = Arc::new(
            OpenAiCompatibleEmbedder::new(&provider.endpoint, provider.model.clone(), key, timeout, provider.max_batch_items).map_err(|_| {
                format!(
                    "embedding provider '{}'의 endpoint·model·timeout_ms·max_batch_items 설정 오류(endpoint는 자격 증명 없는 https)",
                    provider.name
                )
            })?,
        );
        if embeddings.insert(provider.name.clone(), value).is_some() {
            return Err(format!("embedding provider 이름 '{}'이 중복됨", provider.name));
        }
    }
    let mut answers: HashMap<String, Arc<dyn AnswerProvider>> = HashMap::new();
    for provider in &config.answer_providers {
        let key = key_from_env(&provider.name, &provider.api_key_env)?;
        let value = Arc::new(
            OpenAiCompatibleGenerator::new(&provider.endpoint, provider.model.clone(), key, Duration::from_millis(provider.timeout_ms), 1 << 20)
                .map_err(|_| format!("answer provider '{}'의 endpoint·model·timeout_ms 설정 오류(endpoint는 자격 증명 없는 https)", provider.name))?,
        );
        if answers.insert(provider.name.clone(), value).is_some() {
            return Err(format!("answer provider 이름 '{}'이 중복됨", provider.name));
        }
    }
    let corpora = config
        .corpora
        .iter()
        .map(|corpus| CorpusSpec {
            name: corpus.name.clone(),
            embedding_provider: corpus.embedding_provider.clone(),
            answer_provider: corpus.answer_provider.clone(),
            dimensions: corpus.dimensions,
            chunk_chars: corpus.chunk_chars,
            overlap_chars: corpus.overlap_chars,
            max_document_bytes: corpus.max_document_bytes,
            max_document_chunks: corpus.max_document_chunks,
            max_corpus_chunks: corpus.max_corpus_chunks,
            max_top_k: corpus.max_top_k,
            max_context_chars: corpus.max_context_chars,
            max_prompt_tokens: corpus.max_prompt_tokens,
            max_answer_tokens: corpus.max_answer_tokens,
            timeout_ms: corpus.timeout_ms,
        })
        .collect();
    let factory = Arc::new(
        spike_v6_transport::rag_transport::RagConnectionFactory::new(database_url.to_owned(), pool_options)
            .map_err(|_| "RAG 저장소 연결 풀 설정 오류(db_pool 한계 확인)".to_owned())?,
    );
    let storage_schema = aip_execution::infrastructure_schema(schema).map_err(|_| "배포 schema 이름이 RAG 저장소 schema로 쓸 수 없음".to_owned())?;
    let store = Arc::new(
        PgJsonVectorStore::with_factory(factory, storage_schema).map_err(|_| "RAG 저장소를 만들 수 없음(배포 schema 이름 확인)".to_owned())?,
    );
    store.initialize().await.map_err(|_| "RAG 저장소 초기화 실패(DB 연결, pgvector 사용 여부, schema 생성 권한 확인)".to_owned())?;
    let engine = RagEngine::new(corpora, embeddings, answers, store, Arc::new(OwnerAuthorizer))
        .map_err(|_| "corpus 설정 오류(이름 중복, 존재하지 않는 provider 참조, dimensions·chunk·한계 값 범위 확인)".to_owned())?;
    Ok((Arc::new(engine), config.client_index))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn indexing_is_opt_in_and_unknown_configuration_is_rejected() {
        let config: Config = serde_json::from_value(json!({"corpora":[]})).unwrap();
        assert!(!config.client_index);
        assert!(config.embedding_providers.is_empty());
        assert!(serde_json::from_value::<Config>(json!({"corpora":[],"allow_all":true})).is_err());
    }

    #[test]
    fn credential_references_use_the_service_environment_name_contract() {
        for invalid in ["", "1_API_KEY", "aip_key", "AIP-KEY", "AIP KEY"] {
            let failure = key_from_env("p", invalid).unwrap_err();
            assert!(failure.contains("환경 변수 이름이어야 함"), "{failure}");
        }
        let missing = key_from_env("embed", "AIP_TEST_RAG_SECRET_NAME_8842").unwrap_err();
        assert!(missing.contains("'embed'") && missing.contains("환경 변수가 없음"), "{missing}");
        assert!(!missing.contains("AIP_TEST_RAG_SECRET_NAME_8842"), "the variable name must not be printed: {missing}");
        assert!(super::super::valid_env_name("_AIP_KEY_2"));
    }
}
