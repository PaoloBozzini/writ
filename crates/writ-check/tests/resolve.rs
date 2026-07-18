//! Module-resolver tests: qualified names + visibility across modules.

use std::collections::BTreeMap;

use writ_check::check_resolution;

/// Assemble a program from `(module_name, source)` pairs and resolve it.
fn resolve(sources: &[(&str, &str)]) -> Vec<String> {
    let mut modules = BTreeMap::new();
    for (name, src) in sources {
        let parsed = writ_parser::parse(src);
        assert!(
            parsed.diagnostics.is_empty(),
            "`{name}` should parse: {:?}",
            parsed.diagnostics
        );
        modules.insert((*name).to_string(), parsed.module);
    }
    check_resolution(&modules)
        .into_iter()
        .map(|d| d.code)
        .collect()
}

#[test]
fn exported_item_resolves_across_modules() {
    assert!(resolve(&[
        (
            "math",
            "export fn add(a: Int, b: Int) -> Int { return a + b; }"
        ),
        (
            "main",
            "import math\nfn main() -> Int { return math.add(1, 2); }"
        ),
    ])
    .is_empty());
}

// --- Negative tests: the rejection is the feature. ------------------------

#[test]
fn using_a_private_item_across_modules_is_rejected() {
    // `helper` exists in `math` but is not exported.
    let cs = resolve(&[
        ("math", "fn helper() -> Int { return 0; }"),
        (
            "main",
            "import math\nfn main() -> Int { return math.helper(); }",
        ),
    ]);
    assert_eq!(cs, vec!["R0004"], "private item must be refused");
}

#[test]
fn importing_an_unknown_module_is_rejected() {
    let cs = resolve(&[("main", "import nope\nfn main() -> Int { return 1; }")]);
    assert_eq!(cs, vec!["R0001"]);
}

#[test]
fn member_of_an_unimported_module_is_rejected() {
    let cs = resolve(&[
        (
            "math",
            "export fn add(a: Int, b: Int) -> Int { return a + b; }",
        ),
        // `main` never imports `math`.
        ("main", "fn main() -> Int { return math.add(1, 2); }"),
    ]);
    assert_eq!(cs, vec!["R0002"]);
}

#[test]
fn unknown_member_name_is_rejected() {
    let cs = resolve(&[
        (
            "math",
            "export fn add(a: Int, b: Int) -> Int { return a + b; }",
        ),
        (
            "main",
            "import math\nfn main() -> Int { return math.subtract(1, 2); }",
        ),
    ]);
    assert_eq!(cs, vec!["R0003"]);
}

#[test]
fn import_cycles_are_detected() {
    let cs = resolve(&[
        ("a", "import b\nexport fn f() -> Int { return 1; }"),
        ("b", "import a\nexport fn g() -> Int { return 2; }"),
    ]);
    // Both edges of the 2-cycle are flagged.
    assert!(cs.iter().all(|c| c == "R0005"), "{cs:?}");
    assert_eq!(cs.len(), 2);
}

#[test]
fn diagnostics_are_deterministic() {
    let program = [
        ("math", "fn helper() -> Int { return 0; }"),
        (
            "main",
            "import math\nfn main() -> Int { return math.helper(); }",
        ),
    ];
    assert_eq!(resolve(&program), resolve(&program));
}
