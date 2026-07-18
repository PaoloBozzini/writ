//! Check-time example programs — ones that demonstrate a *static* property
//! rather than producing output. These must check cleanly (or, where they show a
//! rejection, they carry the rejected case in a comment so the file itself stays
//! clean).

use std::path::PathBuf;

fn example(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples")
        .join(name)
}

#[test]
fn sanitize_example_checks_cleanly() {
    let (program, mut diags) = writ_cli::load_program(&example("sanitize.writ"));
    diags.extend(writ_cli::check(&program));
    assert!(
        !diags.iter().any(writ_ast::Diagnostic::is_error),
        "sanitize.writ should check cleanly: {diags:?}"
    );
}
