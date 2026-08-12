use super::core::*;
use anyhow::{anyhow, Context, Result};
use arrow_array::{Array, FixedSizeListArray, Float32Array, Int64Array, StringArray};
use futures_util::TryStreamExt;
use lancedb::query::ExecutableQuery;

#[derive(Clone)]
pub(crate) struct WorkspaceSearchHit { pub(crate) note_id: String, pub(crate) title: String, pub(crate) text: String, pub(crate) section: Option<String>, pub(crate) char_start: Option<i64>, pub(crate) char_end: Option<i64>, pub(crate) embedding_fingerprint: String, pub(crate) vector: Option<Vec<f32>> }

async fn validate_live_workspace_table(table: &lancedb::Table) -> Result<()> {
    let actual = table.schema().await.context("failed to inspect workspace Arrow schema")?;
    let expected = [
        ("note_id", DataType::Utf8, false), ("title", DataType::Utf8, false),
        ("tags_text", DataType::Utf8, false), ("path", DataType::Utf8, false),
        ("updated_at", DataType::Utf8, false), ("chunk_index", DataType::Int32, false),
        ("text", DataType::Utf8, false), ("lexical_text", DataType::Utf8, false),
        ("token_count", DataType::Int32, false), ("char_start", DataType::Int64, true),
        ("char_end", DataType::Int64, true), ("section_start", DataType::Utf8, true),
        ("section_end", DataType::Utf8, true), ("embedding_fingerprint", DataType::Utf8, false),
    ];
    if actual.fields().len() != expected.len() + 1 { anyhow::bail!("workspace index schema has unexpected column count"); }
    for (field, (name, data_type, nullable)) in actual.fields().iter().zip(expected) {
        if field.name() != name || field.data_type() != &data_type || field.is_nullable() != nullable { anyhow::bail!("workspace index schema mismatch at column {name}"); }
    }
    let vector = actual.fields().last().context("workspace index is missing vector column")?;
    match vector.data_type() {
        DataType::FixedSizeList(item, width) if item.data_type() == &DataType::Float32 && *width > 0 && vector.is_nullable() => {}
        _ => anyhow::bail!("workspace vector column is not a nullable fixed Float32 vector"),
    }
    let indices = table.list_indices().await.context("failed to inspect workspace indexes")?;
    let has_fts = indices.iter().any(|index| index.columns.iter().any(|column| column == "lexical_text") && matches!(index.index_type, lancedb::index::IndexType::FTS));
    if !has_fts { table.create_index(&["lexical_text"], lancedb::index::Index::FTS(Default::default())).execute().await.context("failed to recreate workspace FTS index")?; }
    Ok(())
}

pub(crate) async fn workspace_chunks(state: &AppState) -> Result<Vec<WorkspaceSearchHit>> {
    let connection = open_database(&state.index_dir()).await?;
    let table = connection.open_table(TABLE_NAME).execute().await.context("workspace index is not initialized")?;
    validate_live_workspace_table(&table).await?;
    let mut stream = table.query().execute().await.context("workspace chunk scan failed")?;
    let mut out = Vec::new();
    while let Some(batch) = stream.try_next().await.context("workspace chunk stream failed")? {
        let strings = |name: &str| batch.column_by_name(name).and_then(|a| a.as_any().downcast_ref::<StringArray>());
        let starts = batch.column_by_name("char_start").and_then(|a| a.as_any().downcast_ref::<Int64Array>()); let ends = batch.column_by_name("char_end").and_then(|a| a.as_any().downcast_ref::<Int64Array>());
        let vectors = batch.column_by_name("vector").and_then(|a| a.as_any().downcast_ref::<FixedSizeListArray>());
        let ids = strings("note_id").ok_or_else(|| anyhow!("workspace index is missing note_id"))?; let titles = strings("title").ok_or_else(|| anyhow!("workspace index is missing title"))?; let texts = strings("text").ok_or_else(|| anyhow!("workspace index is missing text"))?; let sections = strings("section_start"); let fingerprints = strings("embedding_fingerprint").ok_or_else(|| anyhow!("workspace index is missing embedding fingerprint"))?;
        for i in 0..batch.num_rows() {
            let vector = vectors.and_then(|array| if array.is_null(i) { None } else { let values = array.value(i); values.as_any().downcast_ref::<Float32Array>().map(|values| (0..values.len()).map(|j| values.value(j)).collect()) });
            out.push(WorkspaceSearchHit { note_id: ids.value(i).into(), title: titles.value(i).into(), text: texts.value(i).into(), section: sections.and_then(|a| (!a.is_null(i)).then(|| a.value(i).into())), char_start: starts.and_then(|a| (!a.is_null(i)).then(|| a.value(i))), char_end: ends.and_then(|a| (!a.is_null(i)).then(|| a.value(i))), embedding_fingerprint: fingerprints.value(i).into(), vector });
        }
    }
    Ok(out)
}
