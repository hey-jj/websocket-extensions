//! Synchronous message processing: order, error prefixing, and short-circuit.

mod common;

use std::cell::RefCell;
use std::rc::Rc;

use common::{err, flag, strs, Message, MockHandle, Tag};
use websocket_extensions::parser::Value;
use websocket_extensions::{Extensions, Outcome};

fn deflate_handle() -> MockHandle {
    let h = MockHandle::new("deflate", true, false, false);
    h.set_offer(one_mode());
    h.behavior().activate_returns = true;
    h
}

fn one_mode() -> websocket_extensions::parser::Params {
    let mut p = websocket_extensions::parser::Params::new();
    p.insert("mode", Value::Str("compress".into()));
    p
}

fn nonconflict_handle() -> MockHandle {
    let h = MockHandle::new("reverse", false, true, false);
    h.set_offer(flag("utf8"));
    h.behavior().activate_returns = true;
    h
}

/// Build a client-activated Extensions over deflate + reverse.
fn setup() -> (Extensions<Message>, MockHandle, MockHandle) {
    let mut ext = Extensions::new();
    let deflate = deflate_handle();
    let reverse = nonconflict_handle();
    ext.add(deflate.extension()).unwrap();
    ext.add(reverse.extension()).unwrap();
    ext.generate_offer();
    (ext, deflate, reverse)
}

// processIncomingMessage

#[test]
fn incoming_runs_in_reverse_order_of_the_response() {
    let (mut ext, ..) = setup();
    ext.activate("deflate, reverse").unwrap();

    let got: Rc<RefCell<Option<Outcome<Message>>>> = Rc::new(RefCell::new(None));
    let sink = got.clone();
    ext.process_incoming_message(Message::empty(), move |outcome| {
        *sink.borrow_mut() = Some(outcome);
    });

    let outcome = got.borrow_mut().take().unwrap();
    let message = outcome.expect("no error");
    assert_eq!(message.frames, strs(&["reverse", "deflate"]));
}

#[test]
fn incoming_yields_an_error_with_the_name_prefix() {
    let (mut ext, deflate, _reverse) = setup();
    ext.activate("deflate").unwrap();
    deflate.behavior().incoming = Some(Rc::new(|_msg, cb| cb(Err(err("ENOENT")))));

    let got: Rc<RefCell<Option<Outcome<Message>>>> = Rc::new(RefCell::new(None));
    let sink = got.clone();
    ext.process_incoming_message(Message::empty(), move |outcome| {
        *sink.borrow_mut() = Some(outcome);
    });

    let outcome = got.borrow_mut().take().unwrap();
    let error = outcome.expect_err("expected an error");
    assert_eq!(error.message, "deflate: ENOENT");
}

#[test]
fn incoming_does_not_call_sessions_after_an_error() {
    let (mut ext, deflate, reverse) = setup();
    ext.activate("deflate, reverse").unwrap();
    // Incoming hits reverse first. It errors, so deflate never runs.
    reverse.behavior().incoming = Some(Rc::new(|_msg, cb| cb(Err(err("ENOENT")))));

    ext.process_incoming_message(Message::empty(), |_| {});
    assert_eq!(deflate.behavior().incoming_calls, 0);
}

// processOutgoingMessage

#[test]
fn outgoing_runs_in_the_servers_response_order() {
    let (mut ext, ..) = setup();
    ext.activate("deflate, reverse").unwrap();

    let got: Rc<RefCell<Option<Outcome<Message>>>> = Rc::new(RefCell::new(None));
    let sink = got.clone();
    ext.process_outgoing_message(Message::empty(), move |outcome| {
        *sink.borrow_mut() = Some(outcome);
    });

    let outcome = got.borrow_mut().take().unwrap();
    let message = outcome.expect("no error");
    assert_eq!(message.frames, strs(&["deflate", "reverse"]));
}

#[test]
fn outgoing_uses_the_servers_order_not_the_clients() {
    let (mut ext, ..) = setup();
    ext.activate("reverse, deflate").unwrap();

    let got: Rc<RefCell<Option<Outcome<Message>>>> = Rc::new(RefCell::new(None));
    let sink = got.clone();
    ext.process_outgoing_message(Message::empty(), move |outcome| {
        *sink.borrow_mut() = Some(outcome);
    });

    let outcome = got.borrow_mut().take().unwrap();
    let message = outcome.expect("no error");
    assert_eq!(message.frames, strs(&["reverse", "deflate"]));
}

#[test]
fn outgoing_yields_an_error_with_the_name_prefix() {
    let (mut ext, deflate, _reverse) = setup();
    ext.activate("deflate").unwrap();
    deflate.behavior().outgoing = Some(Rc::new(|_msg, cb| cb(Err(err("ENOENT")))));

    let got: Rc<RefCell<Option<Outcome<Message>>>> = Rc::new(RefCell::new(None));
    let sink = got.clone();
    ext.process_outgoing_message(Message::empty(), move |outcome| {
        *sink.borrow_mut() = Some(outcome);
    });

    let outcome = got.borrow_mut().take().unwrap();
    let error = outcome.expect_err("expected an error");
    assert_eq!(error.message, "deflate: ENOENT");
}

#[test]
fn outgoing_does_not_call_sessions_after_an_error() {
    let (mut ext, deflate, reverse) = setup();
    ext.activate("deflate, reverse").unwrap();
    // Outgoing hits deflate first. It errors, so reverse never runs.
    deflate.behavior().outgoing = Some(Rc::new(|_msg, cb| cb(Err(err("ENOENT")))));

    ext.process_outgoing_message(Message::empty(), |_| {});
    assert_eq!(reverse.behavior().outgoing_calls, 0);
}

// Added: close with no pipeline fires the callback immediately.

#[test]
fn close_with_no_pipeline_calls_back_immediately() {
    let ext = Extensions::<Message>::new();
    let notified = Rc::new(RefCell::new(false));
    let flag = notified.clone();
    ext.close(move || *flag.borrow_mut() = true);
    assert!(*notified.borrow());
}

// Added: a trailing id frame survives processing.

#[test]
fn carries_a_leading_id_through_processing() {
    let (mut ext, ..) = setup();
    ext.activate("deflate, reverse").unwrap();

    let got: Rc<RefCell<Option<Message>>> = Rc::new(RefCell::new(None));
    let sink = got.clone();
    ext.process_outgoing_message(Message::id(7), move |outcome| {
        *sink.borrow_mut() = outcome.ok();
    });

    let message = got.borrow_mut().take().unwrap();
    assert_eq!(
        message.frames,
        vec![Tag::Num(7), Tag::Str("deflate".into()), Tag::Str("reverse".into())]
    );
}
