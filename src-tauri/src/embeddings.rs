//! Embeddings & RAG foundation: the client that turns text into vectors via a
//! local nomic-embed `llama-server` (embedding mode), plus the chunker that
//! splits large documents into overlapping windows so a whole book is never put
//! in the model's context — only the few retrieved chunks are.
//!
//! nomic-embed-text v1.5 is asymmetric: documents must be prefixed
//! `search_document:` and queries `search_query:`, or retrieval quality drops.

use reqwest::Client;
use serde_json::json;

/// nomic-embed-text v1.5 output width (full Matryoshka, max quality).
pub const EMBED_DIM: usize = 768;

const TASK_QUERY: &str = "search_query: ";
const TASK_DOCUMENT: &str = "search_document: ";

/// One chunk of a document, ready to embed and store.
#[derive(Debug, Clone, PartialEq)]
pub struct Chunk {
    pub text: String,
    /// Sequential index within the source document.
    pub index: usize,
    /// Word offset of the chunk start (for citation / dedup).
    pub word_start: usize,
}

/// Split text into overlapping word-windows of ~`target_words` with
/// `overlap_words` carried between them. Approximate by design — the embedder
/// tokenizes precisely; ~320 words ≈ ~430 tokens, far under nomic's 8192 limit.
/// Overlap preserves context that would otherwise be cut mid-thought.
pub fn chunk_text(text: &str, target_words: usize, overlap_words: usize) -> Vec<Chunk> {
    let target = target_words.max(20);
    let overlap = overlap_words.min(target / 2);
    let step = (target - overlap).max(1);

    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return Vec::new();
    }

    let mut chunks = Vec::new();
    let mut start = 0;
    let mut index = 0;
    while start < words.len() {
        let end = (start + target).min(words.len());
        chunks.push(Chunk {
            text: words[start..end].join(" "),
            index,
            word_start: start,
        });
        index += 1;
        if end == words.len() {
            break;
        }
        start += step;
    }
    chunks
}

/// Embed a batch of texts via the embedding server's OpenAI-compatible endpoint.
/// `is_query` selects the nomic task prefix (query vs document).
pub async fn embed(
    client: &Client,
    base_url: &str,
    model: &str,
    texts: &[String],
    is_query: bool,
) -> Result<Vec<Vec<f32>>, String> {
    if texts.is_empty() {
        return Ok(Vec::new());
    }
    let prefix = if is_query { TASK_QUERY } else { TASK_DOCUMENT };
    let input: Vec<String> = texts.iter().map(|t| format!("{prefix}{t}")).collect();
    let body = json!({ "model": model, "input": input });

    let resp = client
        .post(format!("{}/v1/embeddings", base_url.trim_end_matches('/')))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("embedding request failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("embedding server returned HTTP {}", resp.status()));
    }
    let raw = resp
        .text()
        .await
        .map_err(|e| format!("embedding read failed: {e}"))?;
    parse_embed_response(&raw)
}

/// Parse the OpenAI-style embeddings response: `{ "data": [ { "embedding": [..] } ] }`.
fn parse_embed_response(raw: &str) -> Result<Vec<Vec<f32>>, String> {
    let v: serde_json::Value =
        serde_json::from_str(raw).map_err(|e| format!("embedding JSON parse failed: {e}"))?;
    let data = v
        .get("data")
        .and_then(|d| d.as_array())
        .ok_or_else(|| "embedding response missing 'data' array".to_string())?;
    let mut out = Vec::with_capacity(data.len());
    for item in data {
        let vec = item
            .get("embedding")
            .and_then(|e| e.as_array())
            .ok_or_else(|| "embedding item missing 'embedding'".to_string())?
            .iter()
            .map(|x| x.as_f64().unwrap_or(0.0) as f32)
            .collect::<Vec<f32>>();
        out.push(vec);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_overlap_and_cover() {
        let text = (0..1000)
            .map(|i| format!("w{i}"))
            .collect::<Vec<_>>()
            .join(" ");
        let chunks = chunk_text(&text, 320, 50);
        assert!(chunks.len() >= 3);
        assert_eq!(chunks[0].word_start, 0);
        // step = target - overlap = 270
        assert_eq!(chunks[1].word_start, 270);
        // first chunk holds `target` words
        assert_eq!(chunks[0].text.split_whitespace().count(), 320);
    }

    #[test]
    fn empty_and_short_text() {
        assert!(chunk_text("", 320, 50).is_empty());
        let one = chunk_text("just a few words here", 320, 50);
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].word_start, 0);
    }

    #[test]
    fn parse_embeddings_ok() {
        let raw = r#"{"data":[{"embedding":[0.1,0.2,0.3]},{"embedding":[0.4,0.5,0.6]}]}"#;
        let parsed = parse_embed_response(raw).unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0], vec![0.1_f32, 0.2, 0.3]);
        assert_eq!(parsed[1], vec![0.4_f32, 0.5, 0.6]);
    }

    #[test]
    fn parse_embeddings_rejects_garbage() {
        assert!(parse_embed_response("{}").is_err());
        assert!(parse_embed_response("not json").is_err());
    }
}
