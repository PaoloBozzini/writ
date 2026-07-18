//! Text built-ins (#122): `concat`, `text_len`, `char_at`, `substring`.
//! `Text` is a sequence of Unicode scalar values, so indexing is char-based;
//! out-of-range access is a runtime error.

use writ_interp::{run, Value};

fn eval(body: &str) -> Value {
    let src = format!("fn f() -> Text {{ {body} }}");
    let m = writ_parser::parse(&src);
    assert!(m.diagnostics.is_empty(), "{:?}", m.diagnostics);
    run(&m.module, "f", vec![]).expect("no runtime error")
}

fn eval_int(body: &str) -> Value {
    let src = format!("fn f() -> Int {{ {body} }}");
    let m = writ_parser::parse(&src);
    assert!(m.diagnostics.is_empty(), "{:?}", m.diagnostics);
    run(&m.module, "f", vec![]).expect("no runtime error")
}

fn err(body: &str) -> String {
    let src = format!("fn f() -> Text {{ {body} }}");
    let m = writ_parser::parse(&src);
    assert!(m.diagnostics.is_empty(), "{:?}", m.diagnostics);
    run(&m.module, "f", vec![]).unwrap_err().message
}

#[test]
fn concat_joins_text() {
    assert_eq!(
        eval(r#"return concat("foo", "bar");"#),
        Value::Text("foobar".into())
    );
}

#[test]
fn text_len_counts_unicode_scalars() {
    assert_eq!(eval_int(r#"return text_len("abc");"#), Value::Int(3));
    // "héllo" is five scalar values (é is one, though two bytes).
    assert_eq!(eval_int(r#"return text_len("héllo");"#), Value::Int(5));
}

#[test]
fn char_at_returns_the_nth_character() {
    assert_eq!(
        eval(r#"return char_at("héllo", 0);"#),
        Value::Text("h".into())
    );
    assert_eq!(
        eval(r#"return char_at("héllo", 1);"#),
        Value::Text("é".into())
    );
}

#[test]
fn substring_slices_by_character() {
    assert_eq!(
        eval(r#"return substring("héllo", 0, 2);"#),
        Value::Text("hé".into())
    );
    assert_eq!(
        eval(r#"return substring("héllo", 1, 5);"#),
        Value::Text("éllo".into())
    );
    assert_eq!(
        eval(r#"return substring("abc", 1, 1);"#),
        Value::Text("".into())
    );
}

#[test]
fn char_at_out_of_range_is_a_runtime_error() {
    assert!(err(r#"return char_at("ab", 5);"#).contains("out of range"));
    assert!(err(r#"return char_at("ab", -1);"#).contains("out of range"));
}

#[test]
fn substring_out_of_bounds_is_a_runtime_error() {
    assert!(err(r#"return substring("ab", 0, 9);"#).contains("out of bounds"));
    assert!(err(r#"return substring("ab", 2, 1);"#).contains("out of bounds"));
}

#[test]
fn char_code_and_code_char_round_trip() {
    assert_eq!(eval_int(r#"return char_code("A");"#), Value::Int(65));
    assert_eq!(eval_int(r#"return char_code("0");"#), Value::Int(48));
    // Non-ASCII: é is U+00E9 = 233.
    assert_eq!(eval_int(r#"return char_code("é");"#), Value::Int(233));
    assert_eq!(eval(r#"return code_char(97);"#), Value::Text("a".into()));
    assert_eq!(eval(r#"return code_char(233);"#), Value::Text("é".into()));
    assert_eq!(
        eval(r#"return code_char(char_code("Z"));"#),
        Value::Text("Z".into())
    );
}

#[test]
fn char_code_of_empty_text_is_a_runtime_error() {
    assert!(err(r#"return code_char(char_code(""));"#).contains("empty"));
}

#[test]
fn code_char_of_a_non_scalar_is_a_runtime_error() {
    // 0xD800 is a surrogate — not a Unicode scalar value.
    assert!(err(r#"return code_char(55296);"#).contains("scalar"));
    assert!(err(r#"return code_char(-1);"#).contains("scalar"));
}
