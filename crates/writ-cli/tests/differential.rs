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
    // `result` is an ordinary name outside `ensures` — both engines agree (#148).
    "fn f(n: Int) -> Int { let result = n + 1; return result; }\n\
     fn main() { print(f(41)); }",
    // Sum types, generic constructors, and `match` (payloads, nullary, catch-all).
    "type Option<T> = Some(T) | None\n\
     fn unwrap_or(o: Option<Int>, d: Int) -> Int { return match o { Some(x) => x, None => d }; }\n\
     fn main() { print(unwrap_or(Some(42), 0)); print(unwrap_or(None, 7)); print(Some(5)); print(None); }",
    // Text: literals (with escapes), printing, and structural equality.
    "fn main() { print(\"hello\"); print(\"a\\\"b\"); print(\"x\" == \"x\"); print(\"x\" == \"y\"); }",
    // Structural equality over variants.
    "type Pair = P(Int, Int)\n\
     fn main() { print(P(1, 2) == P(1, 2)); print(P(1, 2) == P(1, 3)); }",
    // Equality on the primitive comparable types (Int, Bool) agrees (#147).
    "fn main() { print(1 == 1); print(1 == 2); print(1 != 2);\n\
        print(true == true); print(true == false); print(true != false); }",
    // Text built-ins, including a multi-byte (UTF-8) string.
    "fn main() {\n\
        let s = concat(\"hel\", \"lo\");\n\
        print(s); print(text_len(s)); print(char_at(s, 0)); print(substring(s, 1, 4));\n\
        let u = \"héllo\";\n\
        print(text_len(u)); print(char_at(u, 1)); print(substring(u, 0, 2));\n\
     }",
    // char_code / code_char round-trips, incl. a non-ASCII scalar.
    "fn main() {\n\
        print(char_code(\"A\")); print(code_char(97)); print(char_code(\"é\")); print(code_char(233));\n\
        print(code_char(char_code(\"Z\")));\n\
     }",
    // Validator-based sanitize: a rule accepts or rejects, yielding Some / None.
    "fn is_short(s: Text) -> Bool { return text_len(s) < 6; }\n\
     fn main() {\n\
        print(match sanitize(\"hi\", is_short) { Some(x) => x, None => \"REJECTED\" });\n\
        print(match sanitize(\"toolong\", is_short) { Some(x) => x, None => \"REJECTED\" });\n\
     }",
    // Higher-order functions: pass and call pure function values.
    "fn apply(f: fn(Int) -> Int, x: Int) -> Int { return f(x); }\n\
     fn twice(g: fn(Int) -> Int, x: Int) -> Int { return g(g(x)); }\n\
     fn inc(n: Int) -> Int { return n + 1; }\n\
     fn double(n: Int) -> Int { return n * 2; }\n\
     fn main() { print(apply(inc, 5)); print(twice(double, 3)); print(inc); }",
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

/// The two fall-off-the-end programs from #145 (QA-11). They cannot live in
/// `CORPUS` — the interpreter would flow the last statement's value out while
/// the native back end returns `w_unit()`, a silent engine divergence (the
/// second even yields a wrong *arithmetic* answer with no trap). The fix is a
/// missing-return **compile error** (`T0016`), so these never reach either
/// engine. This test locks that in: the differential suite's job here is to
/// prove the divergence is unreachable, not to reproduce it.
const REFUSED_DIVERGENCES: &[&str] = &[
    // interp printed `42`, native printed `()`.
    "fn f() -> Int { 42; }\n\
     fn main() { print(f()); }",
    // interp printed `2`, native printed `1` (untagged `.i` read, no trap).
    "fn g() -> Unit { }\n\
     fn f() -> Int { g(); }\n\
     fn main() { print(f() + 1); }",
];

#[test]
fn fall_off_the_end_programs_are_refused_before_they_can_diverge() {
    for src in REFUSED_DIVERGENCES {
        let dir = scratch("refused");
        let src_path = dir.join("main.writ");
        std::fs::write(&src_path, src).expect("write source");

        let (program, diags) = writ_cli::load_program(&src_path);
        let mut all = diags;
        all.extend(writ_cli::check(&program));
        assert!(
            all.iter().any(|d| d.code == "T0016"),
            "a fall-off-the-end program must be a compile error, got: {all:?}\n{src}"
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

/// Assert both engines reject `src`: the interpreter returns an error and the
/// native binary exits non-zero, each mentioning `needle`. Text-invariant traps
/// (NUL, non-UTF-8) can't live in `CORPUS` — that suite requires exit 0 — so
/// they are checked here as a matched trap instead of a matched value.
fn assert_both_trap(tag: &str, src: &str, needle: &str) {
    let dir = scratch(tag);
    let src_path = dir.join("main.writ");
    std::fs::write(&src_path, src).expect("write source");

    let (program, _) = writ_cli::load_program(&src_path);
    let err = writ_cli::run(&program).expect_err("interpreter should trap");
    assert!(
        format!("{err:?}").contains(needle),
        "interpreter message should mention {needle:?}: {err:?}"
    );

    let bin = dir.join("prog");
    writ_cli::build(&program, &bin).expect("native build");
    let out = Command::new(&bin).output().expect("run native binary");
    assert!(!out.status.success(), "native binary should exit non-zero");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(needle),
        "native stderr should mention {needle:?}: {stderr}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn code_char_zero_traps_in_both_engines() {
    if !have_cc() {
        eprintln!("skipping differential test: no C compiler found");
        return;
    }
    // Family A (#146): U+0000 is the one scalar that encodes to a NUL byte, so a
    // NUL-terminated C string could not carry it. Both engines trap identically
    // rather than diverge (interp `1` vs native `0` on `text_len`).
    assert_both_trap(
        "nul_code_char",
        "fn main() { print(text_len(code_char(0))); }",
        "NUL (U+0000) is not allowed in text",
    );
}

#[test]
fn read_file_of_non_utf8_traps_in_both_engines() {
    if !have_cc() {
        eprintln!("skipping differential test: no C compiler found");
        return;
    }
    // Family B (#146): invalid UTF-8 content. The interpreter's `read_to_string`
    // rejects it; the native `w_read_file` now validates too, so both trap
    // instead of native silently printing a lenient byte count.
    let dir = scratch("read_non_utf8");
    let data = dir.join("data.bin");
    std::fs::write(&data, [b'a', b'b', 0xff, 0xfe, b'c', b'd']).expect("write data");
    let src = format!(
        "fn main(root: Cap<Root>) uses {{ Read }} {{\n\
            print(text_len(read_file(grant<Read>(root), \"{}\")));\n\
         }}",
        data.display()
    );
    assert_both_trap("read_non_utf8_run", &src, "valid UTF-8");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn read_file_containing_a_nul_traps_in_both_engines() {
    if !have_cc() {
        eprintln!("skipping differential test: no C compiler found");
        return;
    }
    // Family A via `read_file`: a NUL byte is valid UTF-8 (U+0000) but violates
    // the no-NUL text invariant, so both engines reject it rather than diverge
    // (interp a 5-char string vs native a NUL-truncated one).
    let dir = scratch("read_nul");
    let data = dir.join("data.bin");
    std::fs::write(&data, [b'a', b'b', 0x00, b'c', b'd']).expect("write data");
    let src = format!(
        "fn main(root: Cap<Root>) uses {{ Read }} {{\n\
            print(text_len(read_file(grant<Read>(root), \"{}\")));\n\
         }}",
        data.display()
    );
    assert_both_trap("read_nul_run", &src, "not valid");
    let _ = std::fs::remove_dir_all(&dir);
}
