//! Deterministic note-edit logic shared by the Myelin Tauri app and the
//! openharn-myelin sidecar. Everything here is pure (no `AppState`, no Tauri),
//! so the two hosts can run the same byte-for-byte `plan_write` /
//! `apply_format_op` decisions and their e2e test stays honest.

use regex::Regex;

/// How the editor should apply a write (drives the streaming UI and chip label).
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum WriteOp {
    Replace,
    Append,
    EditSnippet,
}

#[derive(Debug)]
pub struct WritePlan {
    pub new_body: String,
    pub op: WriteOp,
}

/// Locate a snippet to edit, tolerating the small mismatches a model makes when
/// it reproduces existing text: try an exact match, then a trimmed match, then a
/// whitespace-normalized match (the snippet's words separated by any run of
/// whitespace). Returns the byte span in `body` to replace.
pub fn find_tolerant(body: &str, find: &str) -> Option<(usize, usize)> {
    if let Some(i) = body.find(find) {
        return Some((i, i + find.len()));
    }
    let trimmed = find.trim();
    if !trimmed.is_empty() && trimmed.len() != find.len() {
        if let Some(i) = body.find(trimmed) {
            return Some((i, i + trimmed.len()));
        }
    }
    let tokens: Vec<&str> = trimmed.split_whitespace().collect();
    if tokens.is_empty() {
        return None;
    }
    let pattern = tokens
        .iter()
        .map(|t| regex::escape(t))
        .collect::<Vec<_>>()
        .join(r"\s+");
    let re = Regex::new(&pattern).ok()?;
    re.find(body).map(|m| (m.start(), m.end()))
}

/// Remove the prompt's note-framing markers that some models echo into content.
/// Tolerant of the dash-count / spacing variants models produce (e.g. some
/// models emit "--- END CURRENT NOTE --" with two trailing dashes).
pub fn strip_prompt_markers(s: &str) -> String {
    let mut cleaned = Regex::new(r"(?i)-{2,}\s*(?:end\s+)?current note\s*-*")
        .map(|re| re.replace_all(s, "").into_owned())
        .unwrap_or_else(|_| s.to_string());
    // The model sometimes bleeds the "--- CURRENT NOTE ---" delimiter style into
    // its output as a leading "--- Title" — dashes + text on one line, which is
    // not a real Markdown rule (an HR is dashes alone). Drop the leading dash run
    // but keep the text. (Rust's regex has no lookahead, so capture and re-emit.)
    if let Ok(re) = Regex::new(r"^\s*-{2,}[ \t]+(\S[^\n]*)") {
        cleaned = re.replace(&cleaned, "$1").into_owned();
    }
    cleaned.trim().to_string()
}

/// Remove a model-regenerated copy of the current note from an append payload.
/// Weak models often send the complete note followed by the requested addition
/// despite declaring `mode:"append"`; appending that verbatim duplicates user
/// content. Tool-call wrapper remnants are never valid note text either.
pub fn normalize_append_content(current_body: &str, content: &str) -> String {
    let mut payload = content.trim().to_string();
    for marker in ["</content>,", "</content>", "</tool_call>"] {
        if let Some(stripped) = payload.trim_end().strip_suffix(marker) {
            payload = stripped.trim_end().to_string();
        }
    }

    let current = current_body.trim();
    if !current.is_empty() {
        if let Some(rest) = payload.strip_prefix(current) {
            payload = rest.trim_start_matches(['\r', '\n', ' ', '\t']).to_string();
        } else if let Some(pos) = payload.find(current) {
            // Only strip an echoed note near the start; a later quoted copy may
            // be intentional context in the new paragraph.
            if pos <= 64 {
                payload = payload[pos + current.len()..]
                    .trim_start_matches(['\r', '\n', ' ', '\t'])
                    .to_string();
            }
        }
    }
    payload.trim().to_string()
}

/// Reject leaked model/tool framing instead of silently cleaning and saving a
/// guessed note body. This is a final host-side guard for both the sidecar and
/// the in-process host path.
pub fn note_content_has_protocol_residue(content: &str) -> bool {
    let tail: String = content
        .chars()
        .rev()
        .take(256)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    let tail = tail.to_ascii_lowercase();
    [
        "<|tool_call",
        "<|tool_call_end|>",
        "<tool_call",
        "</tool_call",
        "/content>}",
        "</content>",
        "> write_note(content=",
        "] write_note(content=",
    ]
    .iter()
    .any(|marker| tail.contains(marker))
}

/// Strip HTML line-break tags and model-invented markup artifacts from model-
/// generated note content. Many small models output `<br>` for line breaks or
/// `<<`/`<>` as formatting markers — these are never valid note text.
pub fn clean_note_content(content: &str) -> String {
    let mut s = content.to_string();
    // A few prompt/tool parsers leave an escaped newline literal behind.
    s = s
        .replace("\\r\\n", "\n")
        .replace("\\n", "\n")
        .replace("\r\n", "\n")
        .replace('\r', "\n");
    // Replace <br> (and case/spacing variants) with real newlines.
    if let Ok(br) = Regex::new(r"(?i)<br\s*/?>") {
        s = br.replace_all(&s, "\n").into_owned();
    }
    // Some small models use an em-space as a visual line separator.
    s = s.replace(' ', "\n");
    // Strip trailing <<, <>, >, < markers that some models use as stanza
    // connectors — they appear at line-ends or between phrases.
    if let Ok(re) = Regex::new(r"<{1,2}\s*>*\s*|\s*<{1,2}\s*") {
        // Only strip standalone bracket runs (not inside **bold** or normal text).
        // A standlone <, <<, <>, >> preceded by a word boundary or punctuation.
        if let Ok(standalone) = Regex::new(r"(?:\b|,|;|!|\?|—)\s*<{1,2}\s*>?(?:\s*<{1,2}\s*>?)*") {
            s = standalone.replace_all(&s, "\n").into_owned();
        }
        // Also strip leading/trailing bare bracket runs that survive.
        s = re.replace_all(&s, "").into_owned();
    }
    // A malformed Markdown pattern seen in tool arguments is:
    // `**line one** *line two.* *line three.*`. When it repeats across a
    // single-line payload, the asterisks are acting as line separators rather
    // than Markdown. Convert only that repeated pattern; ordinary emphasis
    // remains untouched.
    if !s.contains('\n') {
        if let Ok(separator) = Regex::new(r"\s+\*+") {
            let parts: Vec<&str> = separator.split(&s).collect();
            let starred_tails = parts
                .iter()
                .skip(1)
                .filter(|part| part.trim_end().ends_with('*'))
                .count();
            if parts.len() >= 4 && starred_tails + 1 >= parts.len() - 1 {
                let mut lines = Vec::with_capacity(parts.len());
                for (index, part) in parts.iter().enumerate() {
                    let mut line = part.trim().to_string();
                    if index > 0 {
                        line = line.trim_end_matches('*').trim_end().to_string();
                    }
                    if !line.is_empty() {
                        lines.push(line);
                    }
                }
                s = lines.join("\n");
            }
        }
    }

    // Collapse multiple consecutive blank lines into one.
    if let Ok(blank) = Regex::new(r"\n{3,}") {
        s = blank.replace_all(&s, "\n\n").into_owned();
    }
    s.trim().to_string()
}

/// `mode` is passed raw ("" when unspecified) so an explicit "replace" can be
/// told apart from the default. Kept free of `AppState`/Tauri for unit tests.
pub fn plan_write(
    current_body: &str,
    content: &str,
    mode: &str,
    find: &str,
) -> Result<WritePlan, String> {
    // Some models echo the prompt's note-framing markers into the tool content.
    // Strip them so they never land in the saved note. This runs before the
    // intent logic so the absorb-check can still clean an edit.
    let content = strip_prompt_markers(content);
    let content = content.as_str();
    let m = mode.trim().to_lowercase();
    let has_find = !find.trim().is_empty();
    let is_append = m == "append";
    let explicit_replace = m == "replace";
    // A targeted edit only when a `find` is given and the model did NOT
    // explicitly ask for a whole-body replace/append.
    let snippet = has_find && !explicit_replace && !is_append;

    if snippet {
        match find_tolerant(current_body, find) {
            Some((start, end)) => {
                let prefix = &current_body[..start];
                let suffix = &current_body[end..];
                // If `content` already contains the surrounding text (so splicing
                // would duplicate it), the model actually sent the whole updated
                // body, not a snippet replacement — treat it as a replace. Catches
                // e.g. find:"blue", content:"The sky is green today." on a note of
                // "The sky is blue today." (which would otherwise garble).
                let absorbs = (!prefix.trim().is_empty() && content.starts_with(prefix))
                    || (!suffix.trim().is_empty() && content.ends_with(suffix));
                if absorbs {
                    return Ok(WritePlan { new_body: content.to_string(), op: WriteOp::Replace });
                }
                let mut body = String::with_capacity(current_body.len() + content.len());
                body.push_str(prefix);
                body.push_str(content);
                body.push_str(suffix);
                Ok(WritePlan { new_body: body, op: WriteOp::EditSnippet })
            }
            None => Err("Could not find the `find` text in the note. Retry with mode \"replace\" and send the COMPLETE updated note as `content`.".to_string()),
        }
    } else if is_append {
        let content = normalize_append_content(current_body, content);
        let body = if current_body.trim().is_empty() {
            content
        } else {
            format!("{}\n\n{}", current_body.trim_end(), content)
        };
        Ok(WritePlan {
            new_body: body,
            op: WriteOp::Append,
        })
    } else {
        // Whole-body replace: explicit replace (find ignored), mode:"edit" with
        // no find, or unspecified mode.
        Ok(WritePlan {
            new_body: content.to_string(),
            op: WriteOp::Replace,
        })
    }
}

/// Every operation `apply_format_op` understands. Kept in sync with the
/// format_note tool's `operation` enum and used to validate the model's choice.
pub const FORMAT_OPS: &[&str] = &[
    "remove_headings",
    "remove_bold",
    "remove_italic",
    "remove_emphasis",
    "remove_bullets",
    "remove_numbering",
    "remove_links",
    "remove_images",
    "remove_code",
    "remove_blockquotes",
    "remove_strikethrough",
    "remove_horizontal_rules",
    "remove_blank_lines",
    "strip_markdown",
    "headings_to_bold",
    "bold_to_headings",
    "promote_headings",
    "demote_headings",
    "bullets_to_numbered",
    "numbered_to_bullets",
    "tasks_to_bullets",
    "uppercase",
    "lowercase",
    "title_case",
];

pub fn is_format_op(op: &str) -> bool {
    FORMAT_OPS.contains(&op)
}

/// Strip bold/italic emphasis. Bold uses doubled markers (** or __), italic
/// single (* or _). Bold is processed first; when only italic is being removed,
/// the doubled markers are protected so the single-marker pass can't chew them
/// (Rust regex has no lookaround).
fn strip_emphasis(body: &str, bold: bool, italic: bool) -> String {
    let re = |p: &str| Regex::new(p).unwrap();
    let mut s = body.to_string();
    if bold {
        s = re(r"\*\*(.+?)\*\*").replace_all(&s, "$1").into_owned();
        s = re(r"__(.+?)__").replace_all(&s, "$1").into_owned();
    }
    if italic {
        let protect = !bold;
        if protect {
            s = s.replace("**", "\u{1}B").replace("__", "\u{1}U");
        }
        s = re(r"\*(.+?)\*").replace_all(&s, "$1").into_owned();
        s = re(r"(?:^|\b)_(.+?)_(?:\b|$)")
            .replace_all(&s, "$1")
            .into_owned();
        if protect {
            s = s.replace("\u{1}B", "**").replace("\u{1}U", "__");
        }
    }
    s
}

/// Renumber/convert list-item markers. `to` = "number" (1. 2. … reset per block)
/// or "bullet" (- ). Operates on a contiguous run of list lines.
fn convert_lists(body: &str, to: &str) -> String {
    let bullet = Regex::new(r"^(\s*)[-*+][ \t]+").unwrap();
    let numbered = Regex::new(r"^(\s*)\d+[.)][ \t]+").unwrap();
    let mut out: Vec<String> = Vec::new();
    let mut counter = 0u32;
    for line in body.split('\n') {
        let b = bullet.captures(line);
        let n = numbered.captures(line);
        let caps = b.as_ref().or(n.as_ref());
        match caps {
            Some(c) => {
                counter += 1;
                let whole = c.get(0).unwrap();
                let indent = c.get(1).map(|m| m.as_str()).unwrap_or("");
                let rest = &line[whole.end()..];
                if to == "number" {
                    out.push(format!("{indent}{counter}. {rest}"));
                } else {
                    out.push(format!("{indent}- {rest}"));
                }
            }
            None => {
                counter = 0;
                out.push(line.to_string());
            }
        }
    }
    out.join("\n")
}

fn to_title_case(body: &str) -> String {
    body.split('\n')
        .map(|line| {
            line.split(' ')
                .map(|word| {
                    let mut chars = word.chars();
                    match chars.next() {
                        Some(c) => {
                            c.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase()
                        }
                        None => String::new(),
                    }
                })
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Apply a deterministic Markdown transform. Done in code precisely so the
/// model never has to (and never gets to wipe or echo the note).
pub fn apply_format_op(body: &str, op: &str) -> String {
    let re = |p: &str| Regex::new(p).unwrap();
    match op {
        // ---- removals ----
        "remove_headings" => re(r"(?m)^[ \t]{0,3}#{1,6}[ \t]+")
            .replace_all(body, "")
            .into_owned(),
        "remove_bold" => strip_emphasis(body, true, false),
        "remove_italic" => strip_emphasis(body, false, true),
        "remove_emphasis" => strip_emphasis(body, true, true),
        "remove_bullets" => re(r"(?m)^([ \t]*)[-*+][ \t]+")
            .replace_all(body, "$1")
            .into_owned(),
        "remove_numbering" => re(r"(?m)^([ \t]*)\d+[.)][ \t]+")
            .replace_all(body, "$1")
            .into_owned(),
        // Keep link/alt text; drop the (url). Protect images during the link pass.
        "remove_links" => {
            let protected = body.replace("![", "\u{1}I");
            let unlinked = re(r"\[([^\]]*)\]\([^)]*\)")
                .replace_all(&protected, "$1")
                .into_owned();
            unlinked.replace("\u{1}I", "![")
        }
        "remove_images" => re(r"!\[[^\]]*\]\([^)]*\)")
            .replace_all(body, "")
            .into_owned(),
        // Fenced blocks first (keep inner code), then inline spans.
        "remove_code" => {
            let no_fence = re(r"(?s)```[^\n]*\n(.*?)```")
                .replace_all(body, "$1")
                .into_owned();
            re(r"`([^`\n]+)`").replace_all(&no_fence, "$1").into_owned()
        }
        "remove_blockquotes" => re(r"(?m)^[ \t]{0,3}>[ \t]?")
            .replace_all(body, "")
            .into_owned(),
        "remove_strikethrough" => re(r"~~(.+?)~~").replace_all(body, "$1").into_owned(),
        "remove_horizontal_rules" => re(r"(?m)^[ \t]{0,3}(?:-{3,}|\*{3,}|_{3,})[ \t]*\n?")
            .replace_all(body, "")
            .into_owned(),
        "remove_blank_lines" => re(r"\n{3,}").replace_all(body, "\n\n").into_owned(),
        // Everything → plain text, in a safe order.
        "strip_markdown" => {
            let mut s = apply_format_op(body, "remove_code");
            s = apply_format_op(&s, "remove_images");
            s = apply_format_op(&s, "remove_links");
            s = apply_format_op(&s, "remove_headings");
            s = apply_format_op(&s, "remove_blockquotes");
            s = apply_format_op(&s, "remove_horizontal_rules");
            s = apply_format_op(&s, "remove_bullets");
            s = apply_format_op(&s, "remove_numbering");
            s = apply_format_op(&s, "remove_strikethrough");
            strip_emphasis(&s, true, true)
        }
        // ---- conversions ----
        "headings_to_bold" => re(r"(?m)^[ \t]{0,3}#{1,6}[ \t]+(.+?)[ \t]*$")
            .replace_all(body, "**$1**")
            .into_owned(),
        "bold_to_headings" => re(r"(?m)^[ \t]*\*\*(.+?)\*\*[ \t]*$")
            .replace_all(body, "# $1")
            .into_owned(),
        // ##→# (one level up); h1 has no second # so is left alone.
        "promote_headings" => re(r"(?m)^#(#+[ \t])").replace_all(body, "$1").into_owned(),
        // #→## (one level down); h6 won't match so is capped.
        "demote_headings" => re(r"(?m)^(#{1,5})([ \t])")
            .replace_all(body, "#$1$2")
            .into_owned(),
        "bullets_to_numbered" => convert_lists(body, "number"),
        "numbered_to_bullets" => convert_lists(body, "bullet"),
        "tasks_to_bullets" => re(r"(?m)^([ \t]*)[-*+][ \t]+\[[ xX]\][ \t]+")
            .replace_all(body, "$1- ")
            .into_owned(),
        "uppercase" => body.to_uppercase(),
        "lowercase" => body.to_lowercase(),
        "title_case" => to_title_case(body),
        _ => body.to_string(),
    }
}
