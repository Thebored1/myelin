//! Embeddings and source-preserving chunking for document RAG.

use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::time::Instant;

pub const CHUNKER_VERSION: &str = "tokens-256-overlap-40-max-384-structural-v3";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkerConfig {
    pub target_tokens: usize,
    pub overlap_tokens: usize,
    pub hard_max_tokens: usize,
    pub min_tail_tokens: usize,
}

pub const RAG_CHUNKER: ChunkerConfig = ChunkerConfig {
    target_tokens: 256,
    overlap_tokens: 40,
    hard_max_tokens: 384,
    min_tail_tokens: 64,
};

pub const EMBEDDING_CONTRACT_VERSION: u32 = 1;
pub const HASHED_EMBEDDING_DIMENSIONS: usize = 384;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EmbeddingPooling {
    Mean,
    Cls,
    Last,
}

impl EmbeddingPooling {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "mean" => Ok(Self::Mean),
            "cls" => Ok(Self::Cls),
            "last" => Ok(Self::Last),
            other => Err(format!(
                "unsupported embedding pooling '{other}' (expected mean, cls, or last)"
            )),
        }
    }
    pub fn as_server_arg(self) -> &'static str {
        match self {
            Self::Mean => "mean",
            Self::Cls => "cls",
            Self::Last => "last",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EmbeddingNormalization {
    L2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbeddingInputKind {
    Query,
    Document,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddingModelContract {
    pub contract_version: u32,
    pub fingerprint: String,
    pub model_id: String,
    pub display_name: String,
    pub model_path: PathBuf,
    pub dimensions: usize,
    pub server_context_tokens: usize,
    pub max_input_tokens: usize,
    pub pooling: Option<EmbeddingPooling>,
    pub query_prefix: String,
    pub document_prefix: String,
    pub normalization: EmbeddingNormalization,
    pub profiled: bool,
}

impl EmbeddingModelContract {
    pub fn prefix(&self, kind: EmbeddingInputKind) -> &str {
        match kind {
            EmbeddingInputKind::Query => &self.query_prefix,
            EmbeddingInputKind::Document => &self.document_prefix,
        }
    }
    pub fn prepare_input(&self, kind: EmbeddingInputKind, text: &str) -> String {
        format!("{}{}", self.prefix(kind), text)
    }
}

#[derive(Debug, Clone)]
pub struct EmbeddingRuntime {
    pub base_url: String,
    pub contract: EmbeddingModelContract,
}

#[derive(Debug, Clone, Default)]
pub struct EmbeddingBatchMetrics {
    pub request_count: usize,
    pub input_count: usize,
    pub token_count: usize,
    pub elapsed_ms: u128,
}

#[derive(Debug, Clone)]
pub struct EmbeddingBatchResult {
    pub vectors: Vec<Vec<f32>>,
    pub metrics: EmbeddingBatchMetrics,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DocumentFormat {
    Markdown,
    Latex,
    PdfText,
    Notebook,
    HtmlText,
    EpubText,
    PlainText,
}

impl DocumentFormat {
    pub fn from_path(path: &str) -> Self {
        let path = path.to_ascii_lowercase();
        if path.ends_with(".md") || path.ends_with(".markdown") {
            Self::Markdown
        } else if path.ends_with(".tex") {
            Self::Latex
        } else if path.ends_with(".pdf") {
            Self::PdfText
        } else if path.ends_with(".ipynb") {
            Self::Notebook
        } else if path.ends_with(".html") || path.ends_with(".htm") {
            Self::HtmlText
        } else if path.ends_with(".epub") {
            Self::EpubText
        } else {
            Self::PlainText
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenizerMode {
    Exact,
    ConservativeFallback,
}

impl TokenizerMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Exact => "exact",
            Self::ConservativeFallback => "conservative_fallback",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub text: String,
    pub index: usize,
    pub token_count: usize,
    pub char_start: Option<u64>,
    pub char_end: Option<u64>,
    pub page_start: Option<i32>,
    pub page_end: Option<i32>,
    pub section_start: Option<String>,
    pub section_end: Option<String>,
    pub tokenizer_mode: TokenizerMode,
}

#[derive(Debug, Clone)]
pub struct ChunkingResult {
    pub chunks: Vec<Chunk>,
    pub mode: TokenizerMode,
    pub tokenizer_requests: usize,
    pub warning: Option<String>,
}

pub trait TokenCounter {
    fn count<'a>(
        &'a self,
        text: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<usize, String>> + Send + 'a>>;
}

pub struct ServerTokenCounter<'a> {
    client: &'a Client,
    endpoint: String,
}

impl<'a> ServerTokenCounter<'a> {
    pub fn new(client: &'a Client, base_url: &str) -> Self {
        Self {
            client,
            endpoint: format!("{}/tokenize", base_url.trim_end_matches('/')),
        }
    }
}

impl TokenCounter for ServerTokenCounter<'_> {
    fn count<'a>(
        &'a self,
        text: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<usize, String>> + Send + 'a>> {
        Box::pin(async move {
            let response = self
                .client
                .post(&self.endpoint)
                .json(&json!({ "content": text, "add_special": false }))
                .send()
                .await
                .map_err(|error| format!("tokenize request failed: {error}"))?;
            let status = response.status();
            let raw = response
                .text()
                .await
                .map_err(|error| format!("tokenize response read failed: {error}"))?;
            if !status.is_success() {
                return Err(format!(
                    "tokenize server returned HTTP {status}: {}",
                    raw.chars().take(500).collect::<String>()
                ));
            }
            let value: serde_json::Value = serde_json::from_str(&raw)
                .map_err(|error| format!("tokenize JSON parse failed: {error}"))?;
            value
                .get("tokens")
                .and_then(|tokens| tokens.as_array())
                .map(Vec::len)
                .ok_or_else(|| "tokenize response missing tokens array".to_string())
        })
    }
}

/// Verify the two endpoints required by the embedding contract and discover
/// the server's output width. The embedding endpoint, not GGUF hidden width,
/// is authoritative.
pub async fn probe_embedding_server(
    client: &Client,
    base_url: &str,
    model_id: &str,
    expected_dimensions: Option<usize>,
) -> Result<usize, String> {
    let counter = ServerTokenCounter::new(client, base_url);
    if counter.count("myelin embedding contract probe").await? == 0 {
        return Err("tokenize endpoint returned zero tokens for a non-empty probe".into());
    }
    let endpoint = format!("{}/v1/embeddings", base_url.trim_end_matches('/'));
    let response = client
        .post(endpoint)
        .json(
            &json!({ "model": model_id, "input": ["myelin query probe", "myelin document probe"] }),
        )
        .send()
        .await
        .map_err(|error| format!("embedding contract probe failed: {error}"))?;
    let status = response.status();
    let raw = response
        .text()
        .await
        .map_err(|error| format!("embedding contract probe response read failed: {error}"))?;
    if !status.is_success() {
        return Err(format!(
            "embedding contract probe returned HTTP {status}: {}",
            raw.chars().take(500).collect::<String>()
        ));
    }
    let value: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|error| format!("embedding contract probe JSON parse failed: {error}"))?;
    let data = value
        .get("data")
        .and_then(|value| value.as_array())
        .ok_or_else(|| "embedding contract probe response missing data".to_string())?;
    let dimensions = data
        .first()
        .and_then(|item| item.get("embedding"))
        .and_then(|value| value.as_array())
        .map(Vec::len)
        .filter(|dimension| *dimension > 0)
        .ok_or_else(|| "embedding contract probe returned no vector dimensions".to_string())?;
    if let Some(expected) = expected_dimensions.filter(|expected| *expected != dimensions) {
        return Err(format!(
            "embedding profile expects {expected} dimensions, but the server returned {dimensions}"
        ));
    }
    parse_embed_response(&raw, 2, dimensions)?;
    Ok(dimensions)
}

pub async fn server_model_id(client: &Client, base_url: &str, fallback: &str) -> String {
    let endpoint = format!("{}/v1/models", base_url.trim_end_matches('/'));
    let Ok(response) = client.get(endpoint).send().await else {
        return fallback.to_string();
    };
    let Ok(value) = response.json::<serde_json::Value>().await else {
        return fallback.to_string();
    };
    value
        .get("data")
        .and_then(|data| data.as_array())
        .and_then(|data| data.first())
        .and_then(|item| item.get("id"))
        .and_then(|id| id.as_str())
        .filter(|id| !id.is_empty())
        .unwrap_or(fallback)
        .to_string()
}

pub struct ByteTokenCounter;

impl TokenCounter for ByteTokenCounter {
    fn count<'a>(
        &'a self,
        text: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<usize, String>> + Send + 'a>> {
        Box::pin(async move { Ok(text.len()) })
    }
}

#[derive(Clone, Default)]
struct Location {
    page: Option<i32>,
    section: Option<String>,
}

/// Chunk a RAG document with the exact embedding tokenizer when possible. If
/// tokenization fails, the whole document is rebuilt with byte-counting so no
/// individual chunk is accidentally treated as exact.
pub async fn chunk_document<C: TokenCounter>(
    text: &str,
    format: DocumentFormat,
    counter: &C,
    config: ChunkerConfig,
) -> ChunkingResult {
    match chunk_with_counter(text, format, counter, config, TokenizerMode::Exact).await {
        Ok((chunks, requests)) => ChunkingResult {
            chunks,
            mode: TokenizerMode::Exact,
            tokenizer_requests: requests,
            warning: None,
        },
        Err(error) => {
            let (chunks, requests) = chunk_with_counter(
                text,
                format,
                &ByteTokenCounter,
                config,
                TokenizerMode::ConservativeFallback,
            )
            .await
            .expect("byte token counter cannot fail");
            ChunkingResult {
                chunks,
                mode: TokenizerMode::ConservativeFallback,
                tokenizer_requests: requests,
                warning: Some(format!(
                    "exact embedding tokenization failed; used conservative byte fallback: {error}"
                )),
            }
        }
    }
}

/// Synchronous splitter exclusively for viewer-section cache preparation. It
/// preserves source text but intentionally does not claim embedding-token
/// accuracy; RAG ingestion uses `chunk_document` above.
#[cfg(test)]
pub fn split_viewer_text(text: &str, target_chars: usize, overlap_chars: usize) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    if chars.iter().all(|c| c.is_whitespace()) {
        return Vec::new();
    }
    let target = target_chars.max(256);
    let overlap = overlap_chars.min(target / 2);
    let mut out = Vec::new();
    let mut start = 0usize;
    while start < chars.len() {
        let end = (start + target).min(chars.len());
        out.push(chars[start..end].iter().collect());
        if end == chars.len() {
            break;
        }
        start = end.saturating_sub(overlap);
    }
    out
}

async fn chunk_with_counter<C: TokenCounter>(
    text: &str,
    format: DocumentFormat,
    counter: &C,
    config: ChunkerConfig,
    mode: TokenizerMode,
) -> Result<(Vec<Chunk>, usize), String> {
    let chars: Vec<char> = text.chars().collect();
    if chars.iter().all(|c| c.is_whitespace()) {
        return Ok((Vec::new(), 0));
    }
    let locations = scan_locations(text, format, chars.len());
    let preferred = preferred_boundaries(&chars);
    let mut requests = 0usize;
    let mut spans: Vec<(usize, usize, usize)> = Vec::new();
    let mut start = 0usize;
    while start < chars.len() {
        let (target_end, target_count) =
            largest_end(&chars, start, config.target_tokens, counter, &mut requests).await?;
        let (max_end, _) = largest_end(
            &chars,
            start,
            config.hard_max_tokens,
            counter,
            &mut requests,
        )
        .await?;
        let mut end = target_end.max((start + 1).min(chars.len()));
        if let Some(boundary) = preferred
            .iter()
            .copied()
            .filter(|point| *point > start && *point <= target_end)
            .last()
        {
            let candidate_count =
                count_chars(&chars, start, boundary, counter, &mut requests).await?;
            if candidate_count >= config.target_tokens / 2 {
                end = boundary;
            }
        }
        if end > max_end {
            end = max_end.max((start + 1).min(chars.len()));
        }
        let count = if end == target_end {
            target_count
        } else {
            count_chars(&chars, start, end, counter, &mut requests).await?
        };
        spans.push((start, end, count));
        if end == chars.len() {
            break;
        }
        let overlap_start = overlap_start(
            &chars,
            start,
            end,
            config.overlap_tokens,
            counter,
            &mut requests,
        )
        .await?;
        start = overlap_start.max(start + 1).min(end);
    }
    if spans.len() >= 2 {
        let last = spans.len() - 1;
        if spans[last].2 < config.min_tail_tokens {
            let (previous_start, _, _) = spans[last - 1];
            let merged =
                count_chars(&chars, previous_start, chars.len(), counter, &mut requests).await?;
            if merged <= config.hard_max_tokens {
                spans[last - 1] = (previous_start, chars.len(), merged);
                spans.pop();
            }
        }
    }
    let chunks = spans
        .into_iter()
        .enumerate()
        .map(|(index, (start, end, tokens))| {
            let first = locations.get(start).cloned().unwrap_or_default();
            let last = locations
                .get(end.saturating_sub(1))
                .cloned()
                .unwrap_or_default();
            Chunk {
                text: chars[start..end].iter().collect(),
                index,
                token_count: tokens,
                char_start: Some(start as u64),
                char_end: Some(end as u64),
                page_start: first.page,
                page_end: last.page,
                section_start: first.section,
                section_end: last.section,
                tokenizer_mode: mode,
            }
        })
        .collect();
    Ok((chunks, requests))
}

async fn largest_end<C: TokenCounter>(
    chars: &[char],
    start: usize,
    limit: usize,
    counter: &C,
    requests: &mut usize,
) -> Result<(usize, usize), String> {
    let mut low = start;
    let mut high = chars.len();
    let mut best = start;
    let mut best_count = 0usize;
    while low < high {
        let mid = (low + high + 1) / 2;
        let count = count_chars(chars, start, mid, counter, requests).await?;
        if count <= limit {
            best = mid;
            best_count = count;
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    Ok((best, best_count))
}

async fn count_chars<C: TokenCounter>(
    chars: &[char],
    start: usize,
    end: usize,
    counter: &C,
    requests: &mut usize,
) -> Result<usize, String> {
    *requests += 1;
    counter
        .count(&chars[start..end].iter().collect::<String>())
        .await
}

async fn overlap_start<C: TokenCounter>(
    chars: &[char],
    start: usize,
    end: usize,
    overlap: usize,
    counter: &C,
    requests: &mut usize,
) -> Result<usize, String> {
    let mut low = start;
    let mut high = end;
    let mut best = end;
    while low < high {
        let mid = (low + high) / 2;
        let count = count_chars(chars, mid, end, counter, requests).await?;
        if count <= overlap {
            best = mid;
            high = mid;
        } else {
            low = mid + 1;
        }
    }
    Ok(best)
}

fn preferred_boundaries(chars: &[char]) -> Vec<usize> {
    let mut points = vec![0];
    for (index, character) in chars.iter().enumerate() {
        let next = chars.get(index + 1).copied();
        if character.is_whitespace()
            || matches!(character, '.' | '!' | '?' | '。' | '！' | '？')
                && next.is_none_or(char::is_whitespace)
        {
            points.push(index + 1);
        }
    }
    points
}

fn scan_locations(text: &str, format: DocumentFormat, chars_len: usize) -> Vec<Location> {
    let mut locations = vec![Location::default(); chars_len];
    let mut char_offset = 0usize;
    let mut page = None;
    let mut section = None;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim();
        if let Some(value) = trimmed
            .strip_prefix("[Page ")
            .and_then(|rest| rest.strip_suffix(']'))
            .and_then(|v| v.parse::<i32>().ok())
        {
            page = Some(value);
        }
        let detected =
            match format {
                DocumentFormat::Latex => ["section", "subsection", "subsubsection"]
                    .iter()
                    .find_map(|command| {
                        trimmed
                            .strip_prefix(&format!("\\{command}{{"))
                            .and_then(|rest| {
                                rest.find('}')
                                    .map(|end| format!("{command}: {}", &rest[..end]))
                            })
                    }),
                DocumentFormat::Markdown
                | DocumentFormat::PdfText
                | DocumentFormat::PlainText
                | DocumentFormat::HtmlText
                | DocumentFormat::EpubText => {
                    let hashes = trimmed.chars().take_while(|c| *c == '#').count();
                    ((1..=6).contains(&hashes))
                        .then(|| trimmed[hashes..].trim().to_string())
                        .filter(|label| !label.is_empty())
                }
                DocumentFormat::Notebook => None,
            };
        if detected.is_some() {
            section = detected;
        }
        let end = (char_offset + line.chars().count()).min(chars_len);
        for location in &mut locations[char_offset..end] {
            *location = Location {
                page,
                section: section.clone(),
            };
        }
        char_offset = end;
    }
    locations
}

/// Embed texts through llama-server's OpenAI-compatible endpoint. Every
/// response is validated before any vectors are returned, so callers cannot
/// accidentally mix malformed or wrong-model vectors into a derived index.
pub async fn embed(
    client: &Client,
    runtime: &EmbeddingRuntime,
    texts: &[String],
    kind: EmbeddingInputKind,
) -> Result<EmbeddingBatchResult, String> {
    if texts.is_empty() {
        return Ok(EmbeddingBatchResult {
            vectors: Vec::new(),
            metrics: EmbeddingBatchMetrics::default(),
        });
    }
    let started = Instant::now();
    let counter = ServerTokenCounter::new(client, &runtime.base_url);
    let mut prepared = Vec::with_capacity(texts.len());
    let mut token_counts = Vec::with_capacity(texts.len());
    for (index, text) in texts.iter().enumerate() {
        let input = runtime.contract.prepare_input(kind, text);
        let count = counter.count(&input).await?;
        if count > runtime.contract.max_input_tokens {
            return Err(format!("embedding input {index} has {count} tokens, exceeding this model contract's {}-token limit", runtime.contract.max_input_tokens));
        }
        prepared.push(input);
        token_counts.push(count);
    }
    let endpoint = format!("{}/v1/embeddings", runtime.base_url.trim_end_matches('/'));
    let mut output = Vec::with_capacity(texts.len());
    let mut cursor = 0usize;
    let mut requests = 0usize;
    while cursor < prepared.len() {
        let mut end = cursor;
        let mut batch_tokens = 0usize;
        while end < prepared.len() && end - cursor < 8 {
            let next = token_counts[end];
            if end > cursor && batch_tokens + next > 2048 {
                break;
            }
            batch_tokens += next;
            end += 1;
        }
        let response = client
            .post(&endpoint)
            .json(&json!({ "model": runtime.contract.model_id, "input": &prepared[cursor..end] }))
            .send()
            .await
            .map_err(|error| {
                format!("embedding request for inputs {cursor}..{end} failed: {error}")
            })?;
        let status = response.status();
        let raw = response
            .text()
            .await
            .map_err(|error| format!("embedding response read failed: {error}"))?;
        if !status.is_success() {
            return Err(format!(
                "embedding server returned HTTP {status}: {}",
                raw.chars().take(500).collect::<String>()
            ));
        }
        let vectors = parse_embed_response(&raw, end - cursor, runtime.contract.dimensions)?;
        output.extend(vectors);
        requests += 1;
        cursor = end;
    }
    Ok(EmbeddingBatchResult {
        vectors: output,
        metrics: EmbeddingBatchMetrics {
            request_count: requests,
            input_count: texts.len(),
            token_count: token_counts.into_iter().sum(),
            elapsed_ms: started.elapsed().as_millis(),
        },
    })
}

fn parse_embed_response(
    raw: &str,
    expected_count: usize,
    expected_dimensions: usize,
) -> Result<Vec<Vec<f32>>, String> {
    let value: serde_json::Value = serde_json::from_str(raw)
        .map_err(|error| format!("embedding JSON parse failed: {error}"))?;
    let data = value
        .get("data")
        .and_then(|value| value.as_array())
        .ok_or_else(|| "embedding response missing 'data' array".to_string())?;
    if data.len() != expected_count {
        return Err(format!(
            "embedding response contained {} vectors; expected {expected_count}",
            data.len()
        ));
    }
    let mut ordered: Vec<Option<Vec<f32>>> = vec![None; expected_count];
    for item in data {
        let index = item
            .get("index")
            .and_then(|value| value.as_u64())
            .ok_or_else(|| "embedding item missing numeric index".to_string())?
            as usize;
        if index >= expected_count || ordered[index].is_some() {
            return Err("embedding response contains an invalid or duplicate index".into());
        }
        let values = item
            .get("embedding")
            .and_then(|value| value.as_array())
            .ok_or_else(|| "embedding item missing embedding array".to_string())?;
        if values.is_empty() || values.len() != expected_dimensions {
            return Err(format!(
                "embedding item {index} has {} dimensions; expected {expected_dimensions}",
                values.len()
            ));
        }
        let mut vector = Vec::with_capacity(values.len());
        for value in values {
            let number = value
                .as_f64()
                .ok_or_else(|| format!("embedding item {index} contains a non-numeric value"))?
                as f32;
            if !number.is_finite() {
                return Err(format!(
                    "embedding item {index} contains a non-finite value"
                ));
            }
            vector.push(number);
        }
        normalize_vector(&mut vector)
            .ok_or_else(|| format!("embedding item {index} has zero magnitude"))?;
        ordered[index] = Some(vector);
    }
    ordered
        .into_iter()
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| "embedding response omitted an indexed result".to_string())
}

pub fn normalize_vector(vector: &mut [f32]) -> Option<()> {
    let magnitude = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
    if !magnitude.is_finite() || magnitude == 0.0 {
        return None;
    }
    for value in vector {
        *value /= magnitude;
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;
    struct CharCounter;
    impl TokenCounter for CharCounter {
        fn count<'a>(
            &'a self,
            text: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<usize, String>> + Send + 'a>> {
            Box::pin(async move { Ok(text.chars().count()) })
        }
    }
    #[tokio::test]
    async fn preserves_whitespace_and_bounds_chunks() {
        let text = format!(
            "# Intro\n\n{}\n\n[Page 2]\n{}",
            "a".repeat(300),
            "b".repeat(300)
        );
        let result =
            chunk_document(&text, DocumentFormat::Markdown, &CharCounter, RAG_CHUNKER).await;
        assert!(result
            .chunks
            .iter()
            .all(|chunk| chunk.token_count <= RAG_CHUNKER.hard_max_tokens));
        assert!(result.chunks.iter().all(|chunk| text.contains(&chunk.text)));
        assert!(result
            .chunks
            .iter()
            .any(|chunk| chunk.text.contains("\n\n")));
    }

    #[test]
    fn embedding_response_is_reordered_and_normalized() {
        let raw =
            r#"{"data":[{"index":1,"embedding":[0.0,4.0]},{"index":0,"embedding":[3.0,0.0]}]}"#;
        let vectors = parse_embed_response(raw, 2, 2).unwrap();
        assert_eq!(vectors[0], vec![1.0, 0.0]);
        assert_eq!(vectors[1], vec![0.0, 1.0]);
    }

    #[test]
    fn embedding_response_rejects_bad_vector_values() {
        let raw = r#"{"data":[{"index":0,"embedding":["bad",1.0]}]}"#;
        assert!(parse_embed_response(raw, 1, 2).is_err());
    }
    #[tokio::test]
    async fn fallback_is_conservative() {
        struct Broken;
        impl TokenCounter for Broken {
            fn count<'a>(
                &'a self,
                _: &'a str,
            ) -> Pin<Box<dyn Future<Output = Result<usize, String>> + Send + 'a>> {
                Box::pin(async { Err("down".into()) })
            }
        }
        let text = "é".repeat(500);
        let result = chunk_document(&text, DocumentFormat::PlainText, &Broken, RAG_CHUNKER).await;
        assert_eq!(result.mode, TokenizerMode::ConservativeFallback);
        assert!(result
            .chunks
            .iter()
            .all(|chunk| chunk.token_count <= RAG_CHUNKER.hard_max_tokens));
    }
}
