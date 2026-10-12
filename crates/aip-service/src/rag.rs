use aip_execution::rag::{
    AnswerProvider, CorpusSpec, EmbeddingProvider, OpenAiCompatibleEmbedder, OpenAiCompatibleGenerator, OwnerAuthorizer, PgJsonVectorStore,
    RagEngine, RagError,
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

fn key_from_env(name: &str) -> Result<String, RagError> {
    if !super::valid_env_name(name) {
        return Err(RagError::InvalidInput);
    }
    let value = std::env::var(name).map_err(|_| RagError::InvalidInput)?;
    if value.is_empty() || value.len() > 8192 {
        return Err(RagError::InvalidInput);
    }
    Ok(value)
}

pub async fn build(
    config: &Config,
    database_url: &str,
    schema: &str,
    pool_options: spike_v6_transport::pool::PoolOptions,
) -> Result<(Arc<RagEngine>, bool), RagError> {
    let mut embeddings: HashMap<String, Arc<dyn EmbeddingProvider>> = HashMap::new();
    for provider in &config.embedding_providers {
        let key = key_from_env(&provider.api_key_env)?;
        let timeout = Duration::from_millis(provider.timeout_ms);
        let value = Arc::new(OpenAiCompatibleEmbedder::new(&provider.endpoint, provider.model.clone(), key, timeout, provider.max_batch_items)?);
        if embeddings.insert(provider.name.clone(), value).is_some() {
            return Err(RagError::InvalidInput);
        }
    }
    let mut answers: HashMap<String, Arc<dyn AnswerProvider>> = HashMap::new();
    for provider in &config.answer_providers {
        let key = key_from_env(&provider.api_key_env)?;
        let value = Arc::new(OpenAiCompatibleGenerator::new(
            &provider.endpoint,
            provider.model.clone(),
            key,
            Duration::from_millis(provider.timeout_ms),
            1 << 20,
        )?);
        if answers.insert(provider.name.clone(), value).is_some() {
            return Err(RagError::InvalidInput);
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
        spike_v6_transport::rag_transport::RagConnectionFactory::new(database_url.to_owned(), pool_options).map_err(|_| RagError::InvalidInput)?,
    );
    let storage_schema = aip_execution::infrastructure_schema(schema).map_err(|_| RagError::InvalidInput)?;
    let store = Arc::new(PgJsonVectorStore::with_factory(factory, storage_schema)?);
    store.initialize().await?;
    let engine = RagEngine::new(corpora, embeddings, answers, store, Arc::new(OwnerAuthorizer)).map_err(|_| RagError::InvalidInput)?;
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
            assert_eq!(key_from_env(invalid), Err(RagError::InvalidInput));
        }
        assert!(super::super::valid_env_name("_AIP_KEY_2"));
    }
}
