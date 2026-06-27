//! Diagnostic: compile every .tex note in a directory through the REAL app
//! transform (myelin_lib::state::compile_tex_source) and report PASS/FAIL.
//! Run: TECTONIC_CACHE_DIR=<dir> cargo run --bin texcheck -- <workspace_dir>

use myelin_lib::state::compile_tex_source;

fn main() {
    let dir = std::env::args()
        .nth(1)
        .expect("usage: texcheck <workspace_dir>");
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
        .expect("read_dir")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("tex"))
        .collect();
    files.sort();
    if files.is_empty() {
        println!("no .tex files in {dir}");
        return;
    }
    let mut ok = 0;
    let mut fail = 0;
    for p in &files {
        let name = p.file_name().unwrap().to_string_lossy();
        let raw = std::fs::read_to_string(p).unwrap_or_default();
        match compile_tex_source(&raw) {
            Ok(pdf) => {
                ok += 1;
                println!("OK    {name}  ({} bytes)", pdf.len());
            }
            Err(msg) => {
                fail += 1;
                println!("FAIL  {name}  ->  {}", msg.lines().next().unwrap_or(""));
            }
        }
    }
    println!("\n=== {ok} ok, {fail} fail, {} total ===", files.len());
    if fail > 0 {
        std::process::exit(1);
    }
}
