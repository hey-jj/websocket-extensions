//! Client negotiation: generate_offer and activate.

mod common;

use common::{flag, one, Message, MockHandle};
use websocket_extensions::parser::{Params, Value};
use websocket_extensions::Extensions;

/// Set up the deflate mock with offer { mode: compress } and activate -> true.
fn deflate_handle() -> MockHandle {
    let h = MockHandle::new("deflate", true, false, false);
    h.set_offer(one("mode", Value::Str("compress".into())));
    h.behavior().activate_returns = true;
    h
}

fn conflict_handle() -> MockHandle {
    // tar conflicts with deflate on RSV1.
    let h = MockHandle::new("tar", true, false, false);
    h.set_offer(flag("gzip"));
    h.behavior().activate_returns = true;
    h
}

fn nonconflict_handle() -> MockHandle {
    // reverse uses RSV2, no conflict.
    let h = MockHandle::new("reverse", false, true, false);
    h.set_offer(flag("utf8"));
    h.behavior().activate_returns = true;
    h
}

// generateOffer

#[test]
fn asks_the_extension_to_create_a_client_session() {
    let mut ext = Extensions::new();
    let deflate = deflate_handle();
    ext.add(deflate.extension()).unwrap();
    ext.generate_offer();
    assert_eq!(deflate.behavior().create_client_calls, 1);
}

#[test]
fn asks_the_session_to_generate_an_offer() {
    let mut ext = Extensions::new();
    let deflate = deflate_handle();
    ext.add(deflate.extension()).unwrap();
    ext.generate_offer();
    assert_eq!(deflate.behavior().generate_offer_calls, 1);
}

#[test]
fn does_not_ask_for_an_offer_if_no_session_is_built() {
    let mut ext = Extensions::new();
    let deflate = deflate_handle();
    deflate.behavior().client_session = false;
    ext.add(deflate.extension()).unwrap();
    ext.generate_offer();
    assert_eq!(deflate.behavior().generate_offer_calls, 0);
}

#[test]
fn returns_the_serialized_offer_from_the_session() {
    let mut ext = Extensions::new();
    let deflate = deflate_handle();
    ext.add(deflate.extension()).unwrap();
    assert_eq!(ext.generate_offer().as_deref(), Some("deflate; mode=compress"));
}

#[test]
fn returns_a_null_offer_from_the_session() {
    let mut ext = Extensions::new();
    let deflate = deflate_handle();
    deflate.behavior().offer = None;
    ext.add(deflate.extension()).unwrap();
    assert_eq!(ext.generate_offer(), None);
}

#[test]
fn returns_multiple_serialized_offers_from_the_session() {
    let mut ext = Extensions::new();
    let deflate = deflate_handle();
    // offer [ {mode: compress}, {} ]
    deflate.set_offers(vec![one("mode", Value::Str("compress".into())), Params::new()]);
    ext.add(deflate.extension()).unwrap();
    assert_eq!(
        ext.generate_offer().as_deref(),
        Some("deflate; mode=compress, deflate")
    );
}

#[test]
fn returns_serialized_offers_from_multiple_sessions() {
    let mut ext = Extensions::new();
    ext.add(deflate_handle().extension()).unwrap();
    ext.add(nonconflict_handle().extension()).unwrap();
    assert_eq!(
        ext.generate_offer().as_deref(),
        Some("deflate; mode=compress, reverse; utf8")
    );
}

#[test]
fn generates_offers_for_potentially_conflicting_extensions() {
    let mut ext = Extensions::new();
    ext.add(deflate_handle().extension()).unwrap();
    ext.add(conflict_handle().extension()).unwrap();
    assert_eq!(
        ext.generate_offer().as_deref(),
        Some("deflate; mode=compress, tar; gzip")
    );
}

// activate

/// Build extensions with deflate, tar, reverse added and an offer generated,
/// returning the handles for inspection.
fn setup_activate() -> (Extensions<Message>, MockHandle, MockHandle, MockHandle) {
    let mut ext = Extensions::new();
    let deflate = deflate_handle();
    let conflict = conflict_handle();
    let nonconflict = nonconflict_handle();
    ext.add(deflate.extension()).unwrap();
    ext.add(conflict.extension()).unwrap();
    ext.add(nonconflict.extension()).unwrap();
    ext.generate_offer();
    (ext, deflate, conflict, nonconflict)
}

#[test]
fn throws_if_given_unregistered_extensions() {
    let (mut ext, ..) = setup_activate();
    assert!(ext.activate("xml").is_err());
}

#[test]
fn does_not_throw_if_given_registered_extensions() {
    let (mut ext, ..) = setup_activate();
    assert!(ext.activate("deflate").is_ok());
}

#[test]
fn does_not_throw_for_one_potentially_conflicting_extension() {
    let (mut ext, ..) = setup_activate();
    assert!(ext.activate("tar").is_ok());
}

#[test]
fn throws_if_two_extensions_conflict_on_rsv_bits() {
    let (mut ext, ..) = setup_activate();
    assert!(ext.activate("deflate, tar").is_err());
}

#[test]
fn does_not_throw_for_two_non_conflicting_extensions() {
    let (mut ext, ..) = setup_activate();
    assert!(ext.activate("deflate, reverse").is_ok());
}

#[test]
fn activates_one_session_with_no_params() {
    let (mut ext, deflate, ..) = setup_activate();
    ext.activate("deflate").unwrap();
    let b = deflate.behavior();
    assert_eq!(b.activate_calls, 1);
    assert!(b.activate_args[0].is_empty());
}

#[test]
fn activates_one_session_with_a_boolean_param() {
    let (mut ext, deflate, ..) = setup_activate();
    ext.activate("deflate; gzip").unwrap();
    let b = deflate.behavior();
    assert_eq!(b.activate_calls, 1);
    assert_eq!(b.activate_args[0], flag("gzip"));
}

#[test]
fn activates_one_session_with_a_string_param() {
    let (mut ext, deflate, ..) = setup_activate();
    ext.activate("deflate; mode=compress").unwrap();
    let b = deflate.behavior();
    assert_eq!(b.activate_calls, 1);
    assert_eq!(b.activate_args[0], one("mode", Value::Str("compress".into())));
}

#[test]
fn activates_multiple_sessions() {
    let (mut ext, deflate, _conflict, nonconflict) = setup_activate();
    ext.activate("deflate; a, reverse; b").unwrap();
    assert_eq!(deflate.behavior().activate_args[0], flag("a"));
    assert_eq!(nonconflict.behavior().activate_args[0], flag("b"));
}

#[test]
fn does_not_activate_sessions_not_named_in_the_header() {
    let (mut ext, deflate, _conflict, nonconflict) = setup_activate();
    ext.activate("reverse").unwrap();
    assert_eq!(deflate.behavior().activate_calls, 0);
    assert_eq!(nonconflict.behavior().activate_calls, 1);
}

#[test]
fn throws_if_activate_does_not_return_true() {
    let (mut ext, deflate, ..) = setup_activate();
    deflate.behavior().activate_returns = false;
    assert!(ext.activate("deflate").is_err());
}
