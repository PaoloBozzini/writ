//! Cross-module safety (#96): because the checkers run over the *linked*
//! program, a call across a module boundary is an effect site / sink / typed
//! call exactly like a local one. The pre-fix suite had zero such tests.

use std::path::{Path, PathBuf};

/// Write `files` (name → source) into a fresh scratch dir and return the path to
/// the named root file.
fn program(tag: &str, files: &[(&str, &str)], root: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("writ_xmod_{tag}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    for (name, src) in files {
        std::fs::write(dir.join(name), src).expect("write module");
    }
    dir.join(root)
}

fn check_codes(root: &Path) -> Vec<String> {
    let (prog, mut diags) = writ_cli::load_program(root);
    diags.extend(writ_cli::check(&prog));
    diags.into_iter().map(|d| d.code).collect()
}

#[test]
fn a_cross_module_effect_needs_authority_and_honesty() {
    // `sneaky` reaches `io.write_file` (uses { Write }) across a boundary with
    // no capability and no declaration.
    let root = program(
        "effect",
        &[
            (
                "io.writ",
                "export fn write_file(p: Text, c: Text) uses { Write } { return; }",
            ),
            (
                "main.writ",
                "import io\nfn sneaky() { io.write_file(\"x\", \"y\"); }\nfn main() { sneaky(); }",
            ),
        ],
        "main.writ",
    );
    let cs = check_codes(&root);
    assert!(
        cs.contains(&"E0101".to_string()),
        "honesty across boundary: {cs:?}"
    );
    assert!(
        cs.contains(&"E0301".to_string()),
        "authority across boundary: {cs:?}"
    );
}

#[test]
fn a_cross_module_call_is_type_checked() {
    // `math.add` takes two Ints; passing a Bool must be a type error, not a
    // runtime surprise.
    let root = program(
        "types",
        &[
            (
                "math.writ",
                "export fn add(a: Int, b: Int) -> Int { return a + b; }",
            ),
            (
                "main.writ",
                "import math\nfn main() { print(math.add(true, 2)); }",
            ),
        ],
        "main.writ",
    );
    let cs = check_codes(&root);
    assert!(
        cs.contains(&"T0001".to_string()),
        "cross-module type check: {cs:?}"
    );
}

#[test]
fn a_cross_module_arity_mismatch_is_caught() {
    let root = program(
        "arity",
        &[
            (
                "math.writ",
                "export fn add(a: Int, b: Int) -> Int { return a + b; }",
            ),
            (
                "main.writ",
                "import math\nfn main() { print(math.add(1)); }",
            ),
        ],
        "main.writ",
    );
    let cs = check_codes(&root);
    assert!(
        cs.contains(&"T0004".to_string()),
        "cross-module arity: {cs:?}"
    );
}

#[test]
fn a_cross_module_sink_rejects_tainted_arguments() {
    // A tainted value reaching a sink in another module is E0401.
    let root = program(
        "taint",
        &[
            (
                "db.writ",
                "export fn run(q: Tainted<Text>) uses { Query } { return; }",
            ),
            (
                "main.writ",
                "import db\nfn handle(input: Tainted<Text>) uses { Query } { db.run(input); }\nfn main() {}",
            ),
        ],
        "main.writ",
    );
    let cs = check_codes(&root);
    assert!(
        cs.contains(&"E0401".to_string()),
        "cross-module taint sink: {cs:?}"
    );
}

#[test]
fn a_safe_cross_module_call_still_checks_clean() {
    // The happy path — a pure exported function called with correct types.
    let root = program(
        "ok",
        &[
            (
                "math.writ",
                "export fn add(a: Int, b: Int) -> Int { return a + b; }",
            ),
            (
                "main.writ",
                "import math\nfn main() { print(math.add(2, 3)); }",
            ),
        ],
        "main.writ",
    );
    let cs = check_codes(&root);
    assert!(
        cs.is_empty(),
        "safe cross-module call must check clean: {cs:?}"
    );
}
