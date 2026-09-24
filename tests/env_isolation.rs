//! No library unit test may change the process environment or working directory.
//!
//! All of the library's unit tests share one process and run in parallel. A test that set
//! `TB_READONLY` for a moment made every store opened beside it refuse writes, so a dozen
//! unrelated tests failed now and then under load with "read-only mode … cannot do". The fix
//! tests the parsing of such values directly; this keeps anyone from bringing the pattern back.
//! (`src/main.rs` is the `tb` binary itself, one process per command: its `--read-only` sets
//! `TB_READONLY` for that process on purpose, and is not a test.)
use std::path::Path;

fn rust_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

#[test]
fn no_library_code_changes_the_process_environment() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&src, &mut files);
    let mut hits = Vec::new();
    for f in files.iter().filter(|f| !f.ends_with("main.rs")) {
        let text = std::fs::read_to_string(f).unwrap();
        for (n, line) in text.lines().enumerate() {
            let code = line.split("//").next().unwrap_or("");
            if ["env::set_var(", "env::remove_var(", "env::set_current_dir("].iter().any(|w| code.contains(w)) {
                hits.push(format!("{}:{}: {}", f.strip_prefix(&src).unwrap().display(), n + 1, line.trim()));
            }
        }
    }
    assert!(hits.is_empty(), "the library's tests share one process — test the value, not the environment:\n{}", hits.join("\n"));
}
