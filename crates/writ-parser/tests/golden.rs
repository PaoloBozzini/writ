//! Golden-file (snapshot) tests over a corpus of sample programs.
//!
//! For each `tests/corpus/*.writ` file this compares two committed snapshots:
//! `*.tokens` (the lexer's token stream) and `*.ast` (the parser's AST). A
//! regression shows up as a readable diff in the snapshot.
//!
//! The harness is dependency-free and deterministic: corpus files are visited
//! in sorted order, and rendering uses only stable `Debug`/byte output. To
//! regenerate the snapshots after an intentional change, run:
//!
//! ```text
//! UPDATE_GOLDEN=1 cargo test -p writ-parser --test golden
//! ```
//!
//! CI never sets `UPDATE_GOLDEN`, so there it is a pure comparison.

use std::fs;
use std::path::{Path, PathBuf};

fn corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus")
}

/// Render the lexer's output for `src` as one line per token, plus any
/// diagnostics as canonical JSON.
fn render_tokens(src: &str) -> String {
    let lexed = writ_lexer::lex(src);
    let mut out = String::new();
    for t in &lexed.tokens {
        out.push_str(&format!(
            "{:?} @ {}..{}\n",
            t.kind, t.span.start, t.span.end
        ));
    }
    for d in &lexed.diagnostics {
        out.push_str(&format!("DIAG {}\n", d.to_json()));
    }
    out
}

/// Render the parser's AST for `src` via pretty `Debug`, plus any diagnostics.
fn render_ast(src: &str) -> String {
    let result = writ_parser::parse(src);
    let mut out = format!("{:#?}\n", result.module);
    for d in &result.diagnostics {
        out.push_str(&format!("DIAG {}\n", d.to_json()));
    }
    out
}

/// Compare `actual` against the snapshot at `golden`, or write it when
/// `UPDATE_GOLDEN` is set.
fn check_golden(golden: &Path, actual: &str) {
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        fs::write(golden, actual).expect("write golden");
        return;
    }
    let expected = fs::read_to_string(golden).unwrap_or_else(|_| {
        panic!(
            "missing golden {}; run `UPDATE_GOLDEN=1 cargo test -p writ-parser --test golden` to create it",
            golden.display()
        )
    });
    assert_eq!(
        actual,
        expected.as_str(),
        "snapshot mismatch for {}",
        golden.display()
    );
}

/// Every corpus program is snapshotted for both the lexer and the parser.
#[test]
fn corpus_snapshots() {
    let dir = corpus_dir();
    let mut sources: Vec<PathBuf> = fs::read_dir(&dir)
        .unwrap_or_else(|_| panic!("cannot read corpus dir {}", dir.display()))
        .map(|e| e.expect("dir entry").path())
        .filter(|p| p.extension().is_some_and(|e| e == "writ"))
        .collect();
    sources.sort(); // Deterministic visitation order.

    assert!(!sources.is_empty(), "corpus is empty at {}", dir.display());

    for path in sources {
        let src = fs::read_to_string(&path).expect("read corpus file");
        check_golden(&path.with_extension("tokens"), &render_tokens(&src));
        check_golden(&path.with_extension("ast"), &render_ast(&src));
    }
}
