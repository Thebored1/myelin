//! Shared construction of the note-scoped system-prefix payload.
//!
//! Keeping this pure and used by both warm-up and real chat prevents a subtle
//! cache miss where the synthetic warm request differs from the first turn.

const OUTLINE_CHAR_LIMIT: usize = 8_000;
const OUTLINE_ENTRY_LIMIT: usize = 200;
const ORIENTATION_CHAR_LIMIT: usize = 4_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteSection {
    pub key: String,
    pub label: String,
    pub body: String,
}

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

/// Split a textual document into stable, independently cacheable sections.
/// Headed formats use their native boundaries; unheaded or very large regions
/// fall back to the same conservative word chunker used by RAG.
pub fn sections(body: &str, relative_path: &str) -> Vec<NoteSection> {
    let lower = relative_path.to_ascii_lowercase();
    if lower.ends_with(".ipynb") {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(body) {
            if let Some(cells) = value.get("cells").and_then(|v| v.as_array()) {
                return cells
                    .iter()
                    .enumerate()
                    .filter_map(|(index, cell)| {
                        let kind = cell.get("cell_type").and_then(|v| v.as_str()).unwrap_or("cell");
                        let text = cell
                            .get("source")
                            .and_then(|v| v.as_array())
                            .map(|lines| lines.iter().filter_map(|v| v.as_str()).collect::<String>())
                            .unwrap_or_default();
                        (!text.trim().is_empty()).then(|| NoteSection {
                            key: format!("cell:{}", index + 1),
                            label: format!("Cell {} ({kind})", index + 1),
                            body: text,
                        })
                    })
                    .collect();
            }
        }
    }

    let heading = if lower.ends_with(".tex") {
        |line: &str| {
            ["section", "subsection", "subsubsection"]
                .iter()
                .find_map(|command| {
                    let marker = format!("\\{command}{{");
                    line.trim().strip_prefix(&marker).and_then(|rest| {
                        rest.find('}').map(|end| (format!("{command}: {}", &rest[..end]), *command))
                    })
                })
        }
    } else {
        |line: &str| {
            let trimmed = line.trim();
            let hashes = trimmed.chars().take_while(|c| *c == '#').count();
            (1..=6)
                .contains(&hashes)
                .then(|| (trimmed[hashes..].trim().to_string(), "heading"))
        }
    };

    let mut headed = Vec::new();
    let mut current: Option<(String, String, String)> = None;
    for line in body.lines() {
        if let Some((label, kind)) = heading(line) {
            if let Some((key, label, text)) = current.take() {
                if !text.trim().is_empty() {
                    headed.push(NoteSection { key, label, body: text });
                }
            }
            let index = headed.len() + 1;
            current = Some((format!("{kind}:{index}"), label, String::new()));
        } else if let Some((_, _, text)) = current.as_mut() {
            text.push_str(line);
            text.push('\n');
        }
    }
    if let Some((key, label, text)) = current {
        if !text.trim().is_empty() {
            headed.push(NoteSection { key, label, body: text });
        }
    }
    if !headed.is_empty() {
        return headed;
    }

    crate::embeddings::chunk_text(body, 192, 32)
        .into_iter()
        .map(|chunk| NoteSection {
            key: format!("chunk:{}", chunk.index),
            label: format!("Section {}", chunk.index + 1),
            body: chunk.text,
        })
        .collect()
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

    #[test]
    fn sections_follow_markdown_headings_and_fallback_to_chunks() {
        let headed = sections("# Intro\nalpha\n## Detail\nbeta\n", "a.md");
        assert_eq!(headed.len(), 2);
        assert_eq!(headed[0].key, "heading:1");
        assert!(headed[0].body.contains("alpha"));
        assert!(headed[1].body.contains("beta"));

        let plain = sections(&(0..500).map(|i| format!("word{i}")).collect::<Vec<_>>().join(" "), "a.md");
        assert!(plain.len() > 1);
        assert_eq!(plain[0].key, "chunk:0");
    }

    #[test]
    fn sections_use_tex_commands_and_notebook_cells() {
        let tex = sections("\\section{Intro}\nalpha\n\\subsection{Detail}\nbeta", "a.tex");
        assert_eq!(tex[0].label, "section: Intro");
        let notebook = serde_json::json!({
            "cells": [
                {"cell_type": "markdown", "source": ["# Intro\n"]},
                {"cell_type": "code", "source": ["print(1)\n"]}
            ]
        });
        let cells = sections(&notebook.to_string(), "a.ipynb");
        assert_eq!(cells.iter().map(|cell| cell.key.as_str()).collect::<Vec<_>>(), ["cell:1", "cell:2"]);
    }
}
