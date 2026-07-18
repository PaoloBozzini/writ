//! Differential conformance: the native C back end must agree with the
//! tree-walking interpreter (the semantic reference) on a corpus of core
//! programs. For each program we run it both ways and require identical output.
//!
//! This is the permanent conformance suite promised by #29 / #38. It needs a
//! system C compiler; if none is found the test is skipped rather than failed,
//! so the suite stays green on machines without a toolchain.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Every core program the back end is expected to compile identically to the
/// interpreter. Each prints deterministic output from `main`.
const CORPUS: &[&str] = &[
    // Arithmetic, calls, and printing.
    "fn add(a: Int, b: Int) -> Int { return a + b; }\n\
     fn main() { print(add(3, 4)); print(2 * 5 - 1); }",
    // Booleans and short-circuit operators.
    "fn main() { print(1 < 2 && true); print(false || 3 >= 3); print(!true); }",
    // Conditionals and recursion.
    "fn fact(n: Int) -> Int { if n <= 1 { return 1; } return n * fact(n - 1); }\n\
     fn main() { print(fact(5)); }",
    // Integer division and remainder (truncating toward zero).
    "fn main() { print(10 / 3); print(10 - 13); print(17 - 20 * 1); print(9 / 2); }",
    // Contracts that hold flow through unchanged.
    "fn max(a: Int, b: Int) -> Int ensures result >= a ensures result >= b {\n\
        if a > b { return a; }\n\
        return b;\n\
     }\n\
     fn main() { print(max(9, 2)); print(max(-4, 7)); }",
    // A precondition that is satisfied.
    "fn half(n: Int) -> Int requires n > 0 { return n / 2; }\n\
     fn main() { print(half(8)); }",
    // Sum types, generic constructors, and `match` (payloads, nullary, catch-all).
    "type Option<T> = Some(T) | None\n\
     fn unwrap_or(o: Option<Int>, d: Int) -> Int { return match o { Some(x) => x, None => d }; }\n\
     fn main() { print(unwrap_or(Some(42), 0)); print(unwrap_or(None, 7)); print(Some(5)); print(None); }",
    // Text: literals (with escapes), printing, and structural equality.
    "fn main() { print(\"hello\"); print(\"a\\\"b\"); print(\"x\" == \"x\"); print(\"x\" == \"y\"); }",
    // Structural equality over variants.
    "type Pair = P(Int, Int)\n\
     fn main() { print(P(1, 2) == P(1, 2)); print(P(1, 2) == P(1, 3)); }",
    // Text built-ins, including a multi-byte (UTF-8) string.
    "fn main() {\n\
        let s = concat(\"hel\", \"lo\");\n\
        print(s); print(text_len(s)); print(char_at(s, 0)); print(substring(s, 1, 4));\n\
        let u = \"héllo\";\n\
        print(text_len(u)); print(char_at(u, 1)); print(substring(u, 0, 2));\n\
     }",
    // Nested match sub-patterns.
    "type Option<T> = Some(T) | None\n\
     type Pair = P(Int, Int)\n\
     fn f(o: Option<Pair>) -> Int { return match o { Some(P(a, b)) => a + b, None => 0 }; }\n\
     fn main() { print(f(Some(P(3, 4)))); print(f(None)); }",
    // Capabilities: `grant` narrows authority; a capability prints opaquely.
    "fn write_line(out: Cap<Write>, msg: Text) uses { Write } { return; }\n\
     fn main(root: Cap<Root>) uses { Write } {\n\
        write_line(grant<Write>(root), \"hi\");\n\
        print(\"done\");\n\
        print(grant<Write>(root));\n\
     }",
];

fn cc() -> String {
    std::env::var("CC").unwrap_or_else(|_| "cc".to_string())
}

/// Whether a C compiler is available; if not, differential tests are skipped.
fn have_cc() -> bool {
    Command::new(cc())
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// A unique-per-case scratch directory under the system temp dir. `tag` keeps
/// concurrent cases from colliding without needing a clock or RNG.
fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("writ_diff_{tag}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

/// The interpreter's output for a source file.
fn interpret(src_path: &Path) -> Vec<String> {
    let (program, diags) = writ_cli::load_program(src_path);
    let mut all = diags;
    all.extend(writ_cli::check(&program));
    assert!(
        !all.iter().any(writ_ast::Diagnostic::is_error),
        "corpus program should check cleanly: {all:?}"
    );
    writ_cli::run(&program).expect("interpreter run")
}

/// The native binary's stdout for a source file, as lines.
fn native(program_dir: &Path, src_path: &Path) -> Vec<String> {
    let (program, _) = writ_cli::load_program(src_path);
    let bin = program_dir.join("prog");
    writ_cli::build(&program, &bin).expect("native build");
    let out = Command::new(&bin).output().expect("run native binary");
    assert!(out.status.success(), "native binary exited non-zero");
    String::from_utf8(out.stdout)
        .expect("utf8 output")
        .lines()
        .map(str::to_string)
        .collect()
}

#[test]
fn native_output_matches_interpreter_on_the_corpus() {
    if !have_cc() {
        eprintln!("skipping differential test: no C compiler found");
        return;
    }
    for (i, src) in CORPUS.iter().enumerate() {
        let dir = scratch(&format!("case{i}"));
        let src_path = dir.join("main.writ");
        std::fs::write(&src_path, src).expect("write source");

        let interp_out = interpret(&src_path);
        let native_out = native(&dir, &src_path);
        assert_eq!(
            interp_out, native_out,
            "program #{i} disagrees between interpreter and native:\n{src}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[test]
fn a_violated_precondition_traps_in_the_native_binary() {
    if !have_cc() {
        eprintln!("skipping differential test: no C compiler found");
        return;
    }
    // The interpreter blames the caller for a failed precondition; the native
    // binary reproduces the same message and exits non-zero.
    let dir = scratch("trap");
    let src_path = dir.join("main.writ");
    std::fs::write(
        &src_path,
        "fn half(n: Int) -> Int requires n > 0 { return n / 2; }\n\
         fn main() { print(half(0)); }",
    )
    .expect("write source");

    let (program, _) = writ_cli::load_program(&src_path);
    let bin = dir.join("prog");
    writ_cli::build(&program, &bin).expect("native build");
    let out = Command::new(&bin).output().expect("run native binary");
    assert!(!out.status.success(), "a trapped binary must exit non-zero");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("precondition violated (blame: caller)"),
        "native trap message: {stderr}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
