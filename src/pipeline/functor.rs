//! An order-preserving wrapper around one async session method.
//!
//! A functor binds one direction of one session. Each call pushes a record onto
//! a FIFO queue, then invokes the session. The session may complete out of
//! order, so the functor releases records from the front of the queue only when
//! the front is done. This keeps output order equal to input order.
//!
//! The first error in a direction is released in its correct position. Every
//! record queued behind it is dropped.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use super::{Callback, Outcome, Session};

/// One queued unit of work.
struct Record<M> {
    /// A stable id so a deferred completion can find this record after later
    /// records have queued behind it.
    id: u64,
    /// The outcome once the session completes. `None` while pending.
    outcome: Option<Outcome<M>>,
    /// The callback to fire when this record releases in order.
    callback: Option<Callback<M>>,
    /// True once the session has produced an outcome for this record.
    done: bool,
}

/// The mutable state behind a functor.
struct State<M> {
    queue: VecDeque<Record<M>>,
    stopped: bool,
    /// Count of records still owed to the next stage.
    pending: usize,
    /// Next record id.
    next_id: u64,
}

impl<M> State<M> {
    /// Freeze the pending count and stop accepting new work.
    ///
    /// After this, the count reflects exactly how many records remain to emit,
    /// so closing can complete once they drain.
    fn stop(&mut self) {
        self.pending = self.queue.len();
        self.stopped = true;
    }
}

/// An order-preserving async wrapper bound to one session method and direction.
///
/// Cloning yields another handle to the same internal state, so a cell can hold
/// a functor and still pass a clone into a callback.
pub struct Functor<M> {
    session: Rc<RefCell<Box<dyn Session<M>>>>,
    incoming: bool,
    state: Rc<RefCell<State<M>>>,
}

impl<M> Clone for Functor<M> {
    fn clone(&self) -> Self {
        Functor {
            session: self.session.clone(),
            incoming: self.incoming,
            state: self.state.clone(),
        }
    }
}

impl<M: 'static> Functor<M> {
    /// Build a functor for one direction of `session`.
    ///
    /// `incoming` selects `process_incoming_message`, otherwise
    /// `process_outgoing_message`.
    pub fn new(session: Rc<RefCell<Box<dyn Session<M>>>>, incoming: bool) -> Self {
        Functor {
            session,
            incoming,
            state: Rc::new(RefCell::new(State {
                queue: VecDeque::with_capacity(8),
                stopped: false,
                pending: 0,
                next_id: 0,
            })),
        }
    }

    /// Current pending count.
    pub fn pending(&self) -> usize {
        self.state.borrow().pending
    }

    /// Bump the pending count for a message about to enter the pipe.
    ///
    /// A stopped functor never grows its count, so the count can still drain to
    /// zero after an error.
    pub fn bump_pending(&self) {
        let mut state = self.state.borrow_mut();
        if !state.stopped {
            state.pending += 1;
        }
    }

    /// Run one outcome through the session.
    ///
    /// An incoming error short-circuits: the record is marked done, the functor
    /// stops, and the queue flushes. A normal message invokes the session,
    /// whose completion drives the flush.
    pub fn call(&self, outcome: Outcome<M>, callback: Callback<M>) {
        if self.state.borrow().stopped {
            return;
        }

        let message = match outcome {
            Err(error) => {
                {
                    let mut state = self.state.borrow_mut();
                    let id = state.next_id;
                    state.next_id += 1;
                    state.queue.push_back(Record {
                        id,
                        outcome: Some(Err(error)),
                        callback: Some(callback),
                        done: true,
                    });
                    state.stop();
                }
                flush(&self.state);
                return;
            }
            Ok(message) => message,
        };

        let id = {
            let mut state = self.state.borrow_mut();
            let id = state.next_id;
            state.next_id += 1;
            state.queue.push_back(Record {
                id,
                outcome: None,
                callback: Some(callback),
                done: false,
            });
            id
        };

        let state = self.state.clone();
        let called = Rc::new(RefCell::new(false));
        let handler: Callback<M> = Box::new(move |result: Outcome<M>| {
            // Once-guard: only the first completion has effect.
            {
                let mut flag = called.borrow_mut();
                if *flag {
                    return;
                }
                *flag = true;
            }

            let is_err = result.is_err();
            {
                let mut state = state.borrow_mut();
                if is_err {
                    state.stop();
                }
                if let Some(record) = state.queue.iter_mut().find(|r| r.id == id) {
                    record.outcome = Some(result);
                    record.done = true;
                }
            }
            flush(&state);
        });

        // Hand the message to the session. The session calls the handler once,
        // now or later. The handler drives ordered release.
        let session = self.session.clone();
        if self.incoming {
            session
                .borrow_mut()
                .process_incoming_message(message, handler);
        } else {
            session
                .borrow_mut()
                .process_outgoing_message(message, handler);
        }
    }
}

/// Release done records from the front of the queue, in order.
///
/// A record carrying an error sets pending to zero, drops everything queued
/// behind it, and fires its callback. After that, nothing further is emitted.
fn flush<M>(state: &Rc<RefCell<State<M>>>) {
    loop {
        let (callback, outcome) = {
            let mut s = state.borrow_mut();
            let front_done = s.queue.front().map(|r| r.done).unwrap_or(false);
            if !front_done {
                break;
            }
            let mut record = s.queue.pop_front().unwrap();
            let outcome = record.outcome.take().unwrap();
            if outcome.is_err() {
                s.pending = 0;
                s.queue.clear();
            } else {
                s.pending -= 1;
            }
            (record.callback.take().unwrap(), outcome)
        };
        callback(outcome);
    }
}
