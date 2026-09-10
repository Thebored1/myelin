use anyhow::{Context, Result};
use arrow_array::{Array, FixedSizeListArray, Float32Array, Int32Array, Int64Array, StringArray};
use futures_util::TryStreamExt;
use lancedb::{
    connect,
    query::{ExecutableQuery, QueryBase},
};
use std::path::Path;

/// Copy already-embedded chunks to a new attachment identity without invoking
/// the embedding server again. The source PDF bytes are identical after the
/// attachment copy, so its chunk vectors and tokenizer metadata remain valid.
pub(crate) async fn clone_document(
    index_dir: &Path,
    source_id: &str,
    target_id: &str,
) -> Result<bool> {
    if source_id == target_id {
        return Ok(true);
    }
    let conn = connect(index_dir.to_string_lossy().as_ref())
        .execute()
        .await
        .context("failed to open rag db for attachment reuse")?;
    let table = match conn.open_table("doc_chunks").execute().await {
        Ok(table) => table,
        Err(_) => return Ok(false),
    };
    let escaped = source_id.replace('\'', "''");
    let mut stream = table
        .query()
        .only_if(format!("doc_id = '{escaped}'"))
        .execute()
        .await
        .context("failed to read cached attachment chunks")?;
    let mut chunks = Vec::new();
    while let Some(batch) = stream
        .try_next()
        .await
        .context("failed to read cached attachment batch")?
    {
        let strings = |name: &str| {
            batch
                .column_by_name(name)
                .and_then(|column| column.as_any().downcast_ref::<StringArray>())
        };
        let ints = |name: &str| {
            batch
                .column_by_name(name)
                .and_then(|column| column.as_any().downcast_ref::<Int32Array>())
        };
        let longs = |name: &str| {
            batch
                .column_by_name(name)
                .and_then(|column| column.as_any().downcast_ref::<Int64Array>())
        };
        let ids = strings("doc_id").context("rag chunks are missing doc_id")?;
        let sources = strings("source").context("rag chunks are missing source")?;
        let indices = ints("chunk_index").context("rag chunks are missing chunk_index")?;
        let texts = strings("text").context("rag chunks are missing text")?;
        let lexical = strings("lexical_text").context("rag chunks are missing lexical_text")?;
        let tokens = ints("token_count").context("rag chunks are missing token_count")?;
        let starts = longs("char_start");
        let ends = longs("char_end");
        let pages_start = ints("page_start");
        let pages_end = ints("page_end");
        let sections_start = strings("section_start");
        let sections_end = strings("section_end");
        let tokenizer =
            strings("tokenizer_mode").context("rag chunks are missing tokenizer_mode")?;
        let chunker =
            strings("chunker_version").context("rag chunks are missing chunker_version")?;
        let vectors = batch
            .column_by_name("vector")
            .and_then(|column| column.as_any().downcast_ref::<FixedSizeListArray>())
            .context("rag chunks are missing vectors")?;
        for row in 0..batch.num_rows() {
            if ids.value(row) != source_id || vectors.is_null(row) {
                continue;
            }
            let vector_array = vectors.value(row);
            let values = vector_array
                .as_any()
                .downcast_ref::<Float32Array>()
                .context("rag chunk vector has an invalid type")?;
            chunks.push(crate::rag::DocChunk {
                doc_id: target_id.to_string(),
                source: sources.value(row).to_string(),
                chunk_index: indices.value(row),
                text: texts.value(row).to_string(),
                lexical_text: lexical.value(row).to_string(),
                vector: (0..values.len()).map(|index| values.value(index)).collect(),
                token_count: tokens.value(row),
                char_start: starts
                    .and_then(|array| (!array.is_null(row)).then(|| array.value(row))),
                char_end: ends.and_then(|array| (!array.is_null(row)).then(|| array.value(row))),
                page_start: pages_start
                    .and_then(|array| (!array.is_null(row)).then(|| array.value(row))),
                page_end: pages_end
                    .and_then(|array| (!array.is_null(row)).then(|| array.value(row))),
                section_start: sections_start
                    .and_then(|array| (!array.is_null(row)).then(|| array.value(row).to_string())),
                section_end: sections_end
                    .and_then(|array| (!array.is_null(row)).then(|| array.value(row).to_string())),
                tokenizer_mode: tokenizer.value(row).to_string(),
                chunker_version: chunker.value(row).to_string(),
            });
        }
    }
    if chunks.is_empty() {
        return Ok(false);
    }
    crate::rag::upsert_document(index_dir, target_id, chunks).await?;
    Ok(true)
}
