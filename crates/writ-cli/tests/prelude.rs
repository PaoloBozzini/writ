//! The prelude (#136): standard sum types (`Option`, `Result`) are always in
//! scope — usable with no declaration or import — and a user-declared type of
//! the same name shadows the prelude one.

use std::path::{Path, PathBuf};
use std::process::Command;

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("writ_prelude_{tag}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

fn write_main(dir: &Path, src: &str) -> PathBuf {
    let path = dir.join("main.writ");
    std::fs::write(&path, src).unwrap();
    path
}

fn check_and_run(src: &str, tag: &str) -> Vec<String> {
    let dir = scratch(tag);
    let path = write_main(&dir, src);
    let (program, mut diags) = writ_cli::load_program(&path);
    diags.extend(writ_cli::check(&program));
    assert!(
        !diags.iter().any(writ_ast::Diagnostic::is_error),
        "should check cleanly: {diags:?}"
    );
    let out = writ_cli::run(&program).expect("run");
    let _ = std::fs::remove_dir_all(&dir);
    out
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
fn option_is_available_without_declaration() {
    let out = check_and_run(
        "fn describe(o: Option<Int>) -> Text {\n\
            return match o { Some(x) => \"some\", None => \"none\" };\n\
         }\n\
         fn main() { print(describe(Some(3))); print(describe(None)); }",
        "option",
    );
    assert_eq!(out, vec!["some", "none"]);
}

#[test]
fn result_is_available_without_declaration() {
    let out = check_and_run(
        "fn is_ok(r: Result<Int, Text>) -> Bool {\n\
            return match r { Ok(x) => true, Err(e) => false };\n\
         }\n\
         fn main() { print(is_ok(Ok(1))); print(is_ok(Err(\"bad\"))); }",
        "result",
    );
    assert_eq!(out, vec!["true", "false"]);
}

#[test]
fn a_user_type_of_the_same_name_shadows_the_prelude() {
    // Declaring your own `Option` must still work — no duplicate-definition error.
    let out = check_and_run(
        "type Option<T> = Some(T) | None\n\
         fn f(o: Option<Int>) -> Int { return match o { Some(x) => x, None => 0 }; }\n\
         fn main() { print(f(Some(5))); print(f(None)); }",
        "shadow",
    );
    assert_eq!(out, vec!["5", "0"]);
}

#[test]
fn prelude_types_compile_natively() {
    if !cc_available() {
        eprintln!("skipping native prelude test: no C compiler found");
        return;
    }
    let dir = scratch("native");
    let path = write_main(
        &dir,
        "fn main() {\n\
            print(match Some(7) { Some(x) => x, None => 0 });\n\
            print(match Err(\"e\") { Ok(x) => 1, Err(e) => 0 });\n\
         }",
    );
    let (program, mut diags) = writ_cli::load_program(&path);
    diags.extend(writ_cli::check(&program));
    assert!(
        !diags.iter().any(writ_ast::Diagnostic::is_error),
        "{diags:?}"
    );
    let bin = dir.join("prog");
    writ_cli::build(&program, &bin).expect("build");
    let out = Command::new(&bin).output().expect("run native");
    assert_eq!(String::from_utf8(out.stdout).unwrap(), "7\n0\n");
    let _ = std::fs::remove_dir_all(&dir);
}
