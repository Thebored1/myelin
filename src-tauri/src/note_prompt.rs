//! Shared construction of the note-scoped system-prefix payload.
//!
//! Keeping this pure and used by both warm-up and real chat prevents a subtle
//! cache miss where the synthetic warm request differs from the first turn.

const OUTLINE_CHAR_LIMIT: usize = 8_000;
const OUTLINE_ENTRY_LIMIT: usize = 200;
const ORIENTATION_CHAR_LIMIT: usize = 4_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotePromptShape {
    pub oversized: bool,
    pub char_count: usize,
    pub estimated_tokens: usize,
    pub body: String,
}

impl NotePromptShape {
    pub fn build(body: &str, relative_path: &str, actual_ctx_tokens: usize) -> Self {
        let char_count = body.chars().count();
        let limit = actual_ctx_tokens.saturating_mul(2).clamp(4_000, 400_000);
        if char_count <= limit {
            return Self {
                oversized: false,
                char_count,
                estimated_tokens: char_count.div_ceil(4),
                body: body.to_string(),
            };
        }

        let outline = outline(body, relative_path);
        let orientation: String = body.chars().take(ORIENTATION_CHAR_LIMIT).collect();
        let body = format!(
            "[OVERSIZED NOTE — retrieval-backed]\n\
             Exact length: {char_count} characters\n\
             Estimated length: approximately {} tokens\n\n\
             STRUCTURAL OUTLINE:\n{outline}\n\n\
             ORIENTATION EXCERPT (first {ORIENTATION_CHAR_LIMIT} characters):\n\
             {orientation}\n\n\
             The complete note is indexed. Before editing a section, use \
             search_documents scoped to this note to retrieve the relevant \
             passages. Make only targeted edits; whole-note rewrites and \
             whole-note formatting are forbidden.",
            char_count.div_ceil(4)
        );
        Self {
            oversized: true,
            char_count,
            estimated_tokens: char_count.div_ceil(4),
            body,
        }
    }
}

fn push_entry(entries: &mut Vec<String>, entry: String, chars: &mut usize) {
    if entries.len() >= OUTLINE_ENTRY_LIMIT || *chars >= OUTLINE_CHAR_LIMIT {
        return;
    }
    let remaining = OUTLINE_CHAR_LIMIT - *chars;
    let entry: String = entry.chars().take(remaining).collect();
    *chars += entry.chars().count() + 1;
    entries.push(entry);
}

fn outline(body: &str, relative_path: &str) -> String {
    let lower = relative_path.to_ascii_lowercase();
    let mut entries = Vec::new();
    let mut chars = 0;
    if lower.ends_with(".ipynb") {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(body) {
            if let Some(cells) = value.get("cells").and_then(|v| v.as_array()) {
                for (index, cell) in cells.iter().enumerate() {
                    let kind = cell.get("cell_type").and_then(|v| v.as_str()).unwrap_or("cell");
                    let source = cell.get("source").and_then(|v| v.as_array())
                        .map(|lines| lines.iter().filter_map(|v| v.as_str()).collect::<String>())
                        .unwrap_or_default();
                    let label = source.lines().map(str::trim)
                        .find(|line| !line.is_empty()).unwrap_or("(empty)");
                    push_entry(&mut entries, format!("- Cell {} ({kind}): {}", index + 1, label.chars().take(160).collect::<String>()), &mut chars);
                }
            }
        }
    } else if lower.ends_with(".tex") {
        for line in body.lines() {
            let trimmed = line.trim();
            for command in ["section", "subsection", "subsubsection"] {
                let marker = format!("\\{command}{{");
                if let Some(rest) = trimmed.strip_prefix(&marker) {
                    if let Some((title, _)) = rest.split_once('}') {
                        push_entry(&mut entries, format!("- {command}: {title}"), &mut chars);
                    }
                }
            }
        }
    } else {
        for line in body.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("# ") || trimmed.starts_with("## ") {
                push_entry(&mut entries, format!("- {trimmed}"), &mut chars);
            }
        }
    }
    if entries.is_empty() { "(no structural headings found)".into() } else { entries.join("\n") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_outline_and_oversized_instructions() {
        let body = format!("# One\n{}\n## Two\n", "x".repeat(5000));
        let shaped = NotePromptShape::build(&body, "a.md", 2000);
        assert!(shaped.oversized);
        assert!(shaped.body.contains("- # One"));
        assert!(shaped.body.contains("- ## Two"));
        assert!(shaped.body.contains("whole-note rewrites"));
    }

    #[test]
    fn latex_and_notebook_outlines() {
        let tex = format!("\\section{{Intro}}\n{}", "x".repeat(5000));
        assert!(NotePromptShape::build(&tex, "a.tex", 2000).body.contains("section: Intro"));
        let nb = serde_json::json!({"cells":[{"cell_type":"markdown","source":["# Heading\n"]}]}).to_string()
            + &" ".repeat(5000);
        assert!(NotePromptShape::build(&nb, "a.ipynb", 2000).body.contains("Cell 1 (markdown)"));
    }

    #[test]
    fn ordinary_note_is_byte_identical() {
        let body = "hello 👋";
        assert_eq!(NotePromptShape::build(body, "a.md", 4096).body, body);
    }
}
