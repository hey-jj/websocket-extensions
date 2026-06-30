//! The extension container: registration, negotiation, and message processing.
//!
//! [`Extensions`] registers extension plugins, builds client offers, activates
//! server responses on the client, builds server responses, enforces RSV-bit
//! conflicts, validates frame RSV bits, and runs messages through the pipeline.
//!
//! This is a framework. It implements no specific extension such as
//! `permessage-deflate`. A caller supplies plugins through the [`Extension`]
//! trait, and their sessions through [`ClientSession`] and [`ServerSession`].

use crate::parser::{self, Params, ParseError};
use crate::pipeline::{Outcome, Pipeline, Session, SessionRecord};

/// Opcodes treated as message frames: text (1) and binary (2).
///
/// [`Extensions::valid_frame_rsv`] only consults active sessions for these
/// opcodes. Any other opcode disallows every RSV bit.
pub const MESSAGE_OPCODES: [u8; 2] = [1, 2];

/// A frame header, as far as RSV validation needs it.
///
/// Only `opcode` and the three RSV bits are read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Frame {
    /// The frame opcode. 1 and 2 are message frames.
    pub opcode: u8,
    /// The RSV1 bit.
    pub rsv1: bool,
    /// The RSV2 bit.
    pub rsv2: bool,
    /// The RSV3 bit.
    pub rsv3: bool,
}

/// A client-side session for one extension.
///
/// Extends [`Session`] with the client negotiation methods. The client offers
/// parameters, then activates the server's chosen parameters.
pub trait ClientSession<M>: Session<M> {
    /// Produce the parameter sets to offer, or `None` to offer nothing.
    ///
    /// One session may offer several parameter sets, each serialized under the
    /// same extension name.
    fn generate_offer(&mut self) -> Option<Vec<Params>>;

    /// Accept or reject the server's chosen parameters.
    ///
    /// Returns `true` to accept. Any other value rejects, which aborts
    /// activation with an error.
    fn activate(&mut self, params: &Params) -> bool;
}

/// A server-side session for one extension.
///
/// Extends [`Session`] with the server negotiation method.
pub trait ServerSession<M>: Session<M> {
    /// Produce the parameters to send back in the response.
    fn generate_response(&mut self) -> Params;
}

/// An extension plugin.
///
/// A plugin names itself, declares which RSV bits it uses, and builds sessions
/// on demand. The type is always `permessage`, so no `type` field is needed.
pub trait Extension<M> {
    /// The extension name. Must be unique within one [`Extensions`].
    fn name(&self) -> &str;
    /// Whether the extension uses RSV1.
    fn rsv1(&self) -> bool;
    /// Whether the extension uses RSV2.
    fn rsv2(&self) -> bool;
    /// Whether the extension uses RSV3.
    fn rsv3(&self) -> bool;

    /// Build a client session, or `None` to offer nothing for this extension.
    fn create_client_session(&self) -> Option<Box<dyn ClientSession<M>>>;

    /// Build a server session from all offers parsed for this extension.
    ///
    /// Returns `None` to decline. `offers` holds every parameter set the client
    /// sent under this extension name, in order.
    fn create_server_session(&self, offers: &[&Params]) -> Option<Box<dyn ServerSession<M>>>;
}

/// An error raised while registering or activating extensions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtensionError {
    /// Two extensions share a name. Carries the duplicate name.
    DuplicateName(String),
    /// A server response named an extension that was never offered.
    UnknownExtension(String),
    /// Two server responses claimed the same RSV bit.
    ///
    /// Carries the bit number, the first extension, and the second.
    RsvConflict(u8, String, String),
    /// A session rejected the server's parameters. Carries the serialized pair.
    UnacceptableParams(String),
    /// The header failed to parse.
    Parse(ParseError),
}

impl std::fmt::Display for ExtensionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExtensionError::DuplicateName(name) => {
                write!(
                    f,
                    "An extension with name \"{}\" is already registered",
                    name
                )
            }
            ExtensionError::UnknownExtension(name) => write!(
                f,
                "Server sent an extension response for unknown extension \"{}\"",
                name
            ),
            ExtensionError::RsvConflict(bit, first, second) => write!(
                f,
                "Server sent two extension responses that use the RSV{} bit: \"{}\" and \"{}\"",
                bit, first, second
            ),
            ExtensionError::UnacceptableParams(pair) => {
                write!(f, "Server sent unacceptable extension parameters: {}", pair)
            }
            ExtensionError::Parse(err) => err.fmt(f),
        }
    }
}

impl std::error::Error for ExtensionError {}

impl From<ParseError> for ExtensionError {
    fn from(err: ParseError) -> Self {
        ExtensionError::Parse(err)
    }
}

/// The extension container.
///
/// Holds registered plugins in registration order, tracks RSV reservations, and
/// owns the active pipeline once negotiation completes. One instance serves one
/// side of one socket.
pub struct Extensions<M> {
    rsv: RsvReservations,
    in_order: Vec<Box<dyn Extension<M>>>,
    names: Vec<String>,
    /// Active sessions with their RSV bits, read by `valid_frame_rsv`.
    sessions: Vec<ActiveExt>,
    pipeline: Option<Pipeline<M>>,
    /// Client sessions built by `generate_offer`, consumed by `activate`.
    client_index: Vec<ClientRecord<M>>,
}

/// Which RSV bits an extension uses, as `[rsv1, rsv2, rsv3]`.
#[derive(Clone, Copy)]
struct RsvUse([bool; 3]);

/// The holder of each RSV bit, by bit index.
///
/// Slot `i` names the first extension to claim bit `i + 1`, or is empty.
#[derive(Default)]
struct RsvReservations {
    bits: [Option<String>; 3],
}

impl RsvReservations {
    /// First taken bit this use also wants, as `(bit number, holder name)`.
    ///
    /// Bits are checked in order 1, 2, 3.
    fn conflict(&self, uses: RsvUse) -> Option<(u8, String)> {
        for i in 0..3 {
            if uses.0[i] {
                if let Some(name) = &self.bits[i] {
                    return Some((i as u8 + 1, name.clone()));
                }
            }
        }
        None
    }

    /// Claim each bit this use wants that is still free, naming `name` as holder.
    fn reserve(&mut self, name: &str, uses: RsvUse) {
        for i in 0..3 {
            if uses.0[i] && self.bits[i].is_none() {
                self.bits[i] = Some(name.to_string());
            }
        }
    }
}

/// Bookkeeping for an active session, used by `valid_frame_rsv`.
struct ActiveExt {
    uses: RsvUse,
}

/// A client session built during `generate_offer`, kept for `activate`.
struct ClientRecord<M> {
    name: String,
    uses: RsvUse,
    session: Option<Box<dyn ClientSession<M>>>,
}

impl<M: 'static> Default for Extensions<M> {
    fn default() -> Self {
        Self::new()
    }
}

impl<M: 'static> Extensions<M> {
    /// Create an empty container.
    pub fn new() -> Self {
        Extensions {
            rsv: RsvReservations::default(),
            in_order: Vec::new(),
            names: Vec::new(),
            sessions: Vec::new(),
            pipeline: None,
            client_index: Vec::new(),
        }
    }

    /// Register an extension plugin.
    ///
    /// Registration order sets pipeline order.
    ///
    /// # Errors
    ///
    /// Returns [`ExtensionError::DuplicateName`] when an extension with the same
    /// name is already registered.
    pub fn add(&mut self, ext: Box<dyn Extension<M>>) -> Result<(), ExtensionError> {
        let name = ext.name().to_string();
        if self.names.iter().any(|n| n == &name) {
            return Err(ExtensionError::DuplicateName(name));
        }
        self.names.push(name);
        self.in_order.push(ext);
        Ok(())
    }

    /// Build the client offer header.
    ///
    /// Walks extensions in registration order. For each, builds a client session
    /// and serializes its offers. Stores the sessions for [`activate`]. Returns
    /// the joined header, or `None` when no extension offers anything.
    ///
    /// Conflicting extensions are not filtered here. The client offers
    /// everything. Conflicts resolve at [`activate`].
    ///
    /// [`activate`]: Extensions::activate
    pub fn generate_offer(&mut self) -> Option<String> {
        let mut offer: Vec<String> = Vec::new();
        let mut index: Vec<ClientRecord<M>> = Vec::new();
        let mut active: Vec<ActiveExt> = Vec::new();

        for ext in &self.in_order {
            let mut session = match ext.create_client_session() {
                Some(session) => session,
                None => continue,
            };

            let uses = RsvUse([ext.rsv1(), ext.rsv2(), ext.rsv3()]);
            let offers = session.generate_offer().unwrap_or_default();
            for params in &offers {
                offer.push(parser::serialize_params(ext.name(), params));
            }

            active.push(ActiveExt { uses });
            index.push(ClientRecord {
                name: ext.name().to_string(),
                uses,
                session: Some(session),
            });
        }

        self.client_index = index;
        // Reflect the offered extensions in the active session set so
        // valid_frame_rsv allows their RSV bits in the window before activate.
        // A later generate_offer overwrites this, as activate does.
        self.sessions = active;

        if offer.is_empty() {
            None
        } else {
            Some(offer.join(", "))
        }
    }

    /// Activate the server's response on the client.
    ///
    /// Parses the response header, then for each response in header order looks
    /// up the offered session, checks for an RSV conflict, and activates the
    /// session with the server's parameters. Builds the pipeline from the
    /// activated sessions in server-header order.
    ///
    /// # Errors
    ///
    /// Returns [`ExtensionError::Parse`] when the response header is malformed,
    /// [`ExtensionError::UnknownExtension`] when the server names an extension
    /// that was not offered, [`ExtensionError::RsvConflict`] when two responses
    /// claim the same RSV bit, and [`ExtensionError::UnacceptableParams`] when a
    /// session rejects the server's parameters. On any error the container is
    /// left unchanged, so a corrected response can be activated next.
    pub fn activate(&mut self, header: &str) -> Result<(), ExtensionError> {
        let responses = parser::parse_header(Some(header))?;

        // Validate every response into locals before touching self, so an error
        // partway through leaves self unchanged and a retry starts clean.
        let mut rsv = RsvReservations::default();
        let mut active: Vec<ActiveExt> = Vec::new();
        let mut accepted: Vec<usize> = Vec::new();

        for offer in &responses {
            let name = offer.name.as_str();
            let params = &offer.params;

            let idx = self
                .client_index
                .iter()
                .position(|r| r.name == name)
                .ok_or_else(|| ExtensionError::UnknownExtension(name.to_string()))?;
            let uses = self.client_index[idx].uses;

            if let Some((bit, first)) = rsv.conflict(uses) {
                return Err(ExtensionError::RsvConflict(
                    bit,
                    first,
                    self.client_index[idx].name.clone(),
                ));
            }

            let session = self.client_index[idx]
                .session
                .as_mut()
                .expect("session offered once");
            if !session.activate(params) {
                return Err(ExtensionError::UnacceptableParams(
                    parser::serialize_params(name, params),
                ));
            }

            rsv.reserve(&self.client_index[idx].name, uses);
            active.push(ActiveExt { uses });
            accepted.push(idx);
        }

        // Every response validated. Take the accepted sessions and commit.
        let records: Vec<SessionRecord<M>> = accepted
            .into_iter()
            .map(|idx| {
                let record = &mut self.client_index[idx];
                SessionRecord {
                    name: record.name.clone(),
                    session: record.session.take().expect("session offered once")
                        as Box<dyn Session<M>>,
                }
            })
            .collect();

        self.rsv = rsv;
        self.sessions = active;
        self.pipeline = Some(Pipeline::new(records));
        Ok(())
    }

    /// Build the server response header.
    ///
    /// Parses the client offer, then walks extensions in registration order. For
    /// each offered, non-conflicting extension, builds a server session and
    /// serializes its response. Builds the pipeline. Returns the joined header,
    /// or `None` when no extension responds.
    ///
    /// The response is in registration order, not client-offer order.
    ///
    /// # Errors
    ///
    /// Returns [`ExtensionError::Parse`] when the client offer header is
    /// malformed.
    pub fn generate_response(&mut self, header: &str) -> Result<Option<String>, ExtensionError> {
        let offers = parser::parse_header(Some(header))?;

        let mut response: Vec<String> = Vec::new();
        let mut records: Vec<SessionRecord<M>> = Vec::new();
        let mut active: Vec<ActiveExt> = Vec::new();

        for ext in &self.in_order {
            let offer = offers.by_name(ext.name());
            if offer.is_empty() {
                continue;
            }
            let uses = RsvUse([ext.rsv1(), ext.rsv2(), ext.rsv3()]);
            if self.rsv.conflict(uses).is_some() {
                continue;
            }

            let mut session = match ext.create_server_session(&offer) {
                Some(session) => session,
                None => continue,
            };

            self.rsv.reserve(ext.name(), uses);

            let params = session.generate_response();
            response.push(parser::serialize_params(ext.name(), &params));
            active.push(ActiveExt { uses });
            records.push(SessionRecord {
                name: ext.name().to_string(),
                session: session as Box<dyn Session<M>>,
            });
        }

        self.sessions = active;
        self.pipeline = Some(Pipeline::new(records));

        Ok(if response.is_empty() {
            None
        } else {
            Some(response.join(", "))
        })
    }

    /// Check whether a frame's RSV bits are allowed.
    ///
    /// For a message frame (opcode 1 or 2) a bit is allowed when some active
    /// session reserves it. For any other opcode no bit is allowed. A frame is
    /// valid when every bit it sets is allowed.
    pub fn valid_frame_rsv(&self, frame: &Frame) -> bool {
        let mut allowed = [false, false, false];
        if MESSAGE_OPCODES.contains(&frame.opcode) {
            for ext in &self.sessions {
                allowed[0] |= ext.uses.0[0];
                allowed[1] |= ext.uses.0[1];
                allowed[2] |= ext.uses.0[2];
            }
        }
        (allowed[0] || !frame.rsv1) && (allowed[1] || !frame.rsv2) && (allowed[2] || !frame.rsv3)
    }

    /// Run a message toward the application through the pipeline.
    ///
    /// # Panics
    ///
    /// Panics if called before `activate` or `generate_response` builds the
    /// pipeline.
    pub fn process_incoming_message<F>(&self, message: M, callback: F)
    where
        F: FnOnce(Outcome<M>) + 'static,
    {
        self.pipeline
            .as_ref()
            .expect("pipeline built by activate or generate_response")
            .process_incoming_message(message, callback);
    }

    /// Run a message toward the peer through the pipeline.
    ///
    /// # Panics
    ///
    /// Panics if called before `activate` or `generate_response` builds the
    /// pipeline.
    pub fn process_outgoing_message<F>(&self, message: M, callback: F)
    where
        F: FnOnce(Outcome<M>) + 'static,
    {
        self.pipeline
            .as_ref()
            .expect("pipeline built by activate or generate_response")
            .process_outgoing_message(message, callback);
    }

    /// Close the pipeline and notify when it drains.
    ///
    /// With no pipeline yet, `callback` fires at once. With a pipeline, closing
    /// defers until every in-flight message drains.
    pub fn close<F>(&self, callback: F)
    where
        F: FnOnce() + 'static,
    {
        match &self.pipeline {
            None => callback(),
            Some(pipeline) => pipeline.close(Some(callback)),
        }
    }
}
