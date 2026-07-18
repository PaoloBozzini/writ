//! File I/O via capabilities (#123): the first genuinely effectful built-ins.
//! A write-then-read round-trip must behave identically in the interpreter and
//! the native binary, and reaching the filesystem without declaring the effect
//! (honesty) or holding the capability (authority) must be refused statically.

use std::path::{Path, PathBuf};
use std::process::Command;

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("writ_io_{tag}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

/// A program that writes a file then reads it back and prints it. The path is
/// absolute so the interpreter (in-process) and the native binary (a subprocess)
/// touch the same file.
fn roundtrip_source(path: &Path) -> String {
    format!(
        "fn main(root: Cap<Root>) uses {{ Read, Write }} {{\n\
            write_file(grant<Write>(root), \"{p}\", \"hello from writ\");\n\
            print(read_file(grant<Read>(root), \"{p}\"));\n\
         }}",
        p = path.display()
    )
}

fn cc_available() -> bool {
    let cc = std::env::var("CC").unwrap_or_else(|_| "cc".to_string());
    Command::new(cc)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn check_codes(src: &Path) -> Vec<String> {
    let (program, mut diags) = writ_cli::load_program(src);
    diags.extend(writ_cli::check(&program));
    diags.into_iter().map(|d| d.code).collect()
}

fn write_program(dir: &Path, source: &str) -> PathBuf {
    let src = dir.join("main.writ");
    std::fs::write(&src, source).unwrap();
    src
}

#[test]
fn interpreter_round_trips_a_file() {
    let dir = scratch("interp");
    let data = dir.join("data.txt");
    let src = write_program(&dir, &roundtrip_source(&data));

    let (program, mut diags) = writ_cli::load_program(&src);
    diags.extend(writ_cli::check(&program));
    assert!(
        !diags.iter().any(writ_ast::Diagnostic::is_error),
        "should check cleanly: {diags:?}"
    );
    let out = writ_cli::run(&program).expect("run");
    assert_eq!(out, vec!["hello from writ".to_string()]);
    assert_eq!(std::fs::read_to_string(&data).unwrap(), "hello from writ");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn native_binary_round_trips_a_file_like_the_interpreter() {
    if !cc_available() {
        eprintln!("skipping native file-io test: no C compiler found");
        return;
    }
    let dir = scratch("native");
    let data = dir.join("data.txt");
    let src = write_program(&dir, &roundtrip_source(&data));

    let (program, _) = writ_cli::load_program(&src);
    let bin = dir.join("prog");
    writ_cli::build(&program, &bin).expect("build");
    let out = Command::new(&bin).output().expect("run native");
    assert!(out.status.success());
    assert_eq!(String::from_utf8(out.stdout).unwrap(), "hello from writ\n");
    assert_eq!(std::fs::read_to_string(&data).unwrap(), "hello from writ");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn reaching_the_filesystem_without_declaring_the_effect_is_refused() {
    // Holds `Cap<Read>` (authorized) but the signature omits `uses { Read }`.
    let dir = scratch("honesty");
    let src = write_program(
        &dir,
        "fn reader(c: Cap<Read>) -> Text { return read_file(c, \"x\"); }\nfn main() {}",
    );
    let codes = check_codes(&src);
    assert!(
        codes.contains(&"E0101".to_string()),
        "honesty (E0101): {codes:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn reaching_the_filesystem_without_the_capability_is_refused() {
    // Declares `uses { Read }` (honest) but holds only a `Cap<Write>` — no
    // `Cap<Read>`, so the authority check refuses the effect site.
    let dir = scratch("authority");
    let src = write_program(
        &dir,
        "fn reader(c: Cap<Write>) -> Text uses { Read } { return read_file(c, \"x\"); }\nfn main() {}",
    );
    let codes = check_codes(&src);
    assert!(
        codes.contains(&"E0301".to_string()),
        "authority (E0301): {codes:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
