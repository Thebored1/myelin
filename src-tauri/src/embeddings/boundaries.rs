//! Source-preserving document chunking: where chunk boundaries land and why.
//!
//! Split out of the parent module because boundary placement is a self-contained
//! decision with its own invariants. The reasoning that matters — a boundary must
//! not depend on where the window started, or no chunk can survive an edit
//! elsewhere in the document — is easy to lose when it sits among HTTP plumbing.

use super::{ChunkerConfig, DocumentFormat, Location, TokenCounter};

/// Choose the `(end, tokens)` span for the chunk beginning at `start`.
///
/// A block boundary is preferred over a token-window edge because the boundary
/// is a function of the text inside one block, not of where the window started.
/// That is what keeps a chunk's text identical when an edit lands elsewhere in
/// the document, which is what makes incremental re-embedding possible.
///
/// A boundary only qualifies once it is past the target size and still within
/// the hard maximum. When none qualifies, the remaining blocks are individually
/// larger than the hard maximum — one oversized paragraph, or text with no
/// structure at all as with reflowed PDF text — and the sliding window is the
/// right answer. Falling back to the *nearest* boundary instead would end the
/// chunk immediately and emit one holding nothing but a heading.
pub(super) async fn next_span<C: TokenCounter>(
    chars: &[char],
    start: usize,
    config: &ChunkerConfig,
    counter: &C,
    preferred: &[usize],
    boundaries: &[usize],
    requests: &mut usize,
) -> Result<(usize, usize), String> {
    let floor = (start + 1).min(chars.len());
    let (target_end, target_count) =
        largest_end(chars, start, config.target_tokens, counter, requests).await?;
    let (max_end, _) = largest_end(chars, start, config.hard_max_tokens, counter, requests).await?;

    for point in boundaries
        .iter()
        .copied()
        .filter(|point| *point > start && *point <= max_end)
    {
        if count_chars(chars, start, point, counter, requests).await? >= config.target_tokens {
            return Ok((
                point,
                count_chars(chars, start, point, counter, requests).await?,
            ));
        }
    }

    let mut end = target_end.max(floor);
    if let Some(boundary) = preferred
        .iter()
        .copied()
        .filter(|point| *point > start && *point <= target_end)
        .last()
    {
        let candidate_count = count_chars(chars, start, boundary, counter, requests).await?;
        if candidate_count >= config.target_tokens / 2 {
            end = boundary;
        }
    }
    if end > max_end {
        end = max_end.max(floor);
    }
    let count = if end == target_end {
        target_count
    } else {
        count_chars(chars, start, end, counter, requests).await?
    };
    Ok((end, count))
}

/// Positions where a chunk is allowed to end, ascending.
///
/// The chunker slides a window and then nudges each end back to the nearest
/// whitespace. That end depends on where the window *started*, so inserting or
/// deleting text anywhere moves every later boundary — and when every chunk's
/// text shifts, none of them can reuse a stored embedding.
///
/// Anchoring ends to block starts makes each boundary a function of the text
/// inside one block, so editing a paragraph leaves the other chunks
/// byte-identical. A boundary sits at the first line of a block: the first line
/// after a blank run, any heading, and any page marker. Ends therefore land on
/// real content edges, and a chunk that follows a heading opens with it.
pub(super) fn block_boundaries(text: &str, format: DocumentFormat, chars_len: usize) -> Vec<usize> {
    let mut points = Vec::new();
    let mut offset = 0usize;
    // Treat the document start as a block start so the first block is emitted.
    let mut previous_blank = true;
    for line in text.split_inclusive('\n') {
        let start = offset;
        offset += line.chars().count();
        let trimmed = line.trim();
        let blank = trimmed.is_empty();
        let opens_block = !blank
            && (previous_blank || is_heading_line(trimmed, format) || is_page_marker(trimmed));
        if opens_block {
            points.push(start);
        }
        previous_blank = blank;
    }
    points.push(chars_len);
    points.retain(|point| *point > 0 && *point <= chars_len);
    points.dedup();
    points
}

fn is_heading_line(trimmed: &str, format: DocumentFormat) -> bool {
    match format {
        DocumentFormat::Latex => ["section", "subsection", "subsubsection"]
            .iter()
            .any(|command| trimmed.starts_with(&format!("\\{command}{{"))),
        DocumentFormat::Markdown
        | DocumentFormat::PdfText
        | DocumentFormat::PlainText
        | DocumentFormat::HtmlText
        | DocumentFormat::EpubText => {
            let hashes = trimmed.chars().take_while(|c| *c == '#').count();
            (1..=6).contains(&hashes) && !trimmed[hashes..].trim().is_empty()
        }
        DocumentFormat::Notebook => false,
    }
}

fn is_page_marker(trimmed: &str) -> bool {
    trimmed.starts_with("[Page ") && trimmed.ends_with(']')
}

pub(super) async fn largest_end<C: TokenCounter>(
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

pub(super) async fn count_chars<C: TokenCounter>(
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

pub(super) async fn overlap_start<C: TokenCounter>(
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

pub(super) fn preferred_boundaries(chars: &[char]) -> Vec<usize> {
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

pub(super) fn scan_locations(
    text: &str,
    format: DocumentFormat,
    chars_len: usize,
) -> Vec<Location> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embeddings::{chunk_document, CharCounter, Chunk, RAG_CHUNKER};
    /// A note-like body: a heading per chapter, then paragraphs of prose.
    ///
    /// Paragraph length decides whether block anchoring is exercised at all.
    /// `CharCounter` charges one token per character, and a boundary only counts
    /// when it falls inside the chunk's hard maximum while already past the
    /// target — so paragraphs must land between `target_tokens` (256) and
    /// `hard_max_tokens` (384). A longer fixture silently falls through to the
    /// sliding window and stops testing the anchored path; a shorter one packs
    /// several paragraphs per chunk and never isolates a single edit.
    fn structured_body() -> String {
        let mut body = String::from("# Probe\n");
        for chapter in 1..=12 {
            body.push_str(&format!("## Chapter {chapter}\n\n"));
            for paragraph in 1..=20 {
                let mut block = format!(
                    "Paragraph {chapter}.{paragraph} explains retrieval, caching and indexing \
                     for chapter {chapter}. "
                );
                while block.len() < 300 {
                    block.push_str(&format!("It revisits topic {chapter}{paragraph} again. "));
                }
                body.push_str(&block);
                body.push_str("\n\n");
            }
        }
        body
    }

    async fn markdown_chunks(text: &str) -> Vec<Chunk> {
        chunk_document(text, DocumentFormat::Markdown, &CharCounter, RAG_CHUNKER)
            .await
            .chunks
    }

    /// The property the whole incremental-embedding design rests on.
    ///
    /// Reuse is keyed on exact chunk text, so an edit is only cheap if it leaves
    /// the other chunks byte-identical. The previous sliding-window chunker moved
    /// every downstream boundary on any edit, which capped measured reuse at
    /// 8.4% on a real note — close enough to useless that the feature was not
    /// worth its cost. Block-anchored ends are a function of the text inside one
    /// block, so only the chunks touching the edit move.
    #[tokio::test]
    async fn editing_a_note_leaves_almost_every_chunk_text_identical() {
        let original = structured_body();
        let before = markdown_chunks(&original).await;
        assert!(before.len() > 50, "fixture should produce many chunks");

        for (label, needle) in [
            ("early", "Paragraph 1.5 "),
            ("middle", "Paragraph 6.10 "),
            ("late", "Paragraph 11.18 "),
        ] {
            let edited = original.replacen(needle, "ZZZEDIT ", 1);
            assert!(
                edited.contains("ZZZEDIT"),
                "{label}: fixture edit did not apply"
            );
            let after = markdown_chunks(&edited).await;
            let known: std::collections::HashSet<&str> =
                before.iter().map(|chunk| chunk.text.as_str()).collect();
            let identical = after
                .iter()
                .filter(|chunk| known.contains(chunk.text.as_str()))
                .count();
            let ratio = identical as f64 / after.len() as f64;
            assert!(
                ratio >= 0.95,
                "{label} edit changed {ratio:.1}% of chunks ({identical}/{}); \
                 block anchoring should leave the rest byte-identical",
                after.len()
            );
        }
    }

    /// Guards the premise of the stability test above. If the fixture stopped
    /// landing between the target and the hard maximum, boundaries would fall
    /// through to the sliding window, the identical-chunk ratio would still look
    /// healthy for the wrong reason, and the real anchored path would go untested.
    #[tokio::test]
    async fn fixture_paragraphs_land_inside_the_anchored_window() {
        let body = structured_body();
        let paragraphs: Vec<&str> = body
            .split("\n\n")
            .filter(|block| block.starts_with("Paragraph"))
            .collect();
        assert!(!paragraphs.is_empty(), "fixture has no paragraphs");
        for block in paragraphs {
            assert!(
                block.chars().count() >= RAG_CHUNKER.target_tokens,
                "paragraph shorter than target_tokens: {} chars",
                block.chars().count()
            );
            assert!(
                block.chars().count() <= RAG_CHUNKER.hard_max_tokens,
                "paragraph longer than hard_max_tokens: {} chars",
                block.chars().count()
            );
        }
    }

    /// Anchoring must not trade stability for chunk quality: no chunk may exceed
    /// the hard maximum (which would overflow the embedder's input limit), and
    /// no chunk but the last may collapse below a useful size.
    #[tokio::test]
    async fn anchored_chunks_respect_size_bounds() {
        let chunks = markdown_chunks(&structured_body()).await;
        for chunk in &chunks {
            assert!(
                chunk.token_count <= RAG_CHUNKER.hard_max_tokens,
                "chunk {} has {} tokens, over the {} hard maximum",
                chunk.index,
                chunk.token_count,
                RAG_CHUNKER.hard_max_tokens
            );
        }
        let floor = RAG_CHUNKER.target_tokens / 2;
        for chunk in &chunks[..chunks.len() - 1] {
            assert!(
                chunk.token_count >= floor,
                "chunk {} is only {} tokens, under the {floor} floor",
                chunk.index,
                chunk.token_count
            );
        }
    }

    /// A single paragraph larger than the hard maximum has no boundary to anchor
    /// to. It must fall back to splitting rather than emitting a chunk per
    /// character: an earlier version took the next block start unconditionally,
    /// which ended each chunk immediately and produced one chunk per character.
    #[tokio::test]
    async fn oversized_blocks_split_without_collapsing() {
        let text = format!(
            "{}\n\n{}",
            "alpha beta gamma ".repeat(400),
            "tail ".repeat(30)
        );
        let chunks = markdown_chunks(&text).await;
        assert!(
            chunks.len() < 40,
            "a structureless body should split into a few large chunks, got {}",
            chunks.len()
        );
        for chunk in &chunks {
            assert!(chunk.token_count <= RAG_CHUNKER.hard_max_tokens);
        }
        assert!(
            chunks.iter().all(|chunk| chunk.token_count > 1),
            "no chunk should collapse to a single token"
        );
    }
}
