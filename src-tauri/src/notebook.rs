//! Minimal Jupyter notebook (.ipynb) cell operations for AI editing. The body is
//! JSON, so we never let the model rewrite it raw: we present the cells as
//! readable text, and apply cell-scoped edits in place (parse → mutate one cell →
//! re-serialize), leaving every other cell's source, outputs and metadata
//! byte-identical. That makes a notebook edit impossible to corrupt.

use serde_json::{json, Value};

/// A cell's `source` can be a single string or an array of line strings on disk;
/// read either as one string.
fn cell_source(cell: &Value) -> String {
    match &cell["source"] {
        Value::String(s) => s.clone(),
        Value::Array(lines) => lines.iter().filter_map(|l| l.as_str()).collect(),
        _ => String::new(),
    }
}

/// Split `content` into the conventional ipynb `source` array (each line keeps its
/// trailing newline except the last).
fn to_source_array(content: &str) -> Value {
    if content.is_empty() {
        return json!([]);
    }
    let parts: Vec<&str> = content.split('\n').collect();
    let mut lines: Vec<String> = Vec::with_capacity(parts.len());
    for (i, p) in parts.iter().enumerate() {
        if i + 1 < parts.len() {
            lines.push(format!("{p}\n"));
        } else if !p.is_empty() {
            lines.push((*p).to_string());
        }
    }
    json!(lines)
}

/// Render the notebook as a readable, 0-indexed cell list for the prompt. None if
/// the body isn't a parseable notebook.
pub fn present(body: &str) -> Option<String> {
    let nb: Value = serde_json::from_str(body).ok()?;
    let cells = nb["cells"].as_array()?;
    let mut out = format!(
        "This open document is a Jupyter notebook with {} cell(s), numbered from 0. \
         Edit it ONLY with the edit_notebook tool (operation \"edit\" to replace a cell's \
         source, \"insert\" to add a cell before `index`, \"delete\" to remove one). Markdown \
         cells use Markdown; code cells use Python. Do NOT use write_note on a notebook.\n",
        cells.len()
    );
    for (i, cell) in cells.iter().enumerate() {
        let kind = cell["cell_type"].as_str().unwrap_or("code");
        out.push_str(&format!("\n[cell {i} · {kind}]\n"));
        out.push_str(&cell_source(cell));
        if !out.ends_with('\n') {
            out.push('\n');
        }
    }
    Some(out)
}

/// A single cell operation requested by the model.
#[derive(Debug)]
pub struct NotebookOp<'a> {
    pub operation: &'a str, // "edit" | "insert" | "delete"
    pub index: usize,
    pub cell_type: &'a str, // "code" | "markdown" (insert only; edit keeps existing)
    pub content: &'a str,
}

/// Apply a cell op to the notebook JSON, returning the new pretty-printed JSON.
/// Every untouched cell (and its outputs/metadata) is preserved exactly. Errors
/// are returned as human messages to relay back to the model.
pub fn apply(body: &str, op: &NotebookOp) -> Result<String, String> {
    let mut nb: Value =
        serde_json::from_str(body).map_err(|e| format!("The notebook isn't valid JSON: {e}"))?;
    let cells = nb["cells"]
        .as_array_mut()
        .ok_or_else(|| "This notebook has no cells array.".to_string())?;
    let count = cells.len();
    match op.operation {
        "edit" => {
            let cell = cells.get_mut(op.index).ok_or_else(|| {
                format!("No cell at index {} (the notebook has {count}).", op.index)
            })?;
            cell["source"] = to_source_array(op.content);
            // A code cell's outputs no longer match its new source.
            if cell["cell_type"] == json!("code") {
                cell["outputs"] = json!([]);
                cell["execution_count"] = Value::Null;
            }
        }
        "insert" => {
            let new_cell = if op.cell_type == "markdown" {
                json!({"cell_type":"markdown","metadata":{},"source": to_source_array(op.content)})
            } else {
                json!({"cell_type":"code","metadata":{},"execution_count":null,"outputs":[],"source": to_source_array(op.content)})
            };
            cells.insert(op.index.min(count), new_cell);
        }
        "delete" => {
            if op.index >= count {
                return Err(format!("No cell at index {} to delete.", op.index));
            }
            cells.remove(op.index);
        }
        other => {
            return Err(format!(
                "Unknown notebook operation '{other}'. Use \"edit\", \"insert\", or \"delete\"."
            ));
        }
    }
    serde_json::to_string_pretty(&nb).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const NB: &str = r##"{"cells":[
        {"cell_type":"markdown","metadata":{},"source":["# Title\n","intro"]},
        {"cell_type":"code","metadata":{},"execution_count":3,"outputs":[{"x":1}],"source":["print(1)\n"]}
    ],"metadata":{"kernelspec":{"name":"python3"}},"nbformat":4,"nbformat_minor":5}"##;

    #[test]
    fn present_lists_cells_with_indices() {
        let s = present(NB).unwrap();
        assert!(s.contains("[cell 0 · markdown]"));
        assert!(s.contains("# Title"));
        assert!(s.contains("[cell 1 · code]"));
        assert!(s.contains("print(1)"));
    }

    #[test]
    fn edit_replaces_source_and_preserves_other_cells() {
        let out = apply(
            NB,
            &NotebookOp { operation: "edit", index: 0, cell_type: "", content: "# New title" },
        )
        .unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(cell_source(&v["cells"][0]), "# New title");
        // Cell 1 (code) untouched, incl. its outputs + execution_count + metadata.
        assert_eq!(v["cells"][1]["execution_count"], json!(3));
        assert_eq!(v["cells"][1]["outputs"], json!([{"x":1}]));
        assert_eq!(v["metadata"]["kernelspec"]["name"], json!("python3"));
        assert_eq!(v["nbformat"], json!(4));
    }

    #[test]
    fn editing_a_code_cell_clears_its_stale_outputs() {
        let out = apply(
            NB,
            &NotebookOp { operation: "edit", index: 1, cell_type: "", content: "print(2)" },
        )
        .unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["cells"][1]["outputs"], json!([]));
        assert_eq!(v["cells"][1]["execution_count"], Value::Null);
    }

    #[test]
    fn insert_and_delete_shift_cells() {
        let out = apply(
            NB,
            &NotebookOp { operation: "insert", index: 1, cell_type: "code", content: "y = 2" },
        )
        .unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["cells"].as_array().unwrap().len(), 3);
        assert_eq!(cell_source(&v["cells"][1]), "y = 2");
        assert_eq!(v["cells"][1]["cell_type"], json!("code"));

        let out2 = apply(
            NB,
            &NotebookOp { operation: "delete", index: 0, cell_type: "", content: "" },
        )
        .unwrap();
        let v2: Value = serde_json::from_str(&out2).unwrap();
        assert_eq!(v2["cells"].as_array().unwrap().len(), 1);
        assert_eq!(cell_source(&v2["cells"][0]), "print(1)\n");
    }

    #[test]
    fn bad_index_returns_message_not_panic() {
        assert!(apply(NB, &NotebookOp { operation: "edit", index: 9, cell_type: "", content: "x" }).is_err());
        assert!(apply("not json", &NotebookOp { operation: "edit", index: 0, cell_type: "", content: "x" }).is_err());
    }
}
