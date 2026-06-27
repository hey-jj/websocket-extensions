//! A one-shot synchronous barrier.
//!
//! A pledge is a minimal promise. [`Pledge::then`] runs its callback at once if
//! the pledge is already complete, otherwise it queues the callback. [`done`]
//! flips the pledge to complete and runs every queued callback in order. No
//! values pass through. Resolution is synchronous, with no event loop.
//!
//! [`Pledge::all`] resolves when every pledge in a list resolves. An empty list
//! resolves at once.
//!
//! [`done`]: Pledge::done

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

/// A synchronous, non-chaining barrier.
pub struct Pledge {
    complete: bool,
    callbacks: VecDeque<Box<dyn FnOnce()>>,
}

impl Pledge {
    /// Create an unresolved pledge.
    pub fn new() -> Rc<RefCell<Pledge>> {
        Rc::new(RefCell::new(Pledge {
            complete: false,
            callbacks: VecDeque::with_capacity(4),
        }))
    }

    /// Register `callback`.
    ///
    /// Runs at once when already complete, otherwise queues for [`done`].
    ///
    /// [`done`]: Pledge::done
    pub fn then(&mut self, callback: Box<dyn FnOnce()>) {
        if self.complete {
            callback();
        } else {
            self.callbacks.push_back(callback);
        }
    }

    /// Mark complete and run every queued callback in order.
    pub fn done(&mut self) {
        self.complete = true;
        while let Some(callback) = self.callbacks.pop_front() {
            callback();
        }
    }

    /// Resolve when every pledge in `list` resolves.
    ///
    /// An empty list resolves at once.
    pub fn all(list: Vec<Rc<RefCell<Pledge>>>) -> Rc<RefCell<Pledge>> {
        let aggregate = Pledge::new();
        let pending = Rc::new(RefCell::new(list.len()));

        if *pending.borrow() == 0 {
            aggregate.borrow_mut().done();
        }

        for pledge in list {
            let aggregate = aggregate.clone();
            let pending = pending.clone();
            pledge.borrow_mut().then(Box::new(move || {
                let mut count = pending.borrow_mut();
                *count -= 1;
                if *count == 0 {
                    aggregate.borrow_mut().done();
                }
            }));
        }

        aggregate
    }
}
