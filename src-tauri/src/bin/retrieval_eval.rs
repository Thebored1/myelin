//! Run the synthetic retrieval baseline without touching app data.

fn main() {
    let mut args = std::env::args().skip(1);
    let (default_corpus, default_queries) = myelin_lib::retrieval_eval::default_fixture_paths();
    let corpus = args.next().map(Into::into).unwrap_or(default_corpus);
    let queries = args.next().map(Into::into).unwrap_or(default_queries);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("create Tokio runtime");
    match runtime.block_on(myelin_lib::retrieval_eval::run_baseline(&corpus, &queries)) {
        Ok(report) => println!("{}", serde_json::to_string_pretty(&report).expect("serialize report")),
        Err(error) => {
            eprintln!("retrieval evaluation failed: {error:#}");
            std::process::exit(1);
        }
    }
}
