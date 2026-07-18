//! Codegen unit tests: the core language emits, and constructs the back end
//! does not support yet are **refused** with a `CodegenError` rather than
//! silently mis-compiled.

use writ_codegen::emit_c;

fn lower_src(src: &str) -> writ_ast::Module {
    let parsed = writ_parser::parse(src);
    assert!(
        parsed.diagnostics.is_empty(),
        "source should parse: {:?}",
        parsed.diagnostics
    );
    writ_lower::lower(&parsed.module)
}

#[test]
fn core_program_emits_a_c_main_and_function() {
    let m = lower_src(
        "fn add(a: Int, b: Int) -> Int { return a + b; }\nfn main() { print(add(1, 2)); }",
    );
    let c = emit_c(&m).expect("core program emits");
    assert!(c.contains("int main(void)"), "has a C entry point");
    assert!(c.contains("wf_add"), "emits the user function");
    assert!(c.contains("w_add"), "uses the checked-add helper");
}

#[test]
fn a_program_without_main_is_refused() {
    let m = lower_src("fn helper() -> Int { return 1; }");
    let err = emit_c(&m).expect_err("no main → error");
    assert!(err.message.contains("main"), "{}", err.message);
}

#[test]
fn text_emits_an_escaped_c_literal() {
    let m = lower_src(r#"fn main() { print("a\"b"); }"#);
    let c = emit_c(&m).expect("text emits");
    // The embedded quote is escaped for the C string literal.
    assert!(c.contains(r#"w_text("a\"b")"#), "escaped text literal: {c}");
}

#[test]
fn sum_type_and_match_emit() {
    let m = lower_src(
        "\
type Option<T> = Some(T) | None
fn f(o: Option<Int>) -> Int { return match o { Some(x) => x, None => 0 }; }
fn main() { print(f(Some(5))); print(f(None)); }
",
    );
    let c = emit_c(&m).expect("sum types + match emit");
    assert!(c.contains("w_variant(\"Some\""), "constructor call: {c}");
    assert!(c.contains("w_is("), "match tests the variant tag: {c}");
}

#[test]
fn nested_subpatterns_emit_a_folded_condition() {
    // `Some(Some(x))` tests both tags and reads the binding two levels deep.
    let m = lower_src(
        "\
type Option<T> = Some(T) | None
fn f(o: Option<Option<Int>>) -> Int {
    return match o { Some(Some(x)) => x, Some(None) => 0, None => 0 };
}
fn main() { print(f(None)); }
",
    );
    let c = emit_c(&m).expect("nested sub-patterns emit");
    // The outer and inner tags are both tested, and the binding reaches the
    // inner field.
    assert!(
        c.contains("w_is(") && c.contains("&&"),
        "folded tag tests: {c}"
    );
    assert!(
        c.contains(".fields[0].fields[0]"),
        "reads the nested field: {c}"
    );
}

#[test]
fn capabilities_emit() {
    // `grant<A>(cap)` becomes a tagged capability; a `Cap` main parameter is
    // handed the root capability by the C entry point.
    let m = lower_src(
        "\
fn write_line(out: Cap<Write>, msg: Text) uses { Write } { return; }
fn main(root: Cap<Root>) uses { Write } { write_line(grant<Write>(root), \"hi\"); }
",
    );
    let c = emit_c(&m).expect("capabilities emit");
    assert!(
        c.contains("w_cap(\"Write\")"),
        "grant narrows to a tagged cap: {c}"
    );
    assert!(
        c.contains("w_cap(\"Root\")"),
        "main receives the root capability: {c}"
    );
}

#[test]
fn emission_is_deterministic() {
    // Codegen is a pure function of the AST: identical input → identical bytes.
    // (Guards against hash-map iteration order or other nondeterminism leaking
    // into output — the foundation of #30's byte-identical builds.)
    let src = "\
type Option<T> = Some(T) | None
fn add(a: Int, b: Int) -> Int { return a + b; }
fn pick(o: Option<Int>) -> Int { return match o { Some(x) => x, None => 0 }; }
fn main() { print(add(1, 2)); print(pick(Some(3))); print(\"hi\"); }
";
    let m = lower_src(src);
    assert_eq!(
        emit_c(&m).unwrap(),
        emit_c(&m).unwrap(),
        "emission must be stable"
    );
}

#[test]
fn a_lowered_contract_emits_a_trap() {
    // After lowering, `ensures` is a `Check`; codegen turns it into a trap that
    // carries the interpreter's exact blame message.
    let m = lower_src(
        "fn f(n: Int) -> Int ensures result >= 0 { return n; }\nfn main() { print(f(1)); }",
    );
    let c = emit_c(&m).expect("emits");
    assert!(
        c.contains("postcondition violated (blame: implementation)"),
        "the lowered ensures becomes a blamed trap"
    );
}

#[test]
fn higher_order_functions_emit_function_pointers() {
    let m = lower_src(
        "fn apply(g: fn(Int) -> Int, x: Int) -> Int { return g(x); }\n\
         fn inc(n: Int) -> Int { return n + 1; }\n\
         fn main() { print(apply(inc, 5)); }",
    );
    let c = emit_c(&m).expect("HOF emits");
    // `inc` passed as a value becomes a tagged function pointer...
    assert!(c.contains("w_fn((WFn)wf_inc"), "function value: {c}");
    // ...and `g(x)` inside `apply` calls through the pointer with an arity cast.
    assert!(c.contains("(WValue(*)(WValue))"), "indirect call: {c}");
}
