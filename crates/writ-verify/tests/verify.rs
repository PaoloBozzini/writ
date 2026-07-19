//! Tests for SMT-backed verification. Query *generation* is pure and fully
//! exercised here; the `verify` decision logic is driven by a mock solver so it
//! is deterministic without a solver installed. A final test uses real `z3` when
//! it is available, and is skipped otherwise.

use writ_ast::{Function, Item, Module};
use writ_verify::{function_queries, verify, Answer, Solver, Z3Cli};

fn parse(src: &str) -> Module {
    let parsed = writ_parser::parse(src);
    assert!(
        parsed.diagnostics.is_empty(),
        "source should parse: {:?}",
        parsed.diagnostics
    );
    parsed.module
}

fn first_fn(m: &Module) -> &Function {
    m.items
        .iter()
        .find_map(|i| match i {
            Item::Function(f) => Some(f),
            Item::Type(_) => None,
        })
        .expect("a function")
}

/// A solver with a fixed verdict, for testing the pass logic deterministically.
struct Mock {
    answer: Answer,
    available: bool,
}

impl Solver for Mock {
    fn available(&self) -> bool {
        self.available
    }
    fn solve(&self, _script: &str) -> Answer {
        self.answer
    }
}

fn mock(answer: Answer) -> Mock {
    Mock {
        answer,
        available: true,
    }
}

// --- Query generation (pure) ----------------------------------------------

#[test]
fn generates_one_query_per_ensures_clause() {
    let m = parse(
        "fn max(a: Int, b: Int) -> Int ensures result >= a ensures result >= b {\n\
            if a > b { return a; }\n\
            return b;\n\
         }",
    );
    let qs = function_queries(first_fn(&m));
    assert_eq!(qs.len(), 2, "one query per ensures");
    let s = &qs[0].script;
    assert!(s.contains("(declare-const a Int)"), "declares params: {s}");
    assert!(s.contains("(ite "), "models the if as an ite: {s}");
    assert!(s.contains("(assert (not "), "asserts the negated goal: {s}");
    assert!(
        s.trim_end().ends_with("(check-sat)"),
        "ends with check-sat: {s}"
    );
}

#[test]
fn requires_becomes_an_assumption() {
    let m = parse("fn half(n: Int) -> Int requires n > 0 ensures result >= 0 { return n; }");
    let qs = function_queries(first_fn(&m));
    assert_eq!(qs.len(), 1);
    assert!(
        qs[0].script.contains("(assert (> n 0))"),
        "requires is assumed: {}",
        qs[0].script
    );
}

#[test]
fn a_function_with_no_ensures_yields_no_queries() {
    let m = parse("fn f(n: Int) -> Int requires n > 0 { return n; }");
    assert!(function_queries(first_fn(&m)).is_empty());
}

#[test]
fn division_leaves_the_supported_fragment() {
    // SMT integer division differs from Writ's truncation, so `/` is excluded;
    // the function produces no queries (and is reported as unverified upstream).
    let m = parse("fn f(n: Int) -> Int ensures result >= 0 { return n / 2; }");
    assert!(function_queries(first_fn(&m)).is_empty());
}

#[test]
fn non_integer_parameters_leave_the_fragment() {
    let m = parse(
        "type Option<T> = Some(T) | None\n\
         fn f(o: Option<Int>) -> Int ensures result >= 0 { return 0; }",
    );
    assert!(function_queries(first_fn(&m)).is_empty());
}

// --- Pass logic (mock solver) ---------------------------------------------

#[test]
fn a_proved_contract_produces_no_diagnostic() {
    let m = parse("fn f(n: Int) -> Int requires n > 0 ensures result >= 0 { return n; }");
    assert!(verify(&m, &mock(Answer::Unsat)).is_empty());
}

#[test]
fn a_refuted_contract_is_reported_as_v0002() {
    let m = parse("fn f(n: Int) -> Int ensures result >= 0 { return n; }");
    let diags = verify(&m, &mock(Answer::Sat));
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].code, "V0002");
    assert!(
        !diags[0].is_error(),
        "verification never blocks (warning only)"
    );
}

#[test]
fn an_undecided_contract_is_reported_as_v0003() {
    let m = parse("fn f(n: Int) -> Int ensures result >= 0 { return n; }");
    let diags = verify(&m, &mock(Answer::Unknown));
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].code, "V0003");
}

#[test]
fn an_unsupported_contract_is_reported_as_v0001() {
    // Division is outside the fragment → the function is flagged, not assumed.
    let m = parse("fn f(n: Int) -> Int ensures result >= 0 { return n / 2; }");
    let diags = verify(&m, &mock(Answer::Unsat));
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].code, "V0001");
}

#[test]
fn an_unavailable_solver_skips_verification() {
    let m = parse("fn f(n: Int) -> Int ensures result >= 0 { return n; }");
    let unavailable = Mock {
        answer: Answer::Sat,
        available: false,
    };
    assert!(
        verify(&m, &unavailable).is_empty(),
        "no solver → skip, never block"
    );
}

// --- Real solver (skipped without z3) -------------------------------------

#[test]
fn z3_proves_a_true_postcondition_and_refutes_a_false_one() {
    let z3 = Z3Cli;
    if !z3.available() {
        eprintln!("skipping z3 test: no solver found");
        return;
    }
    // True: max(a,b) >= a on both branches.
    let good = parse(
        "fn max(a: Int, b: Int) -> Int ensures result >= a {\n\
            if a > b { return a; }\n\
            return b;\n\
         }",
    );
    assert!(
        verify(&good, &z3).is_empty(),
        "a true postcondition is proved"
    );

    // False: claims result > n but returns n.
    let bad = parse("fn f(n: Int) -> Int ensures result > n { return n; }");
    let diags = verify(&bad, &z3);
    assert_eq!(diags.len(), 1, "a false postcondition is caught");
    assert_eq!(diags[0].code, "V0002");
}

// --- Single availability probe (#157) --------------------------------------

/// A solver that counts how many times its availability was probed.
struct CountingSolver {
    available: bool,
    probes: std::cell::Cell<usize>,
}

impl Solver for CountingSolver {
    fn available(&self) -> bool {
        self.probes.set(self.probes.get() + 1);
        self.available
    }
    fn solve(&self, _script: &str) -> Answer {
        Answer::Unsat
    }
}

#[test]
fn verify_reporting_availability_probes_the_solver_once() {
    let m = parse(
        "fn max(a: Int, b: Int) -> Int ensures result >= a { if a > b { return a; } return b; }",
    );
    let solver = CountingSolver {
        available: true,
        probes: std::cell::Cell::new(0),
    };
    let (diags, available) = writ_verify::verify_reporting_availability(&m, &solver);
    assert!(available);
    assert!(diags.is_empty());
    assert_eq!(
        solver.probes.get(),
        1,
        "availability must be probed exactly once"
    );
}

#[test]
fn verify_reporting_availability_skips_when_unavailable() {
    let m = parse("fn f(n: Int) -> Int ensures result > n { return n; }");
    let solver = CountingSolver {
        available: false,
        probes: std::cell::Cell::new(0),
    };
    let (diags, available) = writ_verify::verify_reporting_availability(&m, &solver);
    assert!(!available);
    assert!(
        diags.is_empty(),
        "no verification when the solver is unavailable"
    );
    assert_eq!(solver.probes.get(), 1);
}
