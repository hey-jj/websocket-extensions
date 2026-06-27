//! Parser conformance: parse_header and serialize_params.

use websocket_extensions::parser::{parse_header, serialize_params, Params, Slot, Value};

/// Build a `Params` from a list of (key, value) pairs, preserving order and
/// promoting repeated keys to lists.
fn params(pairs: &[(&str, Value)]) -> Params {
    let mut p = Params::new();
    for (k, v) in pairs {
        p.insert(k, v.clone());
    }
    p
}

fn num(n: f64) -> Value {
    Value::Number(n)
}

fn s(text: &str) -> Value {
    Value::Str(text.to_string())
}

fn flag() -> Value {
    Value::Bool(true)
}

/// Assert that parsing `input` yields the given (name, params) offers in order.
fn assert_parses(input: Option<&str>, expected: &[(&str, Params)]) {
    let offers = parse_header(input).expect("expected a valid header");
    let got = offers.to_vec();
    assert_eq!(got.len(), expected.len(), "offer count for {:?}", input);
    for (offer, (name, params)) in got.iter().zip(expected) {
        assert_eq!(&offer.name, name, "name for {:?}", input);
        assert_eq!(&offer.params, params, "params for {:?}", input);
    }
}

#[test]
fn parses_an_empty_header() {
    assert_parses(Some(""), &[]);
}

#[test]
fn parses_a_missing_header() {
    assert_parses(None, &[]);
}

#[test]
fn throws_on_invalid_input() {
    assert!(parse_header(Some("a,")).is_err());
}

#[test]
fn parses_one_offer_with_no_params() {
    assert_parses(Some("a"), &[("a", Params::new())]);
}

#[test]
fn parses_two_offers_with_no_params() {
    assert_parses(Some("a, b"), &[("a", Params::new()), ("b", Params::new())]);
}

#[test]
fn parses_a_duplicate_offer_name() {
    assert_parses(Some("a, a"), &[("a", Params::new()), ("a", Params::new())]);
}

#[test]
fn parses_a_flag() {
    assert_parses(Some("a; b"), &[("a", params(&[("b", flag())]))]);
}

#[test]
fn parses_an_unquoted_param() {
    assert_parses(Some("a; b=1"), &[("a", params(&[("b", num(1.0))]))]);
}

#[test]
fn parses_a_quoted_param() {
    // Input: a; b="hi, \"there"  -> value: hi, "there (backslashes stripped).
    assert_parses(
        Some("a; b=\"hi, \\\"there\""),
        &[("a", params(&[("b", s("hi, \"there"))]))],
    );
}

#[test]
fn parses_multiple_params() {
    assert_parses(
        Some("a; b; c=1; d=\"hi\""),
        &[("a", params(&[("b", flag()), ("c", num(1.0)), ("d", s("hi"))]))],
    );
}

#[test]
fn parses_duplicate_params() {
    // b appears twice: flag then "hi" -> list [true, "hi"].
    let mut p = Params::new();
    p.insert("b", flag());
    p.insert("c", num(1.0));
    p.insert("b", s("hi"));
    assert_parses(Some("a; b; c=1; b=\"hi\""), &[("a", p)]);
    // Confirm b is a two-element list, c remains scalar.
    let offers = parse_header(Some("a; b; c=1; b=\"hi\"")).unwrap();
    let offer = &offers.to_vec()[0];
    assert_eq!(
        offer.params.get("b"),
        Some(&Slot::Many(vec![flag(), s("hi")]))
    );
    assert_eq!(offer.params.get("c"), Some(&Slot::One(num(1.0))));
}

#[test]
fn parses_multiple_complex_offers() {
    let offers =
        parse_header(Some("a; b=1, c, b; d, c; e=\"hi, there\"; e, a; b")).unwrap();
    let got = offers.to_vec();
    assert_eq!(got.len(), 5);

    assert_eq!(got[0].name, "a");
    assert_eq!(got[0].params.get("b"), Some(&Slot::One(num(1.0))));

    assert_eq!(got[1].name, "c");
    assert!(got[1].params.is_empty());

    assert_eq!(got[2].name, "b");
    assert_eq!(got[2].params.get("d"), Some(&Slot::One(flag())));

    assert_eq!(got[3].name, "c");
    assert_eq!(
        got[3].params.get("e"),
        Some(&Slot::Many(vec![s("hi, there"), flag()]))
    );

    assert_eq!(got[4].name, "a");
    assert_eq!(got[4].params.get("b"), Some(&Slot::One(flag())));
}

#[test]
fn parses_an_extension_name_that_shadows_an_object_property() {
    assert_parses(Some("hasOwnProperty"), &[("hasOwnProperty", Params::new())]);
}

#[test]
fn parses_an_extension_param_that_shadows_an_object_property() {
    let offers = parse_header(Some("foo; hasOwnProperty; x")).unwrap();
    let offer = &offers.to_vec()[0];
    assert_eq!(offer.params.get("hasOwnProperty"), Some(&Slot::One(flag())));
}

#[test]
fn rejects_a_string_missing_its_closing_quote() {
    // 31 escape pairs with no closing quote. Must reject, and fast.
    let mut header = String::from("foo; bar=\"fooa");
    for _ in 0..31 {
        header.push_str("\\a");
    }
    let start = std::time::Instant::now();
    assert!(parse_header(Some(&header)).is_err());
    assert!(
        start.elapsed().as_millis() < 50,
        "parse should fail fast, took {:?}",
        start.elapsed()
    );
}

// serialize_params

#[test]
fn serializes_empty_params() {
    assert_eq!(serialize_params("a", &Params::new()), "a");
}

#[test]
fn serializes_a_flag() {
    assert_eq!(serialize_params("a", &params(&[("b", flag())])), "a; b");
}

#[test]
fn serializes_an_unquoted_param() {
    // A token-safe string stays unquoted.
    assert_eq!(serialize_params("a", &params(&[("b", s("42"))])), "a; b=42");
}

#[test]
fn serializes_a_quoted_param() {
    assert_eq!(
        serialize_params("a", &params(&[("b", s("hi, there"))])),
        "a; b=\"hi, there\""
    );
}

#[test]
fn serializes_multiple_params() {
    assert_eq!(
        serialize_params(
            "a",
            &params(&[("b", flag()), ("c", num(1.0)), ("d", s("hi"))])
        ),
        "a; b; c=1; d=hi"
    );
}

#[test]
fn serializes_duplicate_params() {
    let mut p = Params::new();
    p.insert("b", flag());
    p.insert("b", s("hi"));
    p.insert("c", num(1.0));
    assert_eq!(serialize_params("a", &p), "a; b; b=hi; c=1");
}

// Added: pin the exact NUMBER grammar (TESTS.md section 5.7).

#[test]
fn number_grammar_edge_cases() {
    // Leading zeros are not numbers.
    let offers = parse_header(Some("a; b=01; c=00")).unwrap();
    let offer = &offers.to_vec()[0];
    assert_eq!(offer.params.get("b"), Some(&Slot::One(s("01"))));
    assert_eq!(offer.params.get("c"), Some(&Slot::One(s("00"))));

    // Bare 0 and 0.5 are numbers.
    let offers = parse_header(Some("a; b=0; c=0.5")).unwrap();
    let offer = &offers.to_vec()[0];
    assert_eq!(offer.params.get("b"), Some(&Slot::One(num(0.0))));
    assert_eq!(offer.params.get("c"), Some(&Slot::One(num(0.5))));

    // Negative integers and fractions.
    let offers = parse_header(Some("a; b=-3; c=1.5")).unwrap();
    let offer = &offers.to_vec()[0];
    assert_eq!(offer.params.get("b"), Some(&Slot::One(num(-3.0))));
    assert_eq!(offer.params.get("c"), Some(&Slot::One(num(1.5))));

    // Exponents and a trailing dot are not numbers.
    let offers = parse_header(Some("a; b=1e5")).unwrap();
    let offer = &offers.to_vec()[0];
    assert_eq!(offer.params.get("b"), Some(&Slot::One(s("1e5"))));
}

#[test]
fn quoted_numeric_value_is_coerced() {
    // A quoted numeric string is still coerced to a number.
    let offers = parse_header(Some("a; b=\"1\"")).unwrap();
    let offer = &offers.to_vec()[0];
    assert_eq!(offer.params.get("b"), Some(&Slot::One(num(1.0))));
}

#[test]
fn quoted_value_strips_all_backslashes() {
    // Every backslash is removed, regardless of what follows.
    let offers = parse_header(Some("a; b=\"a\\b\\c\"")).unwrap();
    let offer = &offers.to_vec()[0];
    assert_eq!(offer.params.get("b"), Some(&Slot::One(s("abc"))));
}

#[test]
fn rejects_space_inside_token_area() {
    assert!(parse_header(Some("x-webkit- -frame")).is_err());
}

#[test]
fn number_serializes_without_decimal_point() {
    assert_eq!(
        serialize_params("a", &params(&[("b", num(1.0)), ("c", num(-3.0))])),
        "a; b=1; c=-3"
    );
}
