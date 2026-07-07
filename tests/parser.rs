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
    let got = offers.as_slice();
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
fn parses_tab_after_extension_comma() {
    assert_parses(Some("a,\tb"), &[("a", Params::new()), ("b", Params::new())]);
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
        &[(
            "a",
            params(&[("b", flag()), ("c", num(1.0)), ("d", s("hi"))]),
        )],
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
    let offer = &offers[0];
    assert_eq!(
        offer.params.get("b"),
        Some(&Slot::Many(vec![flag(), s("hi")]))
    );
    assert_eq!(offer.params.get("c"), Some(&Slot::One(num(1.0))));
}

#[test]
fn parses_multiple_complex_offers() {
    let offers = parse_header(Some("a; b=1, c, b; d, c; e=\"hi, there\"; e, a; b")).unwrap();
    let got = offers.as_slice();
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
    let offer = &offers[0];
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
    let offer = &offers[0];
    assert_eq!(offer.params.get("b"), Some(&Slot::One(s("01"))));
    assert_eq!(offer.params.get("c"), Some(&Slot::One(s("00"))));

    // Bare 0 and 0.5 are numbers.
    let offers = parse_header(Some("a; b=0; c=0.5")).unwrap();
    let offer = &offers[0];
    assert_eq!(offer.params.get("b"), Some(&Slot::One(num(0.0))));
    assert_eq!(offer.params.get("c"), Some(&Slot::One(num(0.5))));

    // Negative integers and fractions.
    let offers = parse_header(Some("a; b=-3; c=1.5")).unwrap();
    let offer = &offers[0];
    assert_eq!(offer.params.get("b"), Some(&Slot::One(num(-3.0))));
    assert_eq!(offer.params.get("c"), Some(&Slot::One(num(1.5))));

    // Exponents and a trailing dot are not numbers.
    let offers = parse_header(Some("a; b=1e5")).unwrap();
    let offer = &offers[0];
    assert_eq!(offer.params.get("b"), Some(&Slot::One(s("1e5"))));
}

#[test]
fn quoted_numeric_value_is_coerced() {
    // A quoted numeric string is still coerced to a number.
    let offers = parse_header(Some("a; b=\"1\"")).unwrap();
    let offer = &offers[0];
    assert_eq!(offer.params.get("b"), Some(&Slot::One(num(1.0))));
}

#[test]
fn numeric_value_past_f64_range_stays_a_string_and_round_trips() {
    // A digit run too large for f64 saturates to an infinity. Keeping it as a
    // string avoids serializing the value back as "Infinity".
    let big = "1".repeat(400);
    let header = format!("a; b={}", big);
    let offers = parse_header(Some(&header)).unwrap();
    assert_eq!(offers[0].params.get("b"), Some(&Slot::One(s(&big))));
    assert_eq!(serialize_params("a", &offers[0].params), header);
}

#[test]
fn quoted_value_keeps_escaped_bytes() {
    let offers = parse_header(Some("a; b=\"a\\b\\c\"")).unwrap();
    assert_eq!(offers[0].params.get("b"), Some(&Slot::One(s("abc"))));

    let offers = parse_header(Some("a; b=\"\\\\b\"")).unwrap();
    assert_eq!(offers[0].params.get("b"), Some(&Slot::One(s("\\b"))));

    let offers = parse_header(Some("a; b=\"x\\\\y\\\\z\"")).unwrap();
    assert_eq!(offers[0].params.get("b"), Some(&Slot::One(s("x\\y\\z"))));
}

#[test]
fn escaped_control_char_in_a_quoted_value_is_rejected() {
    assert!(parse_header(Some("a; b=\"x\\\ny\"")).is_err());
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

// Added: quoted-string grammar edge cases the token grammar admits.

#[test]
fn tab_is_allowed_unescaped_in_a_quoted_value() {
    // 0x09 sits outside the forbidden control ranges, so it passes through.
    let offers = parse_header(Some("a; b=\"x\ty\"")).unwrap();
    let offer = &offers[0];
    assert_eq!(offer.params.get("b"), Some(&Slot::One(s("x\ty"))));
}

#[test]
fn non_ascii_bytes_survive_in_a_quoted_value() {
    // The non-escaped class excludes control chars but admits any other code
    // point, so multi-byte UTF-8 passes through intact.
    let offers = parse_header(Some("a; b=\"café\"")).unwrap();
    let offer = &offers[0];
    assert_eq!(offer.params.get("b"), Some(&Slot::One(s("café"))));
}

#[test]
fn empty_value_after_equals_is_rejected() {
    // `a; b=` has no token or quoted value, so the whole header is invalid.
    assert!(parse_header(Some("a; b=")).is_err());
}

#[test]
fn raw_control_char_in_a_quoted_value_is_rejected() {
    // 0x01 is a forbidden control char inside a quoted string.
    assert!(parse_header(Some("a; b=\"\u{01}\"")).is_err());
}

#[test]
fn serializes_special_number_values() {
    // NaN and the infinities print as NaN, Infinity, and -Infinity.
    assert_eq!(
        serialize_params("a", &params(&[("b", num(f64::NAN))])),
        "a; b=NaN"
    );
    assert_eq!(
        serialize_params("a", &params(&[("b", num(f64::INFINITY))])),
        "a; b=Infinity"
    );
    assert_eq!(
        serialize_params("a", &params(&[("b", num(f64::NEG_INFINITY))])),
        "a; b=-Infinity"
    );
}

#[test]
fn serializes_a_fractional_number() {
    assert_eq!(
        serialize_params("a", &params(&[("b", num(1.5))])),
        "a; b=1.5"
    );
}

#[test]
fn serializes_a_value_with_an_inner_quote() {
    assert_eq!(
        serialize_params("a", &params(&[("b", s("a\"b"))])),
        "a; b=\"a\\\"b\""
    );
}

#[test]
fn serializes_a_value_with_an_inner_backslash() {
    assert_eq!(
        serialize_params("a", &params(&[("b", s("a\\b"))])),
        "a; b=\"a\\\\b\""
    );
}

// Offers accessors: by_name and iteration.

#[test]
fn by_name_groups_offers_and_returns_empty_for_unknown() {
    let offers = parse_header(Some("a; x=1, b, a; y=2")).unwrap();
    let a_params = offers.by_name("a");
    assert_eq!(a_params.len(), 2);
    assert_eq!(a_params[0].get("x"), Some(&Slot::One(num(1.0))));
    assert_eq!(a_params[1].get("y"), Some(&Slot::One(num(2.0))));
    assert_eq!(offers.by_name("b").len(), 1);
    assert!(offers.by_name("missing").is_empty());
}

#[test]
fn iter_visits_every_offer_in_wire_order() {
    let offers = parse_header(Some("a, b, a")).unwrap();
    let seen: Vec<&str> = offers.iter().map(|o| o.name.as_str()).collect();
    assert_eq!(seen, vec!["a", "b", "a"]);
}

#[test]
fn offers_iterate_by_reference_and_index_like_a_slice() {
    let offers = parse_header(Some("a, b, a")).unwrap();
    // IntoIterator for &Offers drives a plain for loop.
    let mut seen = Vec::new();
    for offer in &offers {
        seen.push(offer.name.clone());
    }
    assert_eq!(seen, vec!["a", "b", "a"]);
    // Deref to [Offer] gives slice indexing and len.
    assert_eq!(offers.len(), 3);
    assert_eq!(offers[1].name, "b");
}

#[test]
fn serializes_a_large_integer_without_scientific_notation() {
    // 1e19 sits past the i64 range. It still prints as a plain integer with no
    // decimal point and no exponent.
    assert_eq!(
        serialize_params("a", &params(&[("b", num(1e19))])),
        "a; b=10000000000000000000"
    );
}

#[test]
fn quotes_each_value_that_needs_quoting_independently() {
    // Two values in one call both hold a space, so both are quoted. The quoting
    // decision does not depend on earlier values in the same call.
    let mut p = Params::new();
    p.insert("b", s("a b"));
    p.insert("c", s("x y"));
    assert_eq!(serialize_params("a", &p), "a; b=\"a b\"; c=\"x y\"");
}
