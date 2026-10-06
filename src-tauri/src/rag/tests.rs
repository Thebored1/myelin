use super::ingest::{contains_document, upsert_document};
use super::packing::pack_passages;
use super::packing::pack_passages_limited;
use super::schema::{prepare_schema, SchemaMarker, RAG_SCHEMA_MARKER, RAG_SCHEMA_VERSION};
use super::search::{search, search_hybrid};
use super::types::{DocChunk, RetrievedChunk, DIM};

fn chunk(id: &str, idx: i32, v: f32) -> DocChunk {
    DocChunk {
        doc_id: id.into(),
        source: "test".into(),
        chunk_index: idx,
        text: format!("chunk {idx}"),
        lexical_text: format!("test chunk {idx}"),
        vector: vec![v; DIM as usize],
        ..Default::default()
    }
}

#[tokio::test]
async fn ingest_search_and_replace_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    // Two chunks: one near 0.1, one near 0.9.
    upsert_document(
        dir.path(),
        "d1",
        vec![chunk("d1", 0, 0.1), chunk("d1", 1, 0.9)],
    )
    .await
    .unwrap();

    // Query closest to the 0.9 vector → chunk_index 1 ranks first.
    let res = search(dir.path(), vec![0.9; DIM as usize], 5)
        .await
        .unwrap();
    assert_eq!(res.len(), 2);
    assert_eq!(res[0].chunk_index, 1);
    assert_eq!(res[0].doc_id, "d1");

    // Re-ingesting the same doc replaces its chunks (not append).
    upsert_document(dir.path(), "d1", vec![chunk("d1", 0, 0.5)])
        .await
        .unwrap();
    let res2 = search(dir.path(), vec![0.5; DIM as usize], 5)
        .await
        .unwrap();
    assert_eq!(res2.len(), 1);
    assert_eq!(res2[0].chunk_index, 0);
}

#[tokio::test]
async fn search_missing_table_is_empty() {
    let dir = tempfile::tempdir().unwrap();
    let res = search(dir.path(), vec![0.0; DIM as usize], 5)
        .await
        .unwrap();
    assert!(res.is_empty());
}

#[tokio::test]
async fn hybrid_search_reports_uninitialized_index() {
    let dir = tempfile::tempdir().unwrap();
    let err = search_hybrid(dir.path(), vec![0.0; DIM as usize], "query", 5, None)
        .await
        .unwrap_err();
    assert!(err
        .to_string()
        .contains("document index is not initialized"));
}

#[test]
fn incompatible_marker_resets_only_the_derived_index() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join(RAG_SCHEMA_MARKER);
    std::fs::write(&marker, r#"{"version":1}"#).unwrap();
    std::fs::write(dir.path().join("derived-only"), "stale").unwrap();
    assert!(prepare_schema(dir.path(), DIM).unwrap());
    assert!(!dir.path().join("derived-only").exists());
    let current: SchemaMarker = serde_json::from_slice(&std::fs::read(marker).unwrap()).unwrap();
    assert_eq!(current.version, RAG_SCHEMA_VERSION);
    assert!(current
        .expected_columns
        .iter()
        .any(|column| column == "lexical_text"));
}

#[test]
fn limited_passage_packing_deduplicates_and_caps_results() {
    let make = |index: i32, text: &str| RetrievedChunk {
        doc_id: "d".into(),
        source: "source".into(),
        chunk_index: index,
        text: text.into(),
        distance: 0.0,
        ..Default::default()
    };
    let out = pack_passages_limited(
        vec![make(0, "same"), make(1, "same"), make(2, "second")],
        1_000,
        2,
    );
    assert_eq!(out.matches("[source | document d]").count(), 2);
    assert!(out.contains("same"));
    assert!(out.contains("second"));
}

#[test]
fn lexical_analysis_uses_latest_question_not_previous_context() {
    let query = "Latest question: does it hold the works of William Shakespeare\nPrevious assistant context (may be incorrect): prayer blessing curses";
    let analysis = crate::retrieval_pipeline::RetrievalQuery::analyze(query);
    assert!(analysis.original.contains("William Shakespeare"));
    assert!(!analysis.original.contains("prayer"));
    assert!(analysis.fts_query().contains("shakespeare"));
}

#[tokio::test]
async fn hybrid_runs_vector_plus_fts() {
    let dir = tempfile::tempdir().unwrap();
    let docs = vec![
        DocChunk {
            doc_id: "d".into(),
            source: "s".into(),
            chunk_index: 0,
            text: "the eiffel tower is in paris france".into(),
            lexical_text: "s the eiffel tower is in paris france".into(),
            vector: vec![0.1; DIM as usize],
            ..Default::default()
        },
        DocChunk {
            doc_id: "d".into(),
            source: "s".into(),
            chunk_index: 1,
            text: "transformers use the attention mechanism".into(),
            lexical_text: "s transformers use the attention mechanism".into(),
            vector: vec![0.9; DIM as usize],
            ..Default::default()
        },
    ];
    upsert_document(dir.path(), "d", docs).await.unwrap();
    // Hybrid: BM25 should surface chunk 1 on the text terms even though the
    // query vector is nearer chunk 0. Just assert the merge runs and returns.
    let res = search_hybrid(
        dir.path(),
        vec![0.1; DIM as usize],
        "attention transformers",
        5,
        None,
    )
    .await
    .unwrap();
    assert!(!res.is_empty());
    assert!(res.iter().any(|c| c.chunk_index == 1));
}

#[tokio::test]
async fn scoped_hybrid_and_document_presence() {
    let dir = tempfile::tempdir().unwrap();
    upsert_document(dir.path(), "note-a", vec![chunk("note-a", 0, 0.5)])
        .await
        .unwrap();
    upsert_document(dir.path(), "note-b", vec![chunk("note-b", 0, 0.5)])
        .await
        .unwrap();

    assert!(contains_document(dir.path(), "note-a").await.unwrap());
    assert!(!contains_document(dir.path(), "missing").await.unwrap());
    let scope = vec!["note-a".to_string()];
    let hits = search_hybrid(
        dir.path(),
        vec![0.5; DIM as usize],
        "chunk",
        10,
        Some(&scope),
    )
    .await
    .unwrap();
    assert!(!hits.is_empty());
    assert!(hits.iter().all(|hit| hit.doc_id == "note-a"));

    let pair = vec!["note-a".to_string(), "note-b".to_string()];
    let pair_hits = search_hybrid(
        dir.path(),
        vec![0.5; DIM as usize],
        "chunk",
        10,
        Some(&pair),
    )
    .await
    .unwrap();
    assert!(pair_hits.iter().any(|hit| hit.doc_id == "note-a"));
    assert!(pair_hits.iter().any(|hit| hit.doc_id == "note-b"));
}

#[test]
fn passage_packing_labels_sources_and_respects_adaptive_budget() {
    let chunks = vec![RetrievedChunk {
        doc_id: "pdf-a".into(),
        source: "Paper.pdf".into(),
        chunk_index: 0,
        text: "abcdefghij".into(),
        distance: 1.0,
        ..Default::default()
    }];
    let packed = pack_passages(chunks, 4);
    assert!(packed.contains("[Paper.pdf | document pdf-a]"));
    assert!(packed.ends_with("abcd"));
    assert!(!packed.contains("abcde"));
}
