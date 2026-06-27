//! RSV bit validation for frame headers against the active session set.

mod common;

use common::{flag, Message, MockHandle};
use websocket_extensions::{Extensions, Frame, MESSAGE_OPCODES};

fn deflate_handle() -> MockHandle {
    let h = MockHandle::new("deflate", true, false, false);
    h.set_offer(flag("x"));
    h.behavior().activate_returns = true;
    h
}

fn reverse_handle() -> MockHandle {
    let h = MockHandle::new("reverse", false, true, false);
    h.set_offer(flag("y"));
    h.behavior().activate_returns = true;
    h
}

/// Activate deflate (rsv1) and reverse (rsv2).
fn setup() -> Extensions<Message> {
    let mut ext = Extensions::new();
    ext.add(deflate_handle().extension()).unwrap();
    ext.add(reverse_handle().extension()).unwrap();
    ext.generate_offer();
    ext.activate("deflate, reverse").unwrap();
    ext
}

fn frame(opcode: u8, rsv1: bool, rsv2: bool, rsv3: bool) -> Frame {
    Frame {
        opcode,
        rsv1,
        rsv2,
        rsv3,
    }
}

#[test]
fn message_opcodes_constant() {
    assert_eq!(MESSAGE_OPCODES, [1, 2]);
}

#[test]
fn frame_with_no_rsv_bits_is_always_valid() {
    let ext = setup();
    assert!(ext.valid_frame_rsv(&frame(1, false, false, false)));
    assert!(ext.valid_frame_rsv(&frame(8, false, false, false)));
}

#[test]
fn message_frame_allows_a_reserved_rsv_bit() {
    let ext = setup();
    // rsv1 is reserved by deflate, rsv2 by reverse.
    assert!(ext.valid_frame_rsv(&frame(1, true, false, false)));
    assert!(ext.valid_frame_rsv(&frame(2, false, true, false)));
    assert!(ext.valid_frame_rsv(&frame(1, true, true, false)));
}

#[test]
fn message_frame_rejects_an_unreserved_rsv_bit() {
    let ext = setup();
    // No active session reserves rsv3.
    assert!(!ext.valid_frame_rsv(&frame(1, false, false, true)));
    assert!(!ext.valid_frame_rsv(&frame(2, true, false, true)));
}

#[test]
fn control_frame_rejects_any_rsv_bit() {
    let ext = setup();
    // Opcode 8 (close) is not a message opcode, so no bit is allowed even
    // though sessions reserve rsv1 and rsv2.
    assert!(!ext.valid_frame_rsv(&frame(8, true, false, false)));
    assert!(!ext.valid_frame_rsv(&frame(8, false, true, false)));
    assert!(!ext.valid_frame_rsv(&frame(9, false, false, true)));
}

#[test]
fn no_sessions_means_no_rsv_bits_allowed() {
    let mut ext = Extensions::<Message>::new();
    ext.add(deflate_handle().extension()).unwrap();
    ext.generate_offer();
    ext.activate("").unwrap();
    // Empty header activates nothing, so even a message frame disallows rsv1.
    assert!(!ext.valid_frame_rsv(&frame(1, true, false, false)));
    assert!(ext.valid_frame_rsv(&frame(1, false, false, false)));
}

#[test]
fn fresh_container_disallows_every_rsv_bit() {
    // No negotiation has run, so the active session set is empty. A message
    // frame with any RSV bit set is invalid, while a bare frame is valid.
    let ext = Extensions::<Message>::new();
    assert!(!ext.valid_frame_rsv(&frame(1, true, false, false)));
    assert!(!ext.valid_frame_rsv(&frame(2, false, true, false)));
    assert!(!ext.valid_frame_rsv(&frame(1, false, false, true)));
    assert!(ext.valid_frame_rsv(&frame(1, false, false, false)));
    assert!(ext.valid_frame_rsv(&frame(8, false, false, false)));
}

#[test]
fn server_side_reservations_gate_rsv_bits() {
    // Tie generate_response to valid_frame_rsv: an activated server session
    // reserves the bit it uses.
    let mut ext = Extensions::<Message>::new();
    let deflate = MockHandle::new("deflate", true, false, false);
    deflate.set_response(flag("ok"));
    ext.add(deflate.extension()).unwrap();
    ext.generate_response("deflate").unwrap();

    assert!(ext.valid_frame_rsv(&frame(2, true, false, false)));
    assert!(!ext.valid_frame_rsv(&frame(2, false, true, false)));
}

#[test]
fn generate_offer_reflects_offered_extensions_in_the_active_set() {
    // After generate_offer, valid_frame_rsv must already allow the RSV bits the
    // offered extensions use, before activate runs.
    let mut ext = Extensions::<Message>::new();
    ext.add(deflate_handle().extension()).unwrap();
    ext.generate_offer();

    // deflate uses rsv1, so a message frame with rsv1 is valid.
    assert!(ext.valid_frame_rsv(&frame(1, true, false, false)));
    // No offered extension uses rsv2 or rsv3.
    assert!(!ext.valid_frame_rsv(&frame(1, false, true, false)));
    // A control frame still disallows every bit.
    assert!(!ext.valid_frame_rsv(&frame(8, true, false, false)));
}

#[test]
fn a_second_generate_offer_overwrites_the_active_set() {
    // The first offer sets rsv1 via deflate. A second offer with only an rsv2
    // extension must replace the active set, not accumulate it.
    let mut ext = Extensions::<Message>::new();
    ext.add(deflate_handle().extension()).unwrap();
    ext.generate_offer();
    assert!(ext.valid_frame_rsv(&frame(1, true, false, false)));

    let mut ext = Extensions::<Message>::new();
    ext.add(reverse_handle().extension()).unwrap();
    ext.generate_offer();
    ext.generate_offer();
    // reverse uses rsv2, and the repeat offer must not leave rsv1 allowed.
    assert!(ext.valid_frame_rsv(&frame(1, false, true, false)));
    assert!(!ext.valid_frame_rsv(&frame(1, true, false, false)));
}
