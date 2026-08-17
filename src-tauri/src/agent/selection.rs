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
    let content = strip_prompt_markers(content);
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
        let positions = (0..=body.len()).filter(|position| {
            body.is_char_boundary(*position)
                && (sel.before.is_empty() || body[..*position].ends_with(&sel.before))
                && (sel.after.is_empty() || body[*position..].starts_with(&sel.after))
        });
        let matches: Vec<usize> = positions.take(2).collect();
        if matches.len() != 1 {
            return None;
        }
        let position = matches[0];
        let mut insertion = content;
        let left_alnum = body[..position]
            .chars()
            .next_back()
            .is_some_and(char::is_alphanumeric);
        let right_alnum = body[position..]
            .chars()
            .next()
            .is_some_and(char::is_alphanumeric);
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
