//! Taint-pass tests: untrusted data cannot reach a sink without `sanitize`.

use writ_check::{check_taint, check_types};

fn taint_codes(src: &str) -> Vec<String> {
    let parsed = writ_parser::parse(src);
    assert!(
        parsed.diagnostics.is_empty(),
        "source should parse: {:?}",
        parsed.diagnostics
    );
    check_taint(&parsed.module)
        .into_iter()
        .map(|d| d.code)
        .collect()
}

const PROGRAM: &str = "\
type Option<T> = Some(T) | None
fn ok(s: Text) -> Bool { return true; }
fn run_query(q: Text) uses { Query } { return; }
fn nothing() { return; }
fn handle(input: Tainted<Text>) uses { Query } {
    match sanitize(input, ok) {
        Some(clean) => run_query(clean),
        None        => nothing(),
    };
}
";

#[test]
fn sanitized_data_may_reach_a_sink() {
    assert!(
        taint_codes(PROGRAM).is_empty(),
        "{:?}",
        taint_codes(PROGRAM)
    );
}

#[test]
fn sanitize_type_checks_as_a_validated_boundary() {
    // `sanitize(Tainted<Text>, fn(Text) -> Bool) -> Option<Text>`: the `Some`
    // payload is trusted `Text`, which the sink `run_query` accepts.
    let parsed = writ_parser::parse(PROGRAM);
    assert!(
        check_types(&parsed.module).is_empty(),
        "{:?}",
        check_types(&parsed.module)
    );
}

#[test]
fn trusted_data_reaching_a_sink_is_fine() {
    assert!(taint_codes(
        "\
fn run_query(q: Text) uses { Query } { return; }
fn handle(q: Text) uses { Query } { run_query(q); }
"
    )
    .is_empty());
}

// --- Negative tests: tainted data reaching a sink is the rejection. --------

#[test]
fn tainted_value_reaching_a_sink_is_rejected() {
    let cs = taint_codes(
        "\
fn run_query(q: Text) uses { Query } { return; }
fn handle(input: Tainted<Text>) uses { Query } {
    run_query(input);
}
",
    );
    assert_eq!(cs, vec!["E0401"]);
}

#[test]
fn taint_flows_through_a_let_binding() {
    let cs = taint_codes(
        "\
fn shell(cmd: Text) uses { Shell } { return; }
fn handle(input: Tainted<Text>) uses { Shell } {
    let c = input;
    shell(c);
}
",
    );
    assert_eq!(
        cs,
        vec!["E0401"],
        "taint should propagate through `let c = input`"
    );
}

#[test]
fn a_non_sink_call_does_not_flag_tainted_arguments() {
    // `log` is not a sink (no Query/Shell effect), so passing tainted data is
    // not a taint violation here.
    assert!(taint_codes(
        "\
fn log(m: Text) { return; }
fn handle(input: Tainted<Text>) { log(input); }
"
    )
    .is_empty());
}

// --- Laundering via compound expressions (#98) ----------------------------

#[test]
fn a_match_wrapper_does_not_launder_taint() {
    // Wrapping a tainted value in a `match` must not defeat E0401.
    let cs = taint_codes(
        "\
fn run_query(q: Tainted<Text>) uses { Query } { return; }
fn handle(input: Tainted<Text>) uses { Query } {
    run_query(match true { _ => input });
}
",
    );
    assert_eq!(cs, vec!["E0401"], "match must not launder taint");
}

#[test]
fn a_let_bound_match_wrapper_stays_tainted() {
    // `let x = match ... { _ => tainted }` must mark `x` tainted.
    let cs = taint_codes(
        "\
fn run_query(q: Tainted<Text>) uses { Query } { return; }
fn handle(input: Tainted<Text>) uses { Query } {
    let x = match true { _ => input };
    run_query(x);
}
",
    );
    assert_eq!(cs, vec!["E0401"], "taint must survive a let-bound match");
}

#[test]
fn a_binary_wrapper_does_not_launder_taint() {
    // An operator over a tainted operand keeps the result tainted.
    let cs = taint_codes(
        "\
fn run_query(q: Tainted<Bool>) uses { Query } { return; }
fn handle(input: Tainted<Int>) uses { Query } {
    run_query(input == input);
}
",
    );
    assert_eq!(cs, vec!["E0401"], "operators must not launder taint");
}
