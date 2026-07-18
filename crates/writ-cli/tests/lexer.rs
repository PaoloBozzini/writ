//! The Writ-hosted lexer (`bootstrap/lexer.writ`, #34): the first stage of the
//! self-hosted compiler, written in Writ. It must check, run, and compile
//! natively — tokenizing a Writ snippet, with the interpreter and the binary
//! producing the same token stream.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The lexer module, embedded so the test does not depend on CWD.
const LEXER: &str = include_str!("../../../bootstrap/lexer.writ");

const DRIVER: &str = "\
import lexer
fn main() {
    lexer.print_tokens(lexer.lex(\"fn add(a, b) -> a + b; x == 12 * (3 - 4)\"));
}
";

const EXPECTED: &[&str] = &[
    "Ident(fn)",
    "Ident(add)",
    "LParen",
    "Ident(a)",
    "Comma",
    "Ident(b)",
    "RParen",
    "Arrow",
    "Ident(a)",
    "Plus",
    "Ident(b)",
    "Semi",
    "Ident(x)",
    "EqEq",
    "Num(12)",
    "Star",
    "LParen",
    "Num(3)",
    "Minus",
    "Num(4)",
    "RParen",
];

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("writ_lexer_{tag}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    std::fs::write(dir.join("lexer.writ"), LEXER).expect("write lexer");
    std::fs::write(dir.join("main.writ"), DRIVER).expect("write driver");
    dir
}

fn cc_available() -> bool {
    let cc = std::env::var("CC").unwrap_or_else(|_| "cc".to_string());
    Command::new(cc)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn root(dir: &Path) -> PathBuf {
    dir.join("main.writ")
}

#[test]
fn the_writ_lexer_checks_and_tokenizes() {
    let dir = scratch("run");
    let (program, mut diags) = writ_cli::load_program(&root(&dir));
    diags.extend(writ_cli::check(&program));
    assert!(
        !diags.iter().any(writ_ast::Diagnostic::is_error),
        "the lexer should check cleanly: {diags:?}"
    );
    let out = writ_cli::run(&program).expect("run");
    assert_eq!(out, EXPECTED);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_writ_lexer_compiles_and_matches_the_interpreter() {
    if !cc_available() {
        eprintln!("skipping native lexer test: no C compiler found");
        return;
    }
    let dir = scratch("build");
    let (program, _) = writ_cli::load_program(&root(&dir));
    let bin = dir.join("prog");
    writ_cli::build(&program, &bin).expect("build");
    let out = Command::new(&bin).output().expect("run native");
    assert!(out.status.success());
    let lines: Vec<String> = String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect();
    assert_eq!(lines, EXPECTED, "native tokens must match the interpreter");
    let _ = std::fs::remove_dir_all(&dir);
}
