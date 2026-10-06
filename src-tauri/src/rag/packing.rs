//! Passage packing: format ranked chunks into bounded model evidence.

use super::types::RetrievedChunk;

/// Format ranked passages without allowing retrieval to consume the rest of
/// the model context. The final chunk is truncated when it reaches the budget.
#[cfg(test)]
pub fn pack_passages(chunks: Vec<RetrievedChunk>, char_budget: usize) -> String {
    pack_passages_limited(chunks, char_budget, usize::MAX)
}

/// Pack a bounded number of distinct passages for model evidence. Retrieval
/// may return overlapping vector/FTS hits; exact duplicate text should not
/// consume prompt budget twice.
pub fn pack_passages_limited(
    chunks: Vec<RetrievedChunk>,
    char_budget: usize,
    max_chunks: usize,
) -> String {
    let mut used = 0usize;
    let mut packed = 0usize;
    let mut seen = std::collections::HashSet::new();
    let mut kept = Vec::new();
    let mut evidence = String::new();
    for chunk in chunks {
        if used >= char_budget || packed >= max_chunks {
            break;
        }
        let key = chunk.text.trim();
        if key.is_empty() || !seen.insert(key.to_string()) || materially_overlaps(&chunk, &kept) {
            continue;
        }
        let remaining = char_budget - used;
        let excerpt: String = chunk.text.chars().take(remaining).collect();
        if excerpt.trim().is_empty() {
            continue;
        }
        evidence.push_str(&format!(
            "\n\n[{} | document {}]\n{}",
            chunk_label(&chunk),
            chunk.doc_id,
            excerpt.trim()
        ));
        used += excerpt.chars().count();
        packed += 1;
        kept.push(chunk);
    }
    evidence
}

/// Pack a small query-focused excerpt for direct document answers. The normal
/// packer keeps each passage's beginning, which can miss an exact name that
/// appears later in a long chunk. This variant centers the excerpt around the
/// most distinctive query term while retaining the source label.
pub fn pack_passages_focused(
    chunks: Vec<RetrievedChunk>,
    query: &str,
    char_budget: usize,
    max_chunks: usize,
) -> String {
    const STOP: &[&str] = &[
        "about", "does", "from", "mention", "paper", "tell", "that", "this", "what", "which",
        "with",
    ];
    let mut terms: Vec<String> = query
        .to_ascii_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|term| term.len() >= 4 && !STOP.contains(term))
        .map(str::to_string)
        .collect();
    terms.sort_by_key(|term| std::cmp::Reverse(term.len()));

    let mut used = 0usize;
    let mut packed = 0usize;
    let mut seen = std::collections::HashSet::new();
    let mut kept = Vec::new();
    let mut evidence = String::new();
    for chunk in chunks {
        if used >= char_budget || packed >= max_chunks {
            break;
        }
        if chunk.text.trim().is_empty()
            || !seen.insert(chunk.text.trim().to_string())
            || materially_overlaps(&chunk, &kept)
        {
            continue;
        }
        let remaining = char_budget - used;
        let text_lower = chunk.text.to_ascii_lowercase();
        let match_at = terms.iter().find_map(|term| text_lower.find(term));
        let start = match_at
            .map(|at| {
                text_lower[..at]
                    .chars()
                    .count()
                    .saturating_sub(remaining / 3)
            })
            .unwrap_or(0);
        let excerpt: String = chunk.text.chars().skip(start).take(remaining).collect();
        if excerpt.trim().is_empty() {
            continue;
        }
        evidence.push_str(&format!(
            "\n\n[{} | document {}]\n{}",
            chunk_label(&chunk),
            chunk.doc_id,
            excerpt.trim()
        ));
        used += excerpt.chars().count();
        packed += 1;
        kept.push(chunk);
    }
    evidence
}

fn chunk_label(chunk: &RetrievedChunk) -> String {
    let mut label = chunk.source.clone();
    if let (Some(start), Some(end)) = (chunk.page_start, chunk.page_end) {
        if start == end {
            label.push_str(&format!(" · page {start}"));
        } else {
            label.push_str(&format!(" · pages {start}–{end}"));
        }
    }
    if let (Some(start), Some(end)) = (&chunk.section_start, &chunk.section_end) {
        if start == end {
            label.push_str(&format!(" · {start}"));
        } else {
            label.push_str(&format!(" · {start} → {end}"));
        }
    } else if let Some(section) = &chunk.section_start {
        label.push_str(&format!(" · {section}"));
    }
    label
}

pub(super) fn materially_overlaps(candidate: &RetrievedChunk, kept: &[RetrievedChunk]) -> bool {
    let (Some(start), Some(end)) = (candidate.char_start, candidate.char_end) else {
        return false;
    };
    let length = (end - start).max(1) as f32;
    kept.iter().any(|other| {
        if other.doc_id != candidate.doc_id {
            return false;
        }
        let (Some(other_start), Some(other_end)) = (other.char_start, other.char_end) else {
            return false;
        };
        let overlap = (end.min(other_end) - start.max(other_start)).max(0) as f32;
        overlap / length >= 0.60
    })
}
