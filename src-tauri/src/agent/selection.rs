use regex::Regex;
use serde::Deserialize;
/// An editor text selection the user armed, sent alongside the chat request.
/// `text` is the selected source markdown; `before`/`after` are short surrounding
/// context snippets used to pin the exact occurrence so repeats — and a body that
/// drifts between turns — don't move the target. Sent as text+context rather than
/// raw offsets to avoid the JS-UTF16 vs Rust-UTF8 index mismatch.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectionArg {
    pub text: String,
    #[serde(default)]
    pub before: String,
    #[serde(default)]
    pub after: String,
    /// A zero-length target captured from the editor caret. In this mode
    /// `before`/`after` locate an insertion boundary rather than selected text.
    #[serde(default)]
    pub cursor: bool,
    /// UTF-16 source offset captured from Vditor's Markdown serializer. This
    /// is preferred when the note is unchanged; anchors remain the fallback
    /// when the body drifted while the request was pending.
    #[serde(default)]
    pub source_offset: Option<usize>,
    /// Extra line breaks requested when the user clicked below the last editor block.
    #[serde(default)]
    pub line_breaks: usize,
    /// For Jupyter notebooks, the 0-based cell containing this source target.
    #[serde(default)]
    pub cell_index: Option<usize>,
}

/// Locate the byte range in `body` an armed selection refers to. Picks the
/// occurrence of `text` whose surrounding context best matches `before`/`after`,
/// so it survives repeats and edits elsewhere. None if the text no longer occurs
/// (the user changed that span — caller falls back to normal planning).
pub fn locate_selection(body: &str, sel: &SelectionArg) -> Option<(usize, usize)> {
    // The selection captured from the rendered editor often has leading/trailing
    // whitespace that isn't in the source (e.g. it ran past a paragraph), so trim
    // before matching — we only ever want to replace the text itself.
    let text = sel.text.trim();
    if text.is_empty() {
        return None;
    }
    // Compare anchors whitespace-tolerantly: the captured selection's boundaries
    // rarely line up byte-for-byte with the source (markers, stray newlines).
    let before = sel.before.trim();
    let after = sel.after.trim();
    let mut best: Option<(u8, usize, usize)> = None; // (context score, start, end)
    let mut from = 0;
    while let Some(rel) = body[from..].find(text) {
        let start = from + rel;
        let end = start + text.len();
        let before_ok = before.is_empty() || body[..start].trim_end().ends_with(before);
        let after_ok = after.is_empty() || body[end..].trim_start().starts_with(after);
        let score = before_ok as u8 + after_ok as u8;
        if best.map(|(s, _, _)| score > s).unwrap_or(true) {
            best = Some((score, start, end));
            if score == 2 {
                break; // both anchors match — definitely the right occurrence
            }
        }
        from = start + 1;
    }
    // Fallback for a selection that doesn't match the source verbatim — most often
    // a FORMATTED span where the rendered selection dropped the markdown markers
    // (**bold**, `code`, a heading's `# `). find_tolerant matches the inner words
    // (whitespace-tolerant), so we splice inside the markers and keep them.
    best.map(|(_, s, e)| (s, e))
        .or_else(|| find_tolerant(body, text))
}

/// If the user armed a selection, build a plan that replaces ONLY that span with
/// the model's `content`, keeping the rest of the note byte-identical. Returns
/// None (caller falls through to normal planning) when there's no usable span
/// or the model
/// regenerated the surrounding note (its content already contains the text just
/// before/after the selection → it did a full rewrite, which we honor instead).
pub fn selection_scoped_plan(body: &str, content: &str, sel: &SelectionArg) -> Option<WritePlan> {
    let content = clean_note_content(&strip_prompt_markers(content));
    if sel.cursor {
        // Markdown frontmatter parsing may represent a visually empty editor as
        // one or more newlines while the frontend's cursor target has no anchors.
        // Every boundary would otherwise match and be rejected as ambiguous,
        // even though an empty note has only one meaningful insertion location.
        if body.trim().is_empty() && sel.before.trim().is_empty() && sel.after.trim().is_empty() {
            return Some(WritePlan {
                new_body: content,
                op: WriteOp::EditSnippet,
            });
        }
        let Some(position) = locate_cursor(body, sel) else {
            return None;
        };
        let mut insertion = strip_cursor_echo(body, position, &content);
        if sel.line_breaks > 0 {
            let existing_breaks = body[..position]
                .chars()
                .rev()
                .take_while(|character| *character == '\n')
                .count();
            let generated_breaks = insertion
                .chars()
                .take_while(|character| *character == '\n')
                .count();
            let needed_breaks = sel
                .line_breaks
                .saturating_sub(existing_breaks + generated_breaks);
            if needed_breaks > 0 {
                insertion = format!("{}{}", "\n".repeat(needed_breaks), insertion);
            }
        }
        let left_alnum = adjacent_visible_alphanumeric(&body[..position], true);
        let right_alnum = adjacent_visible_alphanumeric(&body[position..], false);
        if left_alnum && insertion.chars().next().is_some_and(char::is_alphanumeric) {
            insertion.insert(0, ' ');
        }
        if right_alnum
            && insertion
                .chars()
                .next_back()
                .is_some_and(char::is_alphanumeric)
        {
            insertion.push(' ');
        }
        let mut new_body = String::with_capacity(body.len() + insertion.len());
        new_body.push_str(&body[..position]);
        new_body.push_str(&insertion);
        new_body.push_str(&body[position..]);
        return Some(WritePlan {
            new_body,
            op: WriteOp::EditSnippet,
        });
    }
    let (start, end) = locate_selection(body, sel)?;
    let regenerated_whole = (!sel.after.trim().is_empty() && content.contains(sel.after.trim()))
        || (!sel.before.trim().is_empty() && content.contains(sel.before.trim()));
    if regenerated_whole {
        return None;
    }
    let mut new_body = String::with_capacity(body.len() + content.len());
    new_body.push_str(&body[..start]);
    new_body.push_str(&content);
    new_body.push_str(&body[end..]);
    Some(WritePlan {
        new_body,
        op: WriteOp::EditSnippet,
    })
}

/// Small local models sometimes ignore the cursor-only instruction and return
/// the existing note followed by the requested addition. Strip that echoed
/// prefix before inserting so a write below a note cannot duplicate the note.
fn strip_cursor_echo(body: &str, position: usize, content: &str) -> String {
    let prefix = &body[..position];
    if prefix.is_empty() {
        return content.to_string();
    }

    let prefix_end = if content.starts_with(prefix) {
        Some(prefix.len())
    } else {
        find_tolerant(content, prefix)
            .filter(|(start, _)| *start == 0)
            .map(|(_, end)| end)
    };
    let Some(prefix_end) = prefix_end else {
        return content.to_string();
    };
    let remainder = &content[prefix_end..];
    let suffix = &body[position..];
    if suffix.is_empty() {
        return remainder.to_string();
    }
    if let Some(suffix_start) = remainder.find(suffix) {
        return remainder[..suffix_start].to_string();
    }
    content.to_string()
}

/// Locate a cursor against raw Markdown. The editor captures anchors from its
/// rendered text, so Markdown hard-break spaces and other formatting can make
/// an otherwise valid cursor differ from the source by whitespace. Prefer the
/// exact boundary, then use the shared whitespace-tolerant matcher while still
/// requiring the surrounding anchors to identify one unique position.
fn locate_cursor(body: &str, sel: &SelectionArg) -> Option<usize> {
    if let Some(offset) = sel.source_offset {
        if let Some(position) = utf16_to_byte_offset(body, offset) {
            let before_matches = sel.before.is_empty() || body[..position].ends_with(&sel.before);
            let after_matches = sel.after.is_empty() || body[position..].starts_with(&sel.after);
            if before_matches && after_matches {
                return Some(position);
            }
        }
    }
    let exact: Vec<usize> = (0..=body.len())
        .filter(|position| {
            body.is_char_boundary(*position)
                && (sel.before.is_empty() || body[..*position].ends_with(&sel.before))
                && (sel.after.is_empty() || body[*position..].starts_with(&sel.after))
        })
        .take(2)
        .collect();
    if exact.len() == 1 {
        return exact.first().copied();
    }
    if exact.len() > 1 {
        return None;
    }

    let before_matches = tolerant_matches(body, &sel.before);
    let after_matches = tolerant_matches(body, &sel.after);
    let before_norm = normalize_anchor(&sel.before);
    let after_norm = normalize_anchor(&sel.after);
    let mut candidates: Vec<(u8, usize)> = Vec::new();

    for (_, end) in before_matches {
        let mut position = end;
        if !sel.after.is_empty() && !body[position..].starts_with(&sel.after) {
            let whitespace = body[position..].len() - body[position..].trim_start().len();
            let aligned = position + whitespace;
            if normalized_starts_with(&body[position..], &after_norm) {
                position = aligned;
            }
        }
        let score = if sel.after.is_empty() {
            1
        } else if body[position..].starts_with(&sel.after) {
            3
        } else if normalized_starts_with(&body[position..], &after_norm) {
            2
        } else {
            0
        };
        if score > 0 {
            candidates.push((score, position));
        }
    }
    for (start, _) in after_matches {
        let position = start;
        let score = if sel.before.is_empty() {
            1
        } else if body[..position].ends_with(&sel.before) {
            3
        } else if normalized_ends_with(&body[..position], &before_norm) {
            2
        } else {
            0
        };
        if score > 0 {
            candidates.push((score, position));
        }
    }

    candidates.sort_unstable_by(|left, right| right.cmp(left));
    candidates.dedup_by_key(|(_, position)| *position);
    let Some((best_score, best_position)) = candidates.first().copied() else {
        if let Some(position) = unique_cursor_before(body, &sel.before) {
            return Some(position);
        }
        return unique_cursor_gap(body, &sel.before, &sel.after);
    };
    let tied = candidates
        .iter()
        .filter(|(score, _)| *score == best_score)
        .count();
    if tied == 1 {
        Some(best_position)
    } else {
        unique_cursor_before(body, &sel.before)
            .or_else(|| unique_cursor_gap(body, &sel.before, &sel.after))
    }
}

/// Convert a JavaScript string offset (UTF-16 code units) into a Rust UTF-8
/// byte offset without ever splitting a multibyte character.
fn utf16_to_byte_offset(value: &str, offset: usize) -> Option<usize> {
    let mut units = 0;
    for (byte, character) in value.char_indices() {
        if units == offset {
            return Some(byte);
        }
        units += character.len_utf16();
        if units > offset {
            return None;
        }
    }
    (units == offset).then_some(value.len())
}

fn unique_cursor_before(body: &str, before: &str) -> Option<usize> {
    let matches = cursor_anchor_matches(body, before, true);
    if matches.len() != 1 {
        return None;
    }
    let (_, end) = matches[0];
    Some(end + cursor_marker_prefix_len(&body[end..]))
}

/// Find the one source gap between the two rendered anchors. This is a final
/// fallback for Vditor IR boundaries where Markdown markers (for example
/// `**bold**`) disappear from the DOM and prevent either anchor from matching
/// as a normal whitespace-tolerant string.
fn unique_cursor_gap(body: &str, before: &str, after: &str) -> Option<usize> {
    let before_matches = cursor_anchor_matches(body, before, true);
    let after_matches = cursor_anchor_matches(body, after, false);
    if before_matches.is_empty() || after_matches.is_empty() {
        return None;
    }

    let mut gaps = Vec::new();
    for (_, end) in before_matches {
        for (start, _) in &after_matches {
            if end <= *start
                && *start - end <= 256
                && body[end..*start].chars().all(is_cursor_separator)
            {
                gaps.push((
                    *start - end,
                    end + cursor_marker_prefix_len(&body[end..*start]),
                ));
            }
        }
    }
    gaps.sort_unstable();
    gaps.dedup();
    match gaps.as_slice() {
        [(distance, position)] if *distance <= 256 => Some(*position),
        _ => None,
    }
}

fn cursor_marker_prefix_len(value: &str) -> usize {
    value
        .char_indices()
        .take_while(|(_, character)| !character.is_whitespace() && is_cursor_separator(*character))
        .map(|(index, character)| index + character.len_utf8())
        .last()
        .unwrap_or(0)
}

fn cursor_anchor_matches(body: &str, anchor: &str, from_end: bool) -> Vec<(usize, usize)> {
    let matches = tolerant_matches(body, anchor);
    if !matches.is_empty() {
        return matches;
    }

    // Use a short word-only anchor when the rendered text omitted Markdown
    // punctuation. It is deliberately short and still must form one unique
    // source gap with the opposite anchor before it can be accepted.
    let words = anchor
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>();
    if words.is_empty() {
        return Vec::new();
    }
    let selected = if from_end {
        let start = words.len().saturating_sub(8);
        &words[start..]
    } else {
        &words[..words.len().min(8)]
    };
    let pattern = selected
        .iter()
        .map(|word| regex::escape(word))
        .collect::<Vec<_>>()
        .join(r"[^[:alnum:]]+");
    let Ok(regex) = Regex::new(&pattern) else {
        return Vec::new();
    };
    regex
        .find_iter(body)
        .map(|matched| (matched.start(), matched.end()))
        .collect()
}

fn is_cursor_separator(value: char) -> bool {
    value.is_whitespace()
        || matches!(
            value,
            '*' | '_'
                | '`'
                | '#'
                | '>'
                | '~'
                | '['
                | ']'
                | '{'
                | '}'
                | '('
                | ')'
                | ':'
                | ';'
                | ','
                | '.'
                | '!'
                | '?'
                | '"'
                | '\''
                | '-'
                | '—'
                | '–'
                | '/'
        )
}

fn adjacent_visible_alphanumeric(value: &str, from_end: bool) -> bool {
    let characters: Box<dyn Iterator<Item = char>> = if from_end {
        Box::new(value.chars().rev())
    } else {
        Box::new(value.chars())
    };
    for character in characters {
        if character.is_alphanumeric() {
            return true;
        }
        if character.is_whitespace() {
            return false;
        }
        if !is_cursor_separator(character) {
            return false;
        }
    }
    false
}

fn tolerant_matches(body: &str, anchor: &str) -> Vec<(usize, usize)> {
    let anchor = anchor.trim();
    if anchor.is_empty() {
        return Vec::new();
    }
    let mut matches = Vec::new();
    let mut from = 0;
    while from <= body.len() {
        let Some((start, end)) = find_tolerant(&body[from..], anchor) else {
            break;
        };
        let start = from + start;
        let end = from + end;
        matches.push((start, end));
        from = end.max(start.saturating_add(1));
        while from < body.len() && !body.is_char_boundary(from) {
            from += 1;
        }
    }
    matches
}

fn normalize_anchor(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn normalized_starts_with(value: &str, anchor: &str) -> bool {
    !anchor.is_empty() && normalize_anchor(value).starts_with(anchor)
}

fn normalized_ends_with(value: &str, anchor: &str) -> bool {
    !anchor.is_empty() && normalize_anchor(value).ends_with(anchor)
}

/// Validate an insertion marker against the armed selection and insert only
/// immediately after the selected line.
pub fn selection_insert_after_plan(
    body: &str,
    marker: &str,
    content: &str,
    sel: &SelectionArg,
) -> Result<WritePlan, String> {
    let (start, end) = locate_selection(body, sel)
        .ok_or_else(|| "The armed selection is no longer present in the note.".to_string())?;
    let marker = marker.trim();
    let selected = &body[start..end];
    if marker.is_empty()
        || (marker != selected
            && !selected.contains(marker)
            && !marker.contains(selected)
            && !sel.text.trim().contains(marker))
    {
        return Err(
            "Rejected insertion marker: it does not match the armed selection.".to_string(),
        );
    }
    let line_end = body[end..]
        .find('\n')
        .map(|i| end + i + 1)
        .unwrap_or(body.len());
    let content = clean_note_content(&strip_prompt_markers(content));
    let new_body = format!("{}{}\n\n{}", &body[..line_end], content, &body[line_end..]);
    Ok(WritePlan {
        new_body,
        op: WriteOp::Append,
    })
}

/// Pure decision for `write_note`, `append_note` and the shared edit helpers:
/// `plan_write`, `apply_format_op`, `clean_note_content`,
/// `note_content_has_protocol_residue`, `normalize_append_content`,
/// `strip_prompt_markers`, `find_tolerant`, `FORMAT_OPS`, `is_format_op` and
/// the `WriteOp`/`WritePlan` types live in `myelin-edit-core` so the sidecar's
/// on-disk edits stay byte-identical to the app's.
pub use myelin_edit_core::{
    apply_format_op, clean_note_content, find_tolerant, is_format_op, normalize_append_content,
    note_content_has_protocol_residue, plan_write, strip_prompt_markers, WriteOp, WritePlan,
    FORMAT_OPS,
};
