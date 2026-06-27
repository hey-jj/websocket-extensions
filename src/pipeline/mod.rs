//! The ordered, async message-processing pipeline.
//!
//! A pipeline runs each message through every active session in order. Outgoing
//! messages traverse sessions front to back. Incoming messages traverse back to
//! front. The pipeline guarantees:
//!
//! - Each session sees messages in the same order in each direction.
//! - The driver receives transformed messages in input order, even when a
//!   session completes out of order.
//! - An error in one direction drops every later message in that direction and
//!   halts it. The other direction keeps running.
//! - [`Pipeline::close`] defers session teardown until every in-flight message
//!   drains, then fires its callback.
//!
//! Sessions may complete synchronously or store the callback and complete
//! later. The pipeline holds its state behind `Rc<RefCell<_>>` so a deferred
//! completion can drive the rest of the pipe.

mod cell;
mod functor;
mod pledge;

use std::cell::RefCell;
use std::rc::Rc;

use cell::Cell;
use pledge::Pledge;

/// An error flowing through the pipeline.
///
/// The message is mutated in place as it passes a cell, gaining a `name: `
/// prefix for the session that produced or relayed it. Each cell prepends its
/// extension name so a failing hop is identifiable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PipelineError {
    /// The error text. Cells prepend their extension name and `: `.
    pub message: String,
}

impl PipelineError {
    /// Build an error from any message string.
    pub fn new(message: impl Into<String>) -> Self {
        PipelineError {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for PipelineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for PipelineError {}

/// The value handed to a session completion callback.
pub type Outcome<M> = Result<M, PipelineError>;

/// A session completion callback.
///
/// A session calls this exactly once with the transformed message or an error.
/// Calling it more than once has no effect after the first call.
pub type Callback<M> = Box<dyn FnOnce(Outcome<M>)>;

/// Direction of message flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    /// From the peer to the application. Traverses sessions back to front.
    Incoming,
    /// From the application to the peer. Traverses sessions front to back.
    Outgoing,
}

/// One extension instance acting on a socket.
///
/// A session transforms messages in both directions and releases resources on
/// [`Session::close`]. The two `process_*` methods take ownership of the
/// message and call `callback` once, synchronously or later. The pipeline keeps
/// input order regardless of completion order.
pub trait Session<M> {
    /// Transform an incoming message, then call `callback`.
    fn process_incoming_message(&mut self, message: M, callback: Callback<M>);
    /// Transform an outgoing message, then call `callback`.
    fn process_outgoing_message(&mut self, message: M, callback: Callback<M>);
    /// Release any resources. Called once, after all messages drain.
    fn close(&mut self);
}

/// A registered extension paired with its session.
///
/// The name is read when prefixing error messages. The session does the work.
pub struct SessionRecord<M> {
    /// The extension name, used to prefix errors.
    pub name: String,
    /// The session that transforms messages.
    pub session: Box<dyn Session<M>>,
}

/// The ordered async pipeline over a set of sessions.
pub struct Pipeline<M> {
    cells: Rc<[Rc<RefCell<Cell<M>>>]>,
    stopped_incoming: Rc<RefCell<bool>>,
    stopped_outgoing: Rc<RefCell<bool>>,
}

impl<M: 'static> Pipeline<M> {
    /// Build a pipeline from session records in pipeline order.
    pub fn new(sessions: Vec<SessionRecord<M>>) -> Self {
        let cells: Rc<[Rc<RefCell<Cell<M>>>]> = sessions
            .into_iter()
            .map(|record| Rc::new(RefCell::new(Cell::new(record))))
            .collect();
        Pipeline {
            cells,
            stopped_incoming: Rc::new(RefCell::new(false)),
            stopped_outgoing: Rc::new(RefCell::new(false)),
        }
    }

    /// Push a message toward the application.
    ///
    /// Dropped without calling `callback` when the incoming direction is
    /// stopped.
    pub fn process_incoming_message<F>(&self, message: M, callback: F)
    where
        F: FnOnce(Outcome<M>) + 'static,
    {
        if *self.stopped_incoming.borrow() {
            return;
        }
        let n = self.cells.len() as isize;
        self.run(
            Direction::Incoming,
            n - 1,
            -1,
            -1,
            message,
            Box::new(callback),
        );
    }

    /// Push a message toward the peer.
    ///
    /// Dropped without calling `callback` when the outgoing direction is
    /// stopped.
    pub fn process_outgoing_message<F>(&self, message: M, callback: F)
    where
        F: FnOnce(Outcome<M>) + 'static,
    {
        if *self.stopped_outgoing.borrow() {
            return;
        }
        let n = self.cells.len() as isize;
        self.run(Direction::Outgoing, 0, n, 1, message, Box::new(callback));
    }

    /// Close the pipeline.
    ///
    /// Stops both directions so no new message is accepted, then closes every
    /// cell. When every cell has drained, `callback` fires. With no callback,
    /// closing still proceeds.
    pub fn close<F>(&self, callback: Option<F>)
    where
        F: FnOnce() + 'static,
    {
        *self.stopped_incoming.borrow_mut() = true;
        *self.stopped_outgoing.borrow_mut() = true;

        let closed: Vec<Rc<RefCell<Pledge>>> = self
            .cells
            .iter()
            .map(|cell| cell.borrow_mut().close())
            .collect();

        if let Some(callback) = callback {
            Pledge::all(closed).borrow_mut().then(Box::new(callback));
        }
    }

    /// Drive one message through the cells in `direction`.
    ///
    /// Every cell's pending counter is bumped up front so a concurrent close
    /// sees the outstanding work. A recursive step hands the message to each
    /// cell in turn and delivers the final result to `callback`.
    fn run(
        &self,
        direction: Direction,
        start: isize,
        end: isize,
        step: isize,
        message: M,
        callback: Callback<M>,
    ) {
        for cell in self.cells.iter() {
            cell.borrow_mut().pending(direction);
        }

        let stopped = match direction {
            Direction::Incoming => self.stopped_incoming.clone(),
            Direction::Outgoing => self.stopped_outgoing.clone(),
        };

        let ctx = Rc::new(LoopCtx {
            // One refcount bump on the shared cell slice, no Vec allocation.
            cells: self.cells.clone(),
            direction,
            end,
            step,
            stopped,
        });

        pipe(ctx, start, Ok(message), callback);
    }
}

/// The loop-invariant parts of one pipe traversal.
struct LoopCtx<M> {
    cells: Rc<[Rc<RefCell<Cell<M>>>]>,
    direction: Direction,
    end: isize,
    step: isize,
    stopped: Rc<RefCell<bool>>,
}

/// One step of the recursive pipe.
///
/// When `index` reaches `end`, the result is delivered to `callback`. Otherwise
/// the message runs through `cells[index]`, and the cell's completion advances
/// to the next index, halting the direction on error.
fn pipe<M: 'static>(ctx: Rc<LoopCtx<M>>, index: isize, outcome: Outcome<M>, callback: Callback<M>) {
    if index == ctx.end {
        callback(outcome);
        return;
    }

    let cell = ctx.cells[index as usize].clone();
    let direction = ctx.direction;
    let step = ctx.step;
    let next = Box::new(move |result: Outcome<M>| {
        if result.is_err() {
            *ctx.stopped.borrow_mut() = true;
        }
        pipe(ctx, index + step, result, callback);
    });

    Cell::exec(&cell, direction, outcome, next);
}
