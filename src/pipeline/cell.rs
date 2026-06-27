//! One stage of the pipeline, wrapping a single session.
//!
//! A cell holds two functors, one per direction, plus the session they share.
//! It prefixes the extension name onto any error passing through, and it owns
//! the deferred close logic: a session closes once a close was requested and
//! both functors have drained.
//!
//! The cell keeps its mutable parts behind `Rc<RefCell<_>>` so a session that
//! completes synchronously can re-enter the cell from inside a callback without
//! tripping a borrow that the caller still holds.

use std::cell::RefCell;
use std::rc::Rc;

use super::functor::Functor;
use super::pledge::Pledge;
use super::{Callback, Direction, Outcome, Session, SessionRecord};

/// The parts of a cell that callbacks mutate.
struct Inner<M> {
    session: Rc<RefCell<Box<dyn Session<M>>>>,
    incoming: Functor<M>,
    outgoing: Functor<M>,
    closed: Option<Rc<RefCell<Pledge>>>,
    session_alive: bool,
}

/// One pipeline stage around a session.
pub struct Cell<M> {
    name: String,
    inner: Rc<RefCell<Inner<M>>>,
}

impl<M: 'static> Cell<M> {
    /// Build a cell from a registered session record.
    pub fn new(record: SessionRecord<M>) -> Self {
        let session: Rc<RefCell<Box<dyn Session<M>>>> = Rc::new(RefCell::new(record.session));
        let inner = Inner {
            incoming: Functor::new(session.clone(), true),
            outgoing: Functor::new(session.clone(), false),
            session,
            closed: None,
            session_alive: true,
        };
        Cell {
            name: record.name,
            inner: Rc::new(RefCell::new(inner)),
        }
    }

    /// Bump the pending counter in `direction` before a message enters.
    pub fn pending(&self, direction: Direction) {
        let inner = self.inner.borrow();
        match direction {
            Direction::Incoming => inner.incoming.bump_pending(),
            Direction::Outgoing => inner.outgoing.bump_pending(),
        }
    }

    /// Run one outcome through the cell in `direction`.
    ///
    /// On completion the cell prefixes the extension name onto any error, calls
    /// `callback`, then attempts a deferred close.
    pub fn exec(
        cell: &Rc<RefCell<Cell<M>>>,
        direction: Direction,
        outcome: Outcome<M>,
        callback: Callback<M>,
    ) {
        let name = cell.borrow().name.clone();
        let inner = cell.borrow().inner.clone();

        let inner_for_close = inner.clone();
        let wrapped: Callback<M> = Box::new(move |result: Outcome<M>| {
            let result = match result {
                Err(mut error) => {
                    error.message = format!("{}: {}", name, error.message);
                    Err(error)
                }
                ok => ok,
            };
            callback(result);
            try_close(&inner_for_close);
        });

        // Clone the functor handle, then drop the inner borrow before calling
        // so a synchronous completion can re-borrow inner.
        let functor = {
            let inner = inner.borrow();
            match direction {
                Direction::Incoming => inner.incoming.clone(),
                Direction::Outgoing => inner.outgoing.clone(),
            }
        };
        functor.call(outcome, wrapped);
    }

    /// Request a close.
    ///
    /// Idempotent. Returns the same close pledge on repeated calls. Attempts to
    /// close right away if both functors have already drained.
    pub fn close(&mut self) -> Rc<RefCell<Pledge>> {
        {
            let mut inner = self.inner.borrow_mut();
            if inner.closed.is_none() {
                inner.closed = Some(Pledge::new());
            }
        }
        let pledge = self.inner.borrow().closed.clone().unwrap();
        try_close(&self.inner);
        pledge
    }
}

/// Close the session if a close was requested and both functors drained.
fn try_close<M: 'static>(inner: &Rc<RefCell<Inner<M>>>) {
    let mut inner = inner.borrow_mut();
    let pending = inner.incoming.pending() + inner.outgoing.pending();
    if inner.closed.is_none() || pending != 0 {
        return;
    }
    if inner.session_alive {
        inner.session.borrow_mut().close();
        inner.session_alive = false;
    }
    inner.closed.as_ref().unwrap().borrow_mut().done();
}
