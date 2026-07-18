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
fn text_is_refused_for_now() {
    let m = lower_src(r#"fn main() { print("hi"); }"#);
    let err = emit_c(&m).expect_err("text not supported yet");
    assert!(err.message.contains("text"), "{}", err.message);
}

#[test]
fn match_is_refused_for_now() {
    let m = lower_src(
        "\
type Option = Some(Int) | None
fn f(o: Option) -> Int { return match o { Some(x) => x, None => 0 }; }
fn main() { print(f(None)); }
",
    );
    let err = emit_c(&m).expect_err("match not supported yet");
    assert!(
        err.message.contains("match") || err.message.contains("constructor"),
        "{}",
        err.message
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
