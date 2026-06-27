//! Server negotiation: generate_response.

mod common;

use common::{flag, one, Message, MockHandle};
use websocket_extensions::parser::Value;
use websocket_extensions::Extensions;

fn deflate_handle() -> MockHandle {
    let h = MockHandle::new("deflate", true, false, false);
    h.set_response(one("mode", Value::Str("compress".into())));
    h
}

fn conflict_handle() -> MockHandle {
    let h = MockHandle::new("tar", true, false, false);
    h.set_response(flag("gzip"));
    h
}

fn nonconflict_handle() -> MockHandle {
    let h = MockHandle::new("reverse", false, true, false);
    h.set_response(flag("utf8"));
    h
}

/// Build extensions with deflate, tar, reverse in registration order.
fn setup() -> (Extensions<Message>, MockHandle, MockHandle, MockHandle) {
    let mut ext = Extensions::new();
    let deflate = deflate_handle();
    let conflict = conflict_handle();
    let nonconflict = nonconflict_handle();
    ext.add(deflate.extension()).unwrap();
    ext.add(conflict.extension()).unwrap();
    ext.add(nonconflict.extension()).unwrap();
    (ext, deflate, conflict, nonconflict)
}

#[test]
fn asks_for_a_server_session_with_the_offer() {
    let (mut ext, deflate, ..) = setup();
    ext.generate_response("deflate; flag").unwrap();
    let b = deflate.behavior();
    assert_eq!(b.create_server_calls, 1);
    assert_eq!(b.server_offers[0], vec![flag("flag")]);
}

#[test]
fn asks_for_a_server_session_with_multiple_offers() {
    let (mut ext, deflate, ..) = setup();
    ext.generate_response("deflate; a, deflate; b").unwrap();
    let b = deflate.behavior();
    assert_eq!(b.create_server_calls, 1);
    assert_eq!(b.server_offers[0], vec![flag("a"), flag("b")]);
}

#[test]
fn asks_the_session_to_generate_a_response() {
    let (mut ext, deflate, ..) = setup();
    ext.generate_response("deflate").unwrap();
    assert_eq!(deflate.behavior().generate_response_calls, 1);
}

#[test]
fn asks_multiple_sessions_to_generate_a_response() {
    let (mut ext, deflate, _conflict, nonconflict) = setup();
    ext.generate_response("deflate, reverse").unwrap();
    assert_eq!(deflate.behavior().generate_response_calls, 1);
    assert_eq!(nonconflict.behavior().generate_response_calls, 1);
}

#[test]
fn no_response_if_the_extension_builds_no_session() {
    let (mut ext, deflate, ..) = setup();
    deflate.behavior().server_session = false;
    ext.generate_response("deflate").unwrap();
    assert_eq!(deflate.behavior().generate_response_calls, 0);
}

#[test]
fn does_not_build_a_session_for_unoffered_extensions() {
    let (mut ext, _deflate, _conflict, nonconflict) = setup();
    ext.generate_response("deflate").unwrap();
    assert_eq!(nonconflict.behavior().create_server_calls, 0);
}

#[test]
fn does_not_build_a_session_for_conflicting_extensions() {
    let (mut ext, _deflate, conflict, _nonconflict) = setup();
    ext.generate_response("deflate, tar").unwrap();
    assert_eq!(conflict.behavior().create_server_calls, 0);
}

#[test]
fn returns_the_serialized_response_from_the_session() {
    let (mut ext, ..) = setup();
    assert_eq!(
        ext.generate_response("deflate").unwrap().as_deref(),
        Some("deflate; mode=compress")
    );
}

#[test]
fn returns_serialized_responses_from_multiple_sessions() {
    let (mut ext, ..) = setup();
    assert_eq!(
        ext.generate_response("deflate, reverse").unwrap().as_deref(),
        Some("deflate; mode=compress, reverse; utf8")
    );
}

#[test]
fn returns_responses_in_registration_order() {
    let (mut ext, ..) = setup();
    assert_eq!(
        ext.generate_response("reverse, deflate").unwrap().as_deref(),
        Some("deflate; mode=compress, reverse; utf8")
    );
}

#[test]
fn does_not_return_responses_for_unoffered_extensions() {
    let (mut ext, ..) = setup();
    assert_eq!(
        ext.generate_response("reverse").unwrap().as_deref(),
        Some("reverse; utf8")
    );
}

#[test]
fn does_not_return_responses_for_conflicting_extensions() {
    let (mut ext, ..) = setup();
    assert_eq!(
        ext.generate_response("deflate, tar").unwrap().as_deref(),
        Some("deflate; mode=compress")
    );
}

#[test]
fn throws_if_the_header_is_invalid() {
    let (mut ext, ..) = setup();
    assert!(ext.generate_response("x-webkit- -frame").is_err());
}

#[test]
fn returns_a_response_for_conflicting_extension_if_predecessor_builds_no_session() {
    let (mut ext, deflate, ..) = setup();
    deflate.behavior().server_session = false;
    assert_eq!(
        ext.generate_response("deflate, tar").unwrap().as_deref(),
        Some("tar; gzip")
    );
}
