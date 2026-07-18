//! Hermetic, deterministic builds (#30): one command from source to binary,
//! and identical source produces byte-identical output — independent of *where*
//! the build runs. Building the same program in two different directories must
//! yield an identical `.c` and an identical binary.
//!
//! Needs a system C compiler; skipped (not failed) when none is present.

use std::path::PathBuf;
use std::process::Command;

const PROGRAM: &str = "\
type Option<T> = Some(T) | None
fn add(a: Int, b: Int) -> Int { return a + b; }
fn pick(o: Option<Int>) -> Int { return match o { Some(x) => x, None => 0 }; }
fn main() { print(add(2, 3)); print(pick(Some(9))); print(\"hi\"); }
";

fn cc() -> String {
    std::env::var("CC").unwrap_or_else(|_| "cc".to_string())
}

fn have_cc() -> bool {
    Command::new(cc())
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("writ_hermetic_{tag}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

/// Build `PROGRAM` inside a fresh directory; return the emitted C and binary
/// bytes.
fn build_in(tag: &str) -> (Vec<u8>, Vec<u8>) {
    let dir = scratch(tag);
    let src = dir.join("main.writ");
    std::fs::write(&src, PROGRAM).expect("write source");

    let (program, _) = writ_cli::load_program(&src);
    let bin = dir.join("out");
    let c_path = writ_cli::build(&program, &bin).expect("build");

    let c_bytes = std::fs::read(&c_path).expect("read .c");
    let bin_bytes = std::fs::read(&bin).expect("read binary");
    let _ = std::fs::remove_dir_all(&dir);
    (c_bytes, bin_bytes)
}

#[test]
fn identical_source_builds_byte_identical_output_regardless_of_location() {
    if !have_cc() {
        eprintln!("skipping hermetic test: no C compiler found");
        return;
    }
    let (c1, bin1) = build_in("a");
    let (c2, bin2) = build_in("b");

    assert_eq!(
        c1, c2,
        "emitted C must be byte-identical across build locations"
    );
    assert_eq!(
        bin1, bin2,
        "binary must be byte-identical across build locations (hermetic)"
    );
}
