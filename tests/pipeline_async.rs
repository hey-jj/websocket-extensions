//! Async pipeline behavior under a virtual clock: out-of-order completion,
//! deferred close, per-cell close timing, and already-closed notification.

mod common;

use std::cell::RefCell;
use std::rc::Rc;

use common::{flag, Clock, Message, MockHandle, Tag};
use websocket_extensions::Extensions;

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

fn setup() -> (Extensions<Message>, MockHandle, MockHandle) {
    let mut ext = Extensions::new();
    let deflate = deflate_handle();
    let reverse = reverse_handle();
    ext.add(deflate.extension()).unwrap();
    ext.add(reverse.extension()).unwrap();
    ext.generate_offer();
    (ext, deflate, reverse)
}

#[test]
fn processes_messages_in_order_despite_out_of_order_completion() {
    let clock = Clock::new();
    let (mut ext, deflate, reverse) = setup();
    ext.activate("deflate, reverse").unwrap();

    // Shared tag queue consumed in hand-off order.
    let tags = Rc::new(RefCell::new(vec![
        "a".to_string(),
        "b".to_string(),
        "c".to_string(),
        "d".to_string(),
    ]));

    {
        let clock = clock.clone();
        let tags = tags.clone();
        deflate.behavior().outgoing = Some(Rc::new(move |mut message: Message, cb| {
            let time = if message.frames.is_empty() { 100 } else { 20 };
            let tag = tags.borrow_mut().remove(0);
            message.push(&tag);
            clock.set_timeout(time, Box::new(move || cb(Ok(message))));
        }));
    }
    {
        let clock = clock.clone();
        let tags = tags.clone();
        reverse.behavior().outgoing = Some(Rc::new(move |mut message: Message, cb| {
            let time = if message.frames.len() == 1 { 100 } else { 20 };
            let tag = tags.borrow_mut().remove(0);
            message.push(&tag);
            clock.set_timeout(time, Box::new(move || cb(Ok(message))));
        }));
    }

    let out: Rc<RefCell<Vec<Message>>> = Rc::new(RefCell::new(Vec::new()));

    let sink = out.clone();
    ext.process_outgoing_message(Message::empty(), move |outcome| {
        sink.borrow_mut().push(outcome.unwrap());
    });
    let sink = out.clone();
    ext.process_outgoing_message(Message::id(1), move |outcome| {
        sink.borrow_mut().push(outcome.unwrap());
    });

    clock.tick(200);

    let result = out.borrow();
    assert_eq!(result.len(), 2);
    assert_eq!(
        result[0].frames,
        vec![Tag::Str("a".into()), Tag::Str("c".into())]
    );
    assert_eq!(
        result[1].frames,
        vec![Tag::Num(1), Tag::Str("b".into()), Tag::Str("d".into())]
    );
}

#[test]
fn defers_closing_until_the_extension_finishes() {
    let clock = Clock::new();
    let (mut ext, deflate, _reverse) = setup();
    ext.activate("deflate").unwrap();

    {
        let clock = clock.clone();
        deflate.behavior().outgoing = Some(Rc::new(move |message: Message, cb| {
            clock.set_timeout(100, Box::new(move || cb(Ok(message))));
        }));
    }

    let notified = Rc::new(RefCell::new(false));

    ext.process_outgoing_message(Message::empty(), |_| {});
    let flag = notified.clone();
    ext.close(move || *flag.borrow_mut() = true);

    clock.tick(50);
    assert_eq!(deflate.behavior().close_calls, 0);
    assert!(!*notified.borrow());

    clock.tick(50);
    assert_eq!(deflate.behavior().close_calls, 1);
    assert!(*notified.borrow());
}

#[test]
fn closes_each_session_as_soon_as_it_finishes() {
    let clock = Clock::new();
    let (mut ext, deflate, reverse) = setup();
    ext.activate("deflate, reverse").unwrap();

    // deflate takes 100ms, reverse takes 100ms more (200ms total for the
    // message), so deflate closes first.
    {
        let clock = clock.clone();
        deflate.behavior().outgoing = Some(Rc::new(move |message: Message, cb| {
            clock.set_timeout(100, Box::new(move || cb(Ok(message))));
        }));
    }
    {
        let clock = clock.clone();
        reverse.behavior().outgoing = Some(Rc::new(move |message: Message, cb| {
            clock.set_timeout(100, Box::new(move || cb(Ok(message))));
        }));
    }

    let notified = Rc::new(RefCell::new(false));

    ext.process_outgoing_message(Message::empty(), |_| {});
    let flag = notified.clone();
    ext.close(move || *flag.borrow_mut() = true);

    clock.tick(50);
    assert_eq!(deflate.behavior().close_calls, 0);
    assert_eq!(reverse.behavior().close_calls, 0);
    assert!(!*notified.borrow());

    clock.tick(100);
    assert_eq!(deflate.behavior().close_calls, 1);
    assert_eq!(reverse.behavior().close_calls, 0);
    assert!(!*notified.borrow());

    clock.tick(50);
    assert_eq!(deflate.behavior().close_calls, 1);
    assert_eq!(reverse.behavior().close_calls, 1);
    assert!(*notified.borrow());
}

#[test]
fn notifies_of_closure_immediately_if_already_closed() {
    let clock = Clock::new();
    let (mut ext, deflate, _reverse) = setup();
    ext.activate("deflate").unwrap();

    {
        let clock = clock.clone();
        deflate.behavior().outgoing = Some(Rc::new(move |message: Message, cb| {
            clock.set_timeout(100, Box::new(move || cb(Ok(message))));
        }));
    }

    ext.process_outgoing_message(Message::empty(), |_| {});
    ext.close(|| {});
    clock.tick(100);

    let notified = Rc::new(RefCell::new(false));
    let flag = notified.clone();
    ext.close(move || *flag.borrow_mut() = true);
    assert!(*notified.borrow());
}
