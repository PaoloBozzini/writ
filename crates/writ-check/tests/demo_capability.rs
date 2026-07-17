//! Flagship capability demo (#24): generated code that touches the filesystem
//! without holding the file-write capability is rejected by the checker.
//!
//! This is the permanent, automated proof of "dangerous power is unreachable by
//! default".

use std::fs;
use std::path::PathBuf;

use writ_check::check_authority;

fn example_source(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples")
        .join(name);
    fs::read_to_string(&path).unwrap_or_else(|_| panic!("read {}", path.display()))
}

#[test]
fn filesystem_write_without_the_capability_is_rejected() {
    let src = example_source("reject_fs_write.writ");
    let parsed = writ_parser::parse(&src);
    assert!(
        parsed.diagnostics.is_empty(),
        "example should parse: {:?}",
        parsed.diagnostics
    );

    let diags = check_authority(&parsed.module);

    // Exactly one function — the one with no `Cap<Write>` — is rejected.
    assert_eq!(
        diags.len(),
        1,
        "expected one authority rejection, got {diags:?}"
    );
    let d = &diags[0];
    assert_eq!(d.code, "E0301");
    assert!(d.is_error());
    // A precise diagnostic: it names the effect, the missing capability, and the
    // offending function.
    assert!(
        d.message.contains("`Write`"),
        "names the effect: {}",
        d.message
    );
    assert!(
        d.message.contains("Cap<Write>"),
        "names the missing capability: {}",
        d.message
    );
    assert!(
        d.message.contains("`sneaky_write`"),
        "names the function: {}",
        d.message
    );

    // The effect site is the offending call, not the whole function.
    let call_start = src.rfind("write_file(contents, contents)").unwrap();
    assert_eq!(d.span.start, call_start);
}
