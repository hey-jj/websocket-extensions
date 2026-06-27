//! Test harness: mock extensions, mock sessions, and a virtual clock.
//!
//! Each mock session shares a `Behavior` cell with the test, which configures
//! canned offers, responses, and per-direction processors, and reads back call
//! counts and recorded arguments. A `Clock` provides deterministic virtual time
//! so the async ordering and close-drain scenarios replay without real delays.

#![allow(dead_code)]

use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::rc::Rc;

use websocket_extensions::parser::{Params, Slot, Value};
use websocket_extensions::{
    Callback, ClientSession, Extension, Outcome, PipelineError, ServerSession, Session,
};

/// One item appended to a message during processing.
#[derive(Debug, Clone, PartialEq)]
pub enum Tag {
    /// A numeric item, such as the leading id in `[4]`.
    Num(i64),
    /// A string tag pushed by a session, such as `"deflate"` or `"a"`.
    Str(String),
}

/// A test message: an ordered list of tags.
#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    /// The accumulated tags.
    pub frames: Vec<Tag>,
}

impl Message {
    /// A message with the given frames.
    pub fn new(frames: Vec<Tag>) -> Self {
        Message { frames }
    }

    /// An empty message, matching `{ frames: [] }`.
    pub fn empty() -> Self {
        Message { frames: Vec::new() }
    }

    /// A message starting with one numeric id, matching `[n]`.
    pub fn id(n: i64) -> Self {
        Message {
            frames: vec![Tag::Num(n)],
        }
    }

    /// Append a string tag and return self.
    pub fn with(mut self, tag: &str) -> Self {
        self.frames.push(Tag::Str(tag.to_string()));
        self
    }

    /// Push a string tag in place.
    pub fn push(&mut self, tag: &str) {
        self.frames.push(Tag::Str(tag.to_string()));
    }

    /// Number of frames.
    pub fn len(&self) -> usize {
        self.frames.len()
    }

    /// First tag, if any.
    pub fn first(&self) -> Option<&Tag> {
        self.frames.first()
    }
}

/// A deterministic virtual clock.
///
/// Callbacks scheduled with `set_timeout` fire in due-time order, breaking ties
/// by insertion order, when `tick` advances past their due time. This matches
/// the FIFO behavior of equal-delay timeouts.
#[derive(Clone)]
pub struct Clock {
    inner: Rc<RefCell<ClockInner>>,
}

struct ClockInner {
    now: u64,
    seq: u64,
    queue: BinaryHeap<Timer>,
}

struct Timer {
    due: u64,
    seq: u64,
    action: Box<dyn FnOnce()>,
}

impl PartialEq for Timer {
    fn eq(&self, other: &Self) -> bool {
        self.due == other.due && self.seq == other.seq
    }
}
impl Eq for Timer {}
impl PartialOrd for Timer {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Timer {
    fn cmp(&self, other: &Self) -> Ordering {
        // Reverse so the BinaryHeap (a max-heap) yields the earliest due first,
        // then the lowest sequence number.
        other.due.cmp(&self.due).then(other.seq.cmp(&self.seq))
    }
}

impl Default for Clock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock {
    /// A fresh clock at time zero.
    pub fn new() -> Self {
        Clock {
            inner: Rc::new(RefCell::new(ClockInner {
                now: 0,
                seq: 0,
                queue: BinaryHeap::new(),
            })),
        }
    }

    /// Schedule `action` to run `delay` units from now.
    pub fn set_timeout(&self, delay: u64, action: Box<dyn FnOnce()>) {
        let mut inner = self.inner.borrow_mut();
        let due = inner.now + delay;
        let seq = inner.seq;
        inner.seq += 1;
        inner.queue.push(Timer { due, seq, action });
    }

    /// Advance time by `ms`, firing every timer that comes due, in order.
    ///
    /// A firing timer may schedule more timers. Those run too if they come due
    /// within this tick.
    pub fn tick(&self, ms: u64) {
        let target = {
            let inner = self.inner.borrow();
            inner.now + ms
        };
        loop {
            let timer = {
                let mut inner = self.inner.borrow_mut();
                match inner.queue.peek() {
                    Some(t) if t.due <= target => inner.queue.pop(),
                    _ => None,
                }
            };
            match timer {
                Some(t) => {
                    self.inner.borrow_mut().now = t.due;
                    (t.action)();
                }
                None => break,
            }
        }
        self.inner.borrow_mut().now = target;
    }
}

/// How a mock session responds in one direction.
pub type Processor = Rc<dyn Fn(Message, Callback<Message>)>;

/// Shared, configurable behavior for one mock extension and its sessions.
pub struct Behavior {
    /// Whether `create_client_session` returns a session.
    pub client_session: bool,
    /// Whether `create_server_session` returns a session.
    pub server_session: bool,
    /// The canned client offer, as zero or more parameter sets, or `None`.
    pub offer: Option<Vec<Params>>,
    /// The canned server response parameters.
    pub response: Params,
    /// Whether `activate` returns true.
    pub activate_returns: bool,

    /// Optional incoming processor. Defaults to appending the name.
    pub incoming: Option<Processor>,
    /// Optional outgoing processor. Defaults to appending the name.
    pub outgoing: Option<Processor>,

    // Recorded interactions.
    pub create_client_calls: u32,
    pub create_server_calls: u32,
    pub generate_offer_calls: u32,
    pub generate_response_calls: u32,
    pub activate_calls: u32,
    pub activate_args: Vec<Params>,
    pub server_offers: Vec<Vec<Params>>,
    pub close_calls: u32,
    pub incoming_calls: u32,
    pub outgoing_calls: u32,
}

impl Behavior {
    fn new() -> Self {
        Behavior {
            client_session: true,
            server_session: true,
            offer: None,
            response: Params::new(),
            activate_returns: true,
            incoming: None,
            outgoing: None,
            create_client_calls: 0,
            create_server_calls: 0,
            generate_offer_calls: 0,
            generate_response_calls: 0,
            activate_calls: 0,
            activate_args: Vec::new(),
            server_offers: Vec::new(),
            close_calls: 0,
            incoming_calls: 0,
            outgoing_calls: 0,
        }
    }
}

/// A configurable handle to a mock extension's shared behavior.
#[derive(Clone)]
pub struct MockHandle {
    name: String,
    rsv1: bool,
    rsv2: bool,
    rsv3: bool,
    behavior: Rc<RefCell<Behavior>>,
}

impl MockHandle {
    /// Build a mock with the given name and RSV bits.
    pub fn new(name: &str, rsv1: bool, rsv2: bool, rsv3: bool) -> Self {
        MockHandle {
            name: name.to_string(),
            rsv1,
            rsv2,
            rsv3,
            behavior: Rc::new(RefCell::new(Behavior::new())),
        }
    }

    /// Borrow the shared behavior for configuration or inspection.
    pub fn behavior(&self) -> std::cell::RefMut<'_, Behavior> {
        self.behavior.borrow_mut()
    }

    /// Set the client offer to a single parameter set.
    pub fn set_offer(&self, params: Params) {
        self.behavior.borrow_mut().offer = Some(vec![params]);
    }

    /// Set the client offer to several parameter sets.
    pub fn set_offers(&self, offers: Vec<Params>) {
        self.behavior.borrow_mut().offer = Some(offers);
    }

    /// Set the server response parameters.
    pub fn set_response(&self, params: Params) {
        self.behavior.borrow_mut().response = params;
    }

    /// Build the boxed extension plugin for this mock.
    pub fn extension(&self) -> Box<dyn Extension<Message>> {
        Box::new(MockExt {
            name: self.name.clone(),
            rsv1: self.rsv1,
            rsv2: self.rsv2,
            rsv3: self.rsv3,
            behavior: self.behavior.clone(),
        })
    }
}

struct MockExt {
    name: String,
    rsv1: bool,
    rsv2: bool,
    rsv3: bool,
    behavior: Rc<RefCell<Behavior>>,
}

impl Extension<Message> for MockExt {
    fn name(&self) -> &str {
        &self.name
    }
    fn rsv1(&self) -> bool {
        self.rsv1
    }
    fn rsv2(&self) -> bool {
        self.rsv2
    }
    fn rsv3(&self) -> bool {
        self.rsv3
    }

    fn create_client_session(&self) -> Option<Box<dyn ClientSession<Message>>> {
        let mut b = self.behavior.borrow_mut();
        b.create_client_calls += 1;
        if !b.client_session {
            return None;
        }
        drop(b);
        Some(Box::new(MockSession {
            name: self.name.clone(),
            behavior: self.behavior.clone(),
        }))
    }

    fn create_server_session(&self, offers: &[&Params]) -> Option<Box<dyn ServerSession<Message>>> {
        let mut b = self.behavior.borrow_mut();
        b.create_server_calls += 1;
        b.server_offers
            .push(offers.iter().map(|p| (*p).clone()).collect());
        if !b.server_session {
            return None;
        }
        drop(b);
        Some(Box::new(MockSession {
            name: self.name.clone(),
            behavior: self.behavior.clone(),
        }))
    }
}

struct MockSession {
    name: String,
    behavior: Rc<RefCell<Behavior>>,
}

impl Session<Message> for MockSession {
    fn process_incoming_message(&mut self, message: Message, callback: Callback<Message>) {
        let processor = {
            let mut b = self.behavior.borrow_mut();
            b.incoming_calls += 1;
            b.incoming.clone()
        };
        match processor {
            Some(p) => p(message, callback),
            None => {
                let mut message = message;
                message.push(&self.name);
                callback(Ok(message));
            }
        }
    }

    fn process_outgoing_message(&mut self, message: Message, callback: Callback<Message>) {
        let processor = {
            let mut b = self.behavior.borrow_mut();
            b.outgoing_calls += 1;
            b.outgoing.clone()
        };
        match processor {
            Some(p) => p(message, callback),
            None => {
                let mut message = message;
                message.push(&self.name);
                callback(Ok(message));
            }
        }
    }

    fn close(&mut self) {
        self.behavior.borrow_mut().close_calls += 1;
    }
}

impl ClientSession<Message> for MockSession {
    fn generate_offer(&mut self) -> Option<Vec<Params>> {
        let mut b = self.behavior.borrow_mut();
        b.generate_offer_calls += 1;
        b.offer.clone()
    }

    fn activate(&mut self, params: &Params) -> bool {
        let mut b = self.behavior.borrow_mut();
        b.activate_calls += 1;
        b.activate_args.push(params.clone());
        b.activate_returns
    }
}

impl ServerSession<Message> for MockSession {
    fn generate_response(&mut self) -> Params {
        let mut b = self.behavior.borrow_mut();
        b.generate_response_calls += 1;
        b.response.clone()
    }
}

/// Helper: build a one-key params set.
pub fn one(key: &str, value: Value) -> Params {
    let mut p = Params::new();
    p.insert(key, value);
    p
}

/// Helper: build a flag params set, e.g. `{ gzip: true }`.
pub fn flag(key: &str) -> Params {
    one(key, Value::Bool(true))
}

/// Helper: read a string-or-list slot as a single value for assertions.
pub fn slot_one(slot: Option<&Slot>) -> Option<&Value> {
    match slot {
        Some(Slot::One(v)) => Some(v),
        _ => None,
    }
}

/// Make a pipeline error with the given message.
pub fn err(message: &str) -> PipelineError {
    PipelineError::new(message)
}

/// Convenience: collect tags into a vector for assertions.
pub fn tags(items: &[Tag]) -> Vec<Tag> {
    items.to_vec()
}

/// Build a frame list of string tags.
pub fn strs(items: &[&str]) -> Vec<Tag> {
    items.iter().map(|s| Tag::Str(s.to_string())).collect()
}

/// An outcome's message, or None on error.
pub fn message_of(outcome: &Outcome<Message>) -> Option<Message> {
    outcome.as_ref().ok().cloned()
}
