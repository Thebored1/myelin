//! LanceDB schema definition, compatibility marker, and table open/create.

use anyhow::{Context, Result};
use arrow_schema::{DataType, Field, Schema};
use lancedb::connection::Connection;
use lancedb::{connect, Table};
use std::path::Path;
use std::sync::Arc;

use super::types::{LEXICAL_VERSION, RAG_TABLE};
pub(super) use super::types::{RAG_SCHEMA_MARKER, RAG_SCHEMA_VERSION};

pub(super) fn schema(dimension: i32) -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("doc_id", DataType::Utf8, false),
        Field::new("source", DataType::Utf8, false),
        Field::new("chunk_index", DataType::Int32, false),
        Field::new("text", DataType::Utf8, false),
        Field::new("lexical_text", DataType::Utf8, false),
        Field::new("token_count", DataType::Int32, false),
        Field::new("char_start", DataType::Int64, true),
        Field::new("char_end", DataType::Int64, true),
        Field::new("page_start", DataType::Int32, true),
        Field::new("page_end", DataType::Int32, true),
        Field::new("section_start", DataType::Utf8, true),
        Field::new("section_end", DataType::Utf8, true),
        Field::new("tokenizer_mode", DataType::Utf8, false),
        Field::new("chunker_version", DataType::Utf8, false),
        Field::new(
            "vector",
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, true)),
                dimension,
            ),
            true,
        ),
    ]))
}

#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct SchemaMarker {
    pub(super) version: u32,
    chunker: String,
    dimension: i32,
    lexical_version: String,
    pub(super) expected_columns: Vec<String>,
}

/// Ensure an incompatible *derived* RAG index is reset. Workspace files and
/// source documents live elsewhere and are never touched here.
pub fn prepare_schema(index_dir: &Path, dimension: i32) -> Result<bool> {
    let marker_path = index_dir.join(RAG_SCHEMA_MARKER);
    let expected = SchemaMarker {
        version: RAG_SCHEMA_VERSION,
        chunker: crate::embeddings::CHUNKER_VERSION.into(),
        dimension,
        lexical_version: LEXICAL_VERSION.into(),
        expected_columns: schema(dimension)
            .fields()
            .iter()
            .map(|field| field.name().clone())
            .collect(),
    };
    let current = std::fs::read(&marker_path)
        .ok()
        .and_then(|raw| serde_json::from_slice::<SchemaMarker>(&raw).ok());
    let compatible = current.is_some_and(|marker| {
        marker.version == expected.version
            && marker.chunker == expected.chunker
            && marker.dimension == expected.dimension
            && marker.lexical_version == expected.lexical_version
            && marker.expected_columns == expected.expected_columns
    });
    let reset = index_dir.exists() && !compatible;
    if reset {
        std::fs::remove_dir_all(index_dir).with_context(|| {
            format!("failed to reset derived rag index {}", index_dir.display())
        })?;
    }
    std::fs::create_dir_all(index_dir)
        .with_context(|| format!("failed to create rag index {}", index_dir.display()))?;
    if !compatible {
        let temp = marker_path.with_extension("json.tmp");
        std::fs::write(&temp, serde_json::to_vec_pretty(&expected)?)?;
        std::fs::rename(temp, marker_path)?;
    }
    Ok(reset)
}

pub(super) async fn open(index_dir: &Path, dimension: i32) -> Result<Connection> {
    prepare_schema(index_dir, dimension)?;
    connect(index_dir.to_string_lossy().as_ref())
        .execute()
        .await
        .context("failed to open rag db")
}

pub(super) async fn open_or_create(conn: &Connection, dimension: i32) -> Result<Table> {
    let table = match conn.open_table(RAG_TABLE).execute().await {
        Ok(t) => {
            let actual = t
                .schema()
                .await
                .context("failed to inspect rag table schema")?;
            let expected = schema(dimension);
            let compatible =
                actual
                    .fields()
                    .iter()
                    .zip(expected.fields())
                    .all(|(actual, expected)| {
                        actual.name() == expected.name()
                            && actual.data_type() == expected.data_type()
                            && actual.is_nullable() == expected.is_nullable()
                    })
                    && actual.fields().len() == expected.fields().len();
            if compatible {
                Ok(t)
            } else {
                log::warn!(
                    "[rag] live schema disagrees with marker; recreating derived document table"
                );
                conn.drop_table(RAG_TABLE)
                    .await
                    .context("failed to reset incompatible rag table")?;
                conn.create_empty_table(RAG_TABLE, schema(dimension))
                    .execute()
                    .await
                    .context("failed to recreate rag table")
            }
        }
        Err(_) => conn
            .create_empty_table(RAG_TABLE, schema(dimension))
            .execute()
            .await
            .context("failed to create rag table"),
    }?;
    // Marker files cannot prove an FTS index exists. Inspect the live table
    // before returning it and repair a missing lexical index immediately.
    let indices = table
        .list_indices()
        .await
        .context("failed to inspect rag indexes")?;
    let has_fts = indices.iter().any(|index| {
        index.columns.iter().any(|column| column == "lexical_text")
            && matches!(index.index_type, lancedb::index::IndexType::FTS)
    });
    if !has_fts {
        table
            .create_index(
                &["lexical_text"],
                lancedb::index::Index::FTS(Default::default()),
            )
            .execute()
            .await
            .context("failed to create rag FTS index")?;
    }
    Ok(table)
}
