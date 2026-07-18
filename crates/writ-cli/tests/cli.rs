//! Driver tests: multi-file loading, checking, and running (#31 / #83).

use std::path::PathBuf;

fn example(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples")
        .join(rel)
}

#[test]
fn multi_file_program_checks_and_runs_end_to_end() {
    let (program, load_diags) = writ_cli::load_program(&example("modules/app.writ"));
    assert!(load_diags.is_empty(), "load: {load_diags:?}");
    // Both modules were loaded from disk.
    assert!(program.modules.contains_key("app"));
    assert!(program.modules.contains_key("math"));

    let diags = writ_cli::check(&program);
    assert!(diags.is_empty(), "check: {diags:?}");

    // `main` calls `math.add(2, 3)` across the module boundary.
    let output = writ_cli::run(&program).unwrap();
    assert_eq!(output, vec!["5".to_string()]);
}

#[test]
fn a_missing_imported_file_is_a_clean_diagnostic_not_a_panic() {
    // A root file that imports a module whose file does not exist. Written to a
    // temp dir so nothing touches the repo.
    let broken = std::env::temp_dir().join("writ_cli_missing_import_test.writ");
    std::fs::write(&broken, "import ghost_module\nfn main() { return; }\n").unwrap();

    let (_program, load_diags) = writ_cli::load_program(&broken);
    let _ = std::fs::remove_file(&broken);

    assert!(
        load_diags.iter().any(|d| d.code == "D0001"),
        "expected a clean missing-module diagnostic, got {load_diags:?}"
    );
}
