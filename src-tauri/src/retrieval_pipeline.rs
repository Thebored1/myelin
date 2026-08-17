//! Deterministic, low-latency retrieval query analysis and candidate fusion.
//!
//! This module deliberately has no model dependency: ordinary document
//! retrieval must remain available when a local embedding or chat runtime is
//! unhealthy.

use crate::rag::RetrievedChunk;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Mutex, OnceLock};

pub const VECTOR_POOL: usize = 24;
pub const FTS_POOL: usize = 24;
pub const CANDIDATE_UNION_CAP: usize = 36;
pub const MMR_LAMBDA: f32 = 0.78;
pub const FINAL_SEED_COUNT: usize = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetrievalIntent {
    Lookup,
    BroadSummary,
    Quotation,
    Comparison,
    Synthesis,
    MultiHop,
}

#[derive(Debug, Clone)]
pub struct WeightedTerm {
    pub term: String,
    pub weight: f32,
}

#[derive(Debug, Clone)]
pub struct RetrievalQuery {
    pub original: String,
    pub normalized: String,
    pub phrases: Vec<String>,
    pub weighted_terms: Vec<WeightedTerm>,
    pub entities: Vec<String>,
    pub identifiers: Vec<String>,
    pub urls: Vec<String>,
    pub intent: RetrievalIntent,
    pub requested_sources: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct RetrievalCandidate {
    pub chunk: RetrievedChunk,
    pub vector_rank: Option<usize>,
    pub fts_rank: Option<usize>,
    pub cosine_similarity: Option<f32>,
    pub bm25_score: Option<f32>,
    pub bm25_normalized: Option<f32>,
    pub term_coverage: f32,
    pub phrase_match: f32,
    pub field_match: f32,
    pub base_score: f32,
    pub reranker_score: Option<f32>,
    pub final_score: f32,
}

/// Apply the bounded reranker result to an already calibrated base ranking.
/// The base score is approximated from the stable first-stage order here because
/// the document API intentionally keeps ranking internals out of RetrievedChunk.
pub fn blend_reranker_scores(chunks: &mut [RetrievedChunk], scores: &[f32]) {
    if chunks.is_empty() || chunks.len() != scores.len() {
        return;
    }
    let min = scores.iter().copied().fold(f32::INFINITY, f32::min);
    let max = scores.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let span = (max - min).max(f32::EPSILON);
    let count = chunks.len().max(1) as f32;
    let mut ranked = chunks
        .iter_mut()
        .zip(scores.iter().copied())
        .collect::<Vec<_>>();
    for (index, (chunk, score)) in ranked.iter_mut().enumerate() {
        let reranker = ((*score - min) / span).clamp(0.0, 1.0);
        let base = 1.0 - index as f32 / count;
        // Keep the original first-stage ordering as a deterministic tie-breaker.
        chunk.distance = -(0.65 * reranker + 0.35 * base);
    }
    chunks.sort_by(|left, right| right.distance.total_cmp(&left.distance));
}

const STOP: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "does", "find", "for", "from", "in", "is", "it",
    "me", "of", "on", "or", "show", "tell", "that", "the", "this", "to", "what", "which", "with",
    "about", "document", "paper", "pdf",
];

impl RetrievalQuery {
    pub fn analyze(input: &str) -> Self {
        let authoritative = latest_user_question(input);
        let phrases = quoted_phrases(authoritative);
        let urls = authoritative
            .split_whitespace()
            .filter(|part| part.starts_with("http://") || part.starts_with("https://"))
            .map(|part| {
                part.trim_matches(|c: char| "()[]{}\"'.,;:!?".contains(c))
                    .to_string()
            })
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>();
        let raw_terms = split_terms(authoritative);
        let mut identifiers = Vec::new();
        let mut entities = Vec::new();
        let mut weighted_terms = Vec::new();
        let mut seen = HashSet::new();
        for raw in raw_terms {
            let lower = raw.to_lowercase();
            if lower.is_empty() || STOP.contains(&lower.as_str()) {
                continue;
            }
            let is_identifier = raw.chars().any(|c| c.is_ascii_digit())
                || raw.contains('_')
                || raw.contains('/')
                || raw.contains('\\')
                || raw.contains("::")
                || raw.chars().any(|c| c.is_uppercase()) && raw.chars().any(|c| c.is_lowercase());
            if is_identifier {
                identifiers.push(raw.clone());
            }
            if raw.chars().next().is_some_and(char::is_uppercase) && raw.len() > 1 {
                entities.push(lower.clone());
            }
            if seen.insert(lower.clone()) {
                let weight = if is_identifier || raw.chars().any(|c| c.is_ascii_digit()) {
                    1.5
                } else {
                    1.0
                };
                weighted_terms.push(WeightedTerm {
                    term: lower,
                    weight,
                });
            }
        }
        for url in &urls {
            for part in split_terms(url) {
                let lower = part.to_lowercase();
                if !lower.is_empty() && seen.insert(lower.clone()) {
                    weighted_terms.push(WeightedTerm {
                        term: lower,
                        weight: 1.5,
                    });
                }
            }
        }
        let normalized = weighted_terms
            .iter()
            .map(|term| term.term.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        let lower = authoritative.to_lowercase();
        let intent = if ["compare", "difference", "versus", " vs "]
            .iter()
            .any(|m| lower.contains(m))
        {
            RetrievalIntent::Comparison
        } else if ["quote", "recite", "transcribe", "verbatim"]
            .iter()
            .any(|m| lower.contains(m))
        {
            RetrievalIntent::Quotation
        } else if [
            "summarize",
            "summary",
            "overview",
            "all ",
            "every ",
            "across",
        ]
        .iter()
        .any(|m| lower.contains(m))
        {
            RetrievalIntent::BroadSummary
        } else if ["why", "how does", "connect", "relate"]
            .iter()
            .any(|m| lower.contains(m))
        {
            RetrievalIntent::Synthesis
        } else if ["then", "after that", "also"]
            .iter()
            .any(|m| lower.contains(m))
        {
            RetrievalIntent::MultiHop
        } else {
            RetrievalIntent::Lookup
        };
        Self {
            original: authoritative.to_string(),
            normalized,
            phrases,
            weighted_terms,
            entities,
            identifiers,
            urls,
            intent,
            requested_sources: Vec::new(),
        }
    }

    /// A small process-local LRU avoids repeating Unicode/token analysis while
    /// keeping query text out of persistent state.
    pub fn analyze_cached(input: &str) -> Self {
        const CAPACITY: usize = 128;
        static CACHE: OnceLock<Mutex<VecDeque<(String, RetrievalQuery)>>> = OnceLock::new();
        let cache = CACHE.get_or_init(|| Mutex::new(VecDeque::new()));
        let key = input.trim().to_string();
        let mut cache = cache.lock().expect("retrieval query cache poisoned");
        if let Some(position) = cache.iter().position(|(existing, _)| existing == &key) {
            let entry = cache.remove(position).expect("retrieval query cache entry");
            let value = entry.1.clone();
            cache.push_back(entry);
            return value;
        }
        let value = Self::analyze(&key);
        cache.push_back((key, value.clone()));
        if cache.len() > CAPACITY {
            cache.pop_front();
        }
        value
    }

    /// A conservative plain-term query. Phrase and structural scoring occur in
    /// memory, avoiding FTS grammar injection and language-specific parsing.
    pub fn fts_query(&self) -> String {
        self.weighted_terms
            .iter()
            .map(|term| term.term.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

pub fn latest_user_question(input: &str) -> &str {
    let input = input.strip_prefix("Latest question:").unwrap_or(input);
    input
        .find("\nPrevious ")
        .map(|end| &input[..end])
        .unwrap_or(input)
        .trim()
}

pub fn lexical_text(source: &str, section: Option<&str>, text: &str) -> String {
    let mut pieces = vec![source.to_string()];
    if let Some(section) = section.filter(|value| !value.trim().is_empty()) {
        pieces.push(section.to_string());
    }
    pieces.push(text.to_string());
    let combined = pieces.join("\n");
    let mut additions = Vec::new();
    for token in combined.split_whitespace() {
        if token.contains('_')
            || token.contains('/')
            || token.contains('\\')
            || token.contains("::")
            || has_camel_case(token)
        {
            additions.extend(split_identifier(token));
        }
    }
    additions.extend(cjk_bigrams(&combined));
    format!(
        "{}\n{}",
        combined.split_whitespace().collect::<Vec<_>>().join(" "),
        additions.join(" ")
    )
}

pub fn rank_candidates(
    query: &RetrievalQuery,
    vector_hits: Vec<RetrievedChunk>,
    fts_hits: Vec<RetrievedChunk>,
    limit: usize,
) -> Vec<RetrievalCandidate> {
    let mut candidates: HashMap<(String, i32), RetrievalCandidate> = HashMap::new();
    for (rank, chunk) in vector_hits.into_iter().enumerate() {
        let key = (chunk.doc_id.clone(), chunk.chunk_index);
        let similarity = (1.0 - chunk.distance).clamp(0.0, 1.0);
        candidates.entry(key).or_insert_with(|| RetrievalCandidate {
            chunk,
            vector_rank: Some(rank + 1),
            fts_rank: None,
            cosine_similarity: Some(similarity),
            bm25_score: None,
            bm25_normalized: None,
            term_coverage: 0.0,
            phrase_match: 0.0,
            field_match: 0.0,
            base_score: 0.0,
            reranker_score: None,
            final_score: 0.0,
        });
    }
    for (rank, chunk) in fts_hits.into_iter().enumerate() {
        let key = (chunk.doc_id.clone(), chunk.chunk_index);
        let score = chunk
            .bm25_score
            .filter(|value| value.is_finite() && *value >= 0.0);
        if let Some(existing) = candidates.get_mut(&key) {
            existing.fts_rank = Some(rank + 1);
            existing.bm25_score = score;
        } else {
            candidates.insert(
                key,
                RetrievalCandidate {
                    chunk,
                    vector_rank: None,
                    fts_rank: Some(rank + 1),
                    cosine_similarity: None,
                    bm25_score: score,
                    bm25_normalized: None,
                    term_coverage: 0.0,
                    phrase_match: 0.0,
                    field_match: 0.0,
                    base_score: 0.0,
                    reranker_score: None,
                    final_score: 0.0,
                },
            );
        }
    }
    let best_bm25 = candidates
        .values()
        .filter_map(|candidate| candidate.bm25_score)
        .fold(0.0_f32, f32::max);
    let has_vector = candidates
        .values()
        .any(|candidate| candidate.vector_rank.is_some());
    let has_fts = candidates
        .values()
        .any(|candidate| candidate.fts_rank.is_some());
    let mut values = candidates.into_values().collect::<Vec<_>>();
    for candidate in &mut values {
        candidate.bm25_normalized = candidate.bm25_score.map(|score| {
            if best_bm25 > 0.0 {
                (score / best_bm25).clamp(0.0, 1.0)
            } else {
                0.0
            }
        });
        let searchable = format!(
            "{}\n{}\n{}",
            candidate.chunk.source,
            candidate.chunk.section_start.as_deref().unwrap_or_default(),
            candidate.chunk.text
        )
        .to_lowercase();
        candidate.term_coverage = term_coverage(query, &searchable);
        candidate.phrase_match = phrase_match(query, &searchable);
        candidate.field_match = field_match(query, &candidate.chunk);
        let (semantic_weight, bm25_weight, coverage_weight, field_weight) =
            match (has_vector, has_fts) {
                (true, true) => (0.50, 0.25, 0.15, 0.10),
                (false, true) => (0.0, 0.58, 0.27, 0.15),
                (true, false) => (0.80, 0.0, 0.15, 0.05),
                (false, false) => (0.0, 0.0, 0.0, 0.0),
            };
        candidate.base_score = semantic_weight * candidate.cosine_similarity.unwrap_or(0.0)
            + bm25_weight * candidate.bm25_normalized.unwrap_or(0.0)
            + coverage_weight * candidate.term_coverage
            + field_weight * candidate.field_match
            + candidate.phrase_match * 0.12;
        candidate.final_score = candidate.base_score;
    }
    values.sort_by(|left, right| {
        right
            .final_score
            .total_cmp(&left.final_score)
            .then_with(|| right.base_score.total_cmp(&left.base_score))
            .then_with(|| {
                left.vector_rank
                    .or(left.fts_rank)
                    .unwrap_or(usize::MAX)
                    .cmp(&right.vector_rank.or(right.fts_rank).unwrap_or(usize::MAX))
            })
            .then_with(|| left.chunk.doc_id.cmp(&right.chunk.doc_id))
            .then_with(|| left.chunk.chunk_index.cmp(&right.chunk.chunk_index))
    });
    values.truncate(limit.min(CANDIDATE_UNION_CAP));
    values
}

/// Select a compact, non-redundant seed set before evidence packing. This is
/// intentionally lexical/source-span based in this stage: it adds no embedding
/// request or extra table scan to the hot path.
pub fn select_diverse(
    candidates: Vec<RetrievalCandidate>,
    intent: RetrievalIntent,
    scoped_to_one_document: bool,
) -> Vec<RetrievalCandidate> {
    let per_document_cap = if scoped_to_one_document {
        usize::MAX
    } else if matches!(
        intent,
        RetrievalIntent::BroadSummary | RetrievalIntent::Comparison | RetrievalIntent::Synthesis
    ) {
        4
    } else {
        2
    };
    let mut pending = candidates;
    let mut selected = Vec::new();
    let mut per_document: HashMap<String, usize> = HashMap::new();
    while !pending.is_empty() && selected.len() < FINAL_SEED_COUNT {
        let mut best = None;
        for (index, candidate) in pending.iter().enumerate() {
            if per_document
                .get(&candidate.chunk.doc_id)
                .copied()
                .unwrap_or(0)
                >= per_document_cap
            {
                continue;
            }
            if selected.iter().any(|other: &RetrievalCandidate| {
                source_overlap(&candidate.chunk, &other.chunk) >= 0.60
            }) {
                continue;
            }
            let redundancy = selected
                .iter()
                .map(|other: &RetrievalCandidate| {
                    source_overlap(&candidate.chunk, &other.chunk)
                        .max(shingle_jaccard(&candidate.chunk.text, &other.chunk.text))
                })
                .fold(0.0_f32, f32::max);
            let score = MMR_LAMBDA * candidate.final_score - (1.0 - MMR_LAMBDA) * redundancy;
            if best
                .as_ref()
                .is_none_or(|(_, current): &(usize, f32)| score > *current)
            {
                best = Some((index, score));
            }
        }
        let Some((index, _)) = best else {
            break;
        };
        let candidate = pending.remove(index);
        *per_document
            .entry(candidate.chunk.doc_id.clone())
            .or_default() += 1;
        selected.push(candidate);
    }
    selected
}

pub fn source_overlap(left: &RetrievedChunk, right: &RetrievedChunk) -> f32 {
    if left.doc_id != right.doc_id {
        return 0.0;
    }
    let (Some(start), Some(end), Some(other_start), Some(other_end)) = (
        left.char_start,
        left.char_end,
        right.char_start,
        right.char_end,
    ) else {
        return 0.0;
    };
    let overlap = (end.min(other_end) - start.max(other_start)).max(0) as f32;
    overlap / (end - start).max(1) as f32
}

fn shingle_jaccard(left: &str, right: &str) -> f32 {
    let shingles = |value: &str| -> HashSet<String> {
        let normalized = value.to_lowercase();
        let terms = normalized.split_whitespace().collect::<Vec<_>>();
        terms
            .windows(2)
            .map(|pair| format!("{}\u{0}{}", pair[0], pair[1]))
            .collect()
    };
    let left = shingles(left);
    let right = shingles(right);
    if left.is_empty() || right.is_empty() {
        return 0.0;
    }
    let intersection = left.intersection(&right).count() as f32;
    intersection / left.union(&right).count() as f32
}

fn term_coverage(query: &RetrievalQuery, text: &str) -> f32 {
    let total = query
        .weighted_terms
        .iter()
        .map(|term| term.weight)
        .sum::<f32>();
    if total <= 0.0 {
        return 0.0;
    }
    query
        .weighted_terms
        .iter()
        .filter(|term| contains_term(text, &term.term))
        .map(|term| term.weight)
        .sum::<f32>()
        / total
}

fn phrase_match(query: &RetrievalQuery, text: &str) -> f32 {
    if query.phrases.is_empty() {
        return 0.0;
    }
    let hits = query
        .phrases
        .iter()
        .filter(|phrase| text.contains(&phrase.to_lowercase()))
        .count();
    hits as f32 / query.phrases.len() as f32
}

fn field_match(query: &RetrievalQuery, chunk: &RetrievedChunk) -> f32 {
    let source = chunk.source.to_lowercase();
    let section = chunk
        .section_start
        .as_deref()
        .unwrap_or_default()
        .to_lowercase();
    let mut score: f32 = 0.0;
    for term in &query.weighted_terms {
        if contains_term(&source, &term.term) {
            score += 0.30 * term.weight.min(1.5);
        }
        if contains_term(&section, &term.term) {
            score += 0.35 * term.weight.min(1.5);
        }
    }
    for phrase in &query.phrases {
        if source.contains(&phrase.to_lowercase()) || section.contains(&phrase.to_lowercase()) {
            score += 0.35;
        }
    }
    score.min(1.0)
}

fn quoted_phrases(input: &str) -> Vec<String> {
    let mut phrases = Vec::new();
    let mut quote = None;
    let mut start = 0usize;
    for (index, ch) in input.char_indices() {
        if matches!(ch, '\"' | '“' | '”' | '\'') {
            if quote.take().is_some() {
                let phrase = input[start..index].trim();
                if !phrase.is_empty() {
                    phrases.push(phrase.to_string());
                }
            } else {
                quote = Some(ch);
                start = index + ch.len_utf8();
            }
        }
    }
    phrases
}

fn split_terms(input: &str) -> Vec<String> {
    input
        .split(|c: char| !(c.is_alphanumeric() || matches!(c, '_' | '-' | '/' | '\\' | ':' | '.')))
        .filter(|part| !part.is_empty())
        .flat_map(|part| {
            let mut values = vec![part.to_string()];
            values.extend(split_identifier(part));
            values
        })
        .collect()
}

fn split_identifier(value: &str) -> Vec<String> {
    value
        .split(|c: char| matches!(c, '_' | '-' | '/' | '\\' | ':' | '.'))
        .flat_map(|part| split_camel_case(part))
        .filter(|part| !part.is_empty())
        .collect()
}

fn split_camel_case(value: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut previous_lower = false;
    for ch in value.chars() {
        if previous_lower && ch.is_uppercase() && !current.is_empty() {
            parts.push(std::mem::take(&mut current));
        }
        previous_lower = ch.is_lowercase();
        current.push(ch);
    }
    if !current.is_empty() {
        parts.push(current);
    }
    parts
}

fn has_camel_case(value: &str) -> bool {
    value.chars().any(char::is_lowercase) && value.chars().any(char::is_uppercase)
}

fn cjk_bigrams(value: &str) -> Vec<String> {
    let chars = value.chars().collect::<Vec<_>>();
    chars
        .windows(2)
        .filter_map(|pair| (is_cjk(pair[0]) && is_cjk(pair[1])).then(|| pair.iter().collect()))
        .collect()
}

fn is_cjk(ch: char) -> bool {
    matches!(ch as u32, 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x3040..=0x30FF | 0xAC00..=0xD7AF)
}

fn contains_term(text: &str, term: &str) -> bool {
    if term.chars().count() <= 2 || term.chars().any(is_cjk) {
        text.contains(term)
    } else {
        text.split(|c: char| !c.is_alphanumeric())
            .any(|word| word == term)
            || text.contains(term)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_analysis_preserves_identifiers_and_phrases() {
        let query = RetrievalQuery::analyze(
            "Find `foo_bar` at https://example.test/a-b and \"exact phrase\"",
        );
        assert!(query.identifiers.iter().any(|term| term == "foo_bar"));
        assert!(query.urls.iter().any(|url| url.contains("example.test")));
        assert_eq!(query.phrases, vec!["exact phrase"]);
    }

    #[test]
    fn lexical_text_has_identifier_and_cjk_variants() {
        let text = lexical_text("ReadMe", Some("API Guide"), "Use camelCase and 检索质量.");
        assert!(text.contains("camel"));
        assert!(text.contains("检索"));
    }

    #[test]
    fn mmr_drops_overlapping_seeds() {
        let candidate = |index, start, end| RetrievalCandidate {
            chunk: RetrievedChunk {
                doc_id: "d".into(),
                chunk_index: index,
                char_start: Some(start),
                char_end: Some(end),
                text: format!("chunk {index}"),
                ..Default::default()
            },
            vector_rank: Some(index as usize),
            fts_rank: None,
            cosine_similarity: Some(1.0),
            bm25_score: None,
            bm25_normalized: None,
            term_coverage: 1.0,
            phrase_match: 0.0,
            field_match: 0.0,
            base_score: 1.0,
            reranker_score: None,
            final_score: 1.0 - index as f32 / 10.0,
        };
        let selected = select_diverse(
            vec![
                candidate(0, 0, 100),
                candidate(1, 20, 100),
                candidate(2, 120, 200),
            ],
            RetrievalIntent::Lookup,
            false,
        );
        assert_eq!(selected.len(), 2);
        assert!(selected.iter().any(|value| value.chunk.chunk_index == 2));
    }
}
