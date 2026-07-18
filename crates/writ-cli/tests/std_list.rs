//! The `std/list.writ` standard-library module (first self-hosting brick, #121):
//! a Writ-authored data structure that must check, run, and compile natively —
//! with the interpreter and the binary agreeing.

use std::path::PathBuf;
use std::process::Command;

/// The stdlib module source, embedded so the test does not depend on CWD.
const LIST: &str = include_str!("../../../std/list.writ");

const DRIVER: &str = "\
import list
fn main() {
    let xs = list.push_front(1, list.push_front(2, list.push_front(3, Nil)));
    print(list.len(xs));
    print(list.sum(xs));
    print(list.contains(xs, 2));
    print(list.contains(xs, 9));
    print(list.nth_or(xs, 0, 0));
    print(list.nth_or(xs, 2, 0));
    print(list.nth_or(xs, 5, 0));
    print(list.sum(list.reverse(xs)));
    print(list.len(list.append(xs, xs)));
    print(list.is_empty(Nil));
}
";

const EXPECTED: &[&str] = &["3", "6", "true", "false", "1", "3", "0", "6", "6", "true"];

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("writ_stdlist_{tag}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    std::fs::write(dir.join("list.writ"), LIST).expect("write list");
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

#[test]
fn the_list_module_checks_and_runs() {
    let dir = scratch("run");
    let root = dir.join("main.writ");
    let (program, mut diags) = writ_cli::load_program(&root);
    diags.extend(writ_cli::check(&program));
    assert!(
        !diags.iter().any(writ_ast::Diagnostic::is_error),
        "std/list.writ program should check cleanly: {diags:?}"
    );
    let out = writ_cli::run(&program).expect("run");
    assert_eq!(out, EXPECTED);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_list_module_compiles_and_matches_the_interpreter() {
    if !cc_available() {
        eprintln!("skipping native list test: no C compiler found");
        return;
    }
    let dir = scratch("build");
    let root = dir.join("main.writ");
    let (program, _) = writ_cli::load_program(&root);
    let bin = dir.join("prog");
    writ_cli::build(&program, &bin).expect("build");
    let out = Command::new(&bin).output().expect("run native");
    assert!(out.status.success());
    let lines: Vec<String> = String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect();
    assert_eq!(lines, EXPECTED, "native output must match the interpreter");
    let _ = std::fs::remove_dir_all(&dir);
}
