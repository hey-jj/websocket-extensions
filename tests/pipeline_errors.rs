//! The error-handling scenario, run with a synchronous error and an async
//! error. Both must produce the same observable message and close sequence.

mod common;

use std::cell::RefCell;
use std::rc::Rc;

use common::{err, flag, Clock, Message, MockHandle, Tag};
use websocket_extensions::{Extensions, Outcome};

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

/// How the failing incoming processor reports its error.
#[derive(Clone, Copy)]
enum ErrorMode {
    /// Report the error synchronously, like a thrown exception caught at once.
    Sync,
    /// Report the error after a short delay.
    Async,
}

/// Run the shared scenario and return the recorded messages.
///
/// Each recorded entry is `Some(message)` for a delivered message, `None` for
/// the released error, or the marker string `"close"` once the pipeline drains.
fn run_scenario(mode: ErrorMode) -> Vec<Recorded> {
    let clock = Clock::new();

    let mut ext = Extensions::new();
    let deflate = deflate_handle();
    let reverse = reverse_handle();
    ext.add(deflate.extension()).unwrap();
    ext.add(reverse.extension()).unwrap();
    ext.generate_offer();
    ext.activate("deflate, reverse").unwrap();

    // Outgoing: both append a tag after 100ms.
    {
        let clock = clock.clone();
        deflate.behavior().outgoing = Some(Rc::new(move |message: Message, cb| {
            let message = message.with("a");
            clock.set_timeout(100, Box::new(move || cb(Ok(message))));
        }));
    }
    {
        let clock = clock.clone();
        reverse.behavior().outgoing = Some(Rc::new(move |message: Message, cb| {
            let message = message.with("b");
            clock.set_timeout(100, Box::new(move || cb(Ok(message))));
        }));
    }

    // Incoming: reverse appends "c" after 50ms, but errors when the id is 5.
    {
        let clock = clock.clone();
        reverse.behavior().incoming = Some(Rc::new(move |message: Message, cb| {
            if message.first() == Some(&Tag::Num(5)) {
                match mode {
                    ErrorMode::Sync => cb(Err(err("sync error"))),
                    ErrorMode::Async => {
                        clock.set_timeout(10, Box::new(move || cb(Err(err("async error")))));
                    }
                }
                return;
            }
            let message = message.with("c");
            clock.set_timeout(50, Box::new(move || cb(Ok(message))));
        }));
    }
    // Incoming: deflate appends "d" after 100ms.
    {
        let clock = clock.clone();
        deflate.behavior().incoming = Some(Rc::new(move |message: Message, cb| {
            let message = message.with("d");
            clock.set_timeout(100, Box::new(move || cb(Ok(message))));
        }));
    }

    let messages: Rc<RefCell<Vec<Recorded>>> = Rc::new(RefCell::new(Vec::new()));

    // The driver callback. On error it starts the close and records a close
    // marker once the pipeline drains. It records every outcome's message.
    let ext = Rc::new(RefCell::new(ext));
    let push = {
        let messages = messages.clone();
        let ext = ext.clone();
        move |outcome: Outcome<Message>| {
            if outcome.is_err() {
                let messages = messages.clone();
                ext.borrow()
                    .close(move || messages.borrow_mut().push(Recorded::Close));
            }
            messages.borrow_mut().push(Recorded::from_outcome(&outcome));
        }
    };
    let push = Rc::new(push);

    // Outgoing [1], [2], [3] at t=0.
    for n in [1, 2, 3] {
        let push = push.clone();
        ext.borrow()
            .process_outgoing_message(Message::id(n), move |outcome| push(outcome));
    }

    // Incoming [4], [5], [6] staggered by 20ms.
    for (i, n) in [4, 5, 6].into_iter().enumerate() {
        let push = push.clone();
        let ext = ext.clone();
        clock.set_timeout(
            20 * i as u64,
            Box::new(move || {
                ext.borrow()
                    .process_incoming_message(Message::id(n), move |outcome| push(outcome));
            }),
        );
    }

    clock.tick(200);

    let recorded = messages.borrow();
    recorded.iter().map(Recorded::clone_value).collect()
}

/// A recorded pipeline outcome.
#[derive(Debug, PartialEq)]
enum Recorded {
    Message(Vec<Tag>),
    Error,
    Close,
}

impl Recorded {
    fn from_outcome(outcome: &Outcome<Message>) -> Self {
        match outcome {
            Ok(message) => Recorded::Message(message.frames.clone()),
            Err(_) => Recorded::Error,
        }
    }

    fn clone_value(&self) -> Self {
        match self {
            Recorded::Message(frames) => Recorded::Message(frames.clone()),
            Recorded::Error => Recorded::Error,
            Recorded::Close => Recorded::Close,
        }
    }
}

fn id_tags(n: i64, rest: &[&str]) -> Vec<Tag> {
    let mut frames = vec![Tag::Num(n)];
    for tag in rest {
        frames.push(Tag::Str((*tag).to_string()));
    }
    frames
}

fn check(messages: &[Recorded]) {
    // The message before the error passes through to the end.
    assert_eq!(messages[0], Recorded::Message(id_tags(4, &["c", "d"])));
    // The error reaches the end as a released error.
    assert_eq!(messages[1], Recorded::Error);
    // The message after the error is dropped.
    assert!(!messages.contains(&Recorded::Message(id_tags(6, &["c", "d"]))));
    // The unaffected direction yields all messages.
    assert_eq!(messages[2], Recorded::Message(id_tags(1, &["a", "b"])));
    assert_eq!(messages[3], Recorded::Message(id_tags(2, &["a", "b"])));
    assert_eq!(messages[4], Recorded::Message(id_tags(3, &["a", "b"])));
    // Close fires after every message is processed.
    assert_eq!(messages[5], Recorded::Close);
    assert_eq!(messages.len(), 6);
}

#[test]
fn handles_a_sync_error() {
    let messages = run_scenario(ErrorMode::Sync);
    check(&messages);
}

#[test]
fn handles_an_async_error() {
    let messages = run_scenario(ErrorMode::Async);
    check(&messages);
}
