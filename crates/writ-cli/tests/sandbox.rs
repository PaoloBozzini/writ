//! Sandbox guarantees (#33): the check step executes nothing, and the runtime
//! has no ambient filesystem or network authority.

use std::collections::BTreeMap;

fn program(src: &str) -> writ_cli::Program {
    let module = writ_parser::parse(src).module;
    let mut modules = BTreeMap::new();
    modules.insert("main".to_string(), module);
    writ_cli::Program {
        modules,
        root: "main".to_string(),
    }
}

#[test]
fn the_check_step_does_not_execute_the_program() {
    // `main` divides by zero — but only *at runtime*. If checking executed the
    // program we'd observe that failure; instead the check step is pure static
    // analysis and reports nothing.
    let p = program("fn main() { print(1 / 0); }");
    assert!(
        writ_cli::check(&p).is_empty(),
        "check must not execute the program"
    );
    // Proof the failure is a genuine runtime effect: running it does fail.
    assert!(writ_cli::run(&p).is_err());
}

#[test]
fn there_is_no_ambient_filesystem_authority() {
    // `read_file` exists, but it is **capability-gated**, not ambient: a
    // sandboxed `main` (no capability parameter) that reaches for it is refused
    // statically — the honesty check (no `uses { Read }`) and the authority
    // check (holds no `Cap<Read>`) both fire. So there is no ambient path to the
    // filesystem, checked or run.
    let p = program("fn main() { read_file(\"/etc/passwd\"); }");
    let codes: Vec<String> = writ_cli::check(&p).iter().map(|d| d.code.clone()).collect();
    assert!(
        codes.contains(&"E0101".to_string()) && codes.contains(&"E0301".to_string()),
        "ambient file access must be refused by honesty + authority, got {codes:?}"
    );
    assert!(
        writ_cli::run(&p).is_err(),
        "running it fails closed, touching nothing"
    );
}

#[test]
fn there_is_no_ambient_network_authority() {
    let p = program("fn main() { http_get(\"http://example.com\"); }");
    assert!(
        writ_cli::run(&p).is_err(),
        "no network primitive exists to reach"
    );
}
