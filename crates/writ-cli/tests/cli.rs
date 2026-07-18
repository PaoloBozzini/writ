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
fn passes_run_independently() {
    use std::collections::BTreeMap;
    // This program has BOTH a type error (`1 + true`) and an authority error
    // (`bad` performs Write via `w` without holding `Cap<Write>`).
    let src = "\
fn w(out: Cap<Write>) uses { Write } { return; }
fn bad(x: Int) uses { Write } { w(x); let y = 1 + true; }
";
    let module = writ_parser::parse(src).module;
    let mut modules = BTreeMap::new();
    modules.insert("main".to_string(), module);
    let program = writ_cli::Program {
        modules,
        root: "main".to_string(),
    };

    // Only the type pass: reports the type error, not the authority error.
    let types_only = writ_cli::check_passes(&program, &["types".to_string()]);
    assert!(types_only.iter().any(|d| d.code == "T0001"));
    assert!(
        types_only.iter().all(|d| d.code != "E0301"),
        "authority pass must not run"
    );

    // Only the authority pass: reports the authority error, not the type error.
    let authority_only = writ_cli::check_passes(&program, &["authority".to_string()]);
    assert!(authority_only.iter().any(|d| d.code == "E0301"));
    assert!(
        authority_only.iter().all(|d| d.code != "T0001"),
        "type pass must not run"
    );
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
