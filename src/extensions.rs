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
#[derive(Debug, Clone, Copy)]
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
                write!(f, "An extension with name \"{}\" is already registered", name)
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

/// A registered extension and the index it occupies.
struct Registered<M> {
    ext: Box<dyn Extension<M>>,
}

/// The extension container.
///
/// Holds registered plugins in registration order, tracks RSV reservations, and
/// owns the active pipeline once negotiation completes. One instance serves one
/// side of one socket.
pub struct Extensions<M> {
    rsv1: Option<String>,
    rsv2: Option<String>,
    rsv3: Option<String>,
    in_order: Vec<Registered<M>>,
    names: Vec<String>,
    /// Active sessions as (extension index, name, rsv bits) after negotiation.
    sessions: Vec<ActiveExt>,
    pipeline: Option<Pipeline<M>>,
    /// Client offer index: name to client session, set by generate_offer.
    client_index: Vec<ClientRecord<M>>,
}

/// Bookkeeping for an active session, used by `valid_frame_rsv`.
struct ActiveExt {
    rsv1: bool,
    rsv2: bool,
    rsv3: bool,
}

/// A client session built during `generate_offer`, kept for `activate`.
struct ClientRecord<M> {
    name: String,
    rsv1: bool,
    rsv2: bool,
    rsv3: bool,
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
            rsv1: None,
            rsv2: None,
            rsv3: None,
            in_order: Vec::new(),
            names: Vec::new(),
            sessions: Vec::new(),
            pipeline: None,
            client_index: Vec::new(),
        }
    }

    /// Register an extension plugin.
    ///
    /// Registration order sets pipeline order. A duplicate name returns
    /// [`ExtensionError::DuplicateName`].
    pub fn add(&mut self, ext: Box<dyn Extension<M>>) -> Result<(), ExtensionError> {
        let name = ext.name().to_string();
        if self.names.iter().any(|n| n == &name) {
            return Err(ExtensionError::DuplicateName(name));
        }
        self.names.push(name);
        self.in_order.push(Registered { ext });
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

        for registered in &self.in_order {
            let ext = &registered.ext;
            let mut session = match ext.create_client_session() {
                Some(session) => session,
                None => continue,
            };

            let offers = session.generate_offer().unwrap_or_default();
            for params in &offers {
                offer.push(parser::serialize_params(ext.name(), params));
            }

            index.push(ClientRecord {
                name: ext.name().to_string(),
                rsv1: ext.rsv1(),
                rsv2: ext.rsv2(),
                rsv3: ext.rsv3(),
                session: Some(session),
            });
        }

        self.client_index = index;

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
    pub fn activate(&mut self, header: &str) -> Result<(), ExtensionError> {
        let responses = parser::parse_header(Some(header))?;

        let mut records: Vec<SessionRecord<M>> = Vec::new();
        let mut active: Vec<ActiveExt> = Vec::new();
        let mut conflict: Option<ExtensionError> = None;
        let mut order: Vec<(String, bool, bool, bool)> = Vec::new();

        responses.each_offer(|name, params| {
            if conflict.is_some() {
                return;
            }
            let record = self.client_index.iter_mut().find(|r| r.name == name);
            let record = match record {
                Some(record) => record,
                None => {
                    conflict = Some(ExtensionError::UnknownExtension(name.to_string()));
                    return;
                }
            };

            if let Some((bit, first)) =
                reserved(&self.rsv1, &self.rsv2, &self.rsv3, record.rsv1, record.rsv2, record.rsv3)
            {
                conflict = Some(ExtensionError::RsvConflict(bit, first, record.name.clone()));
                return;
            }

            let mut session = record.session.take().expect("session offered once");
            if !session.activate(params) {
                conflict = Some(ExtensionError::UnacceptableParams(
                    parser::serialize_params(name, params),
                ));
                // Put the session back so a retry sees it; matches single-use
                // semantics loosely but keeps the error path clean.
                record.session = Some(session);
                return;
            }

            reserve(
                &mut self.rsv1,
                &mut self.rsv2,
                &mut self.rsv3,
                &record.name,
                record.rsv1,
                record.rsv2,
                record.rsv3,
            );
            order.push((record.name.clone(), record.rsv1, record.rsv2, record.rsv3));
            active.push(ActiveExt {
                rsv1: record.rsv1,
                rsv2: record.rsv2,
                rsv3: record.rsv3,
            });
            records.push(SessionRecord {
                name: record.name.clone(),
                session: session as Box<dyn Session<M>>,
            });
        });

        if let Some(err) = conflict {
            return Err(err);
        }

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
    pub fn generate_response(&mut self, header: &str) -> Result<Option<String>, ExtensionError> {
        let offers = parser::parse_header(Some(header))?;

        let mut response: Vec<String> = Vec::new();
        let mut records: Vec<SessionRecord<M>> = Vec::new();
        let mut active: Vec<ActiveExt> = Vec::new();

        for registered in &self.in_order {
            let ext = &registered.ext;
            let offer = offers.by_name(ext.name());
            if offer.is_empty() {
                continue;
            }
            if reserved(
                &self.rsv1, &self.rsv2, &self.rsv3, ext.rsv1(), ext.rsv2(), ext.rsv3(),
            )
            .is_some()
            {
                continue;
            }

            let mut session = match ext.create_server_session(&offer) {
                Some(session) => session,
                None => continue,
            };

            reserve(
                &mut self.rsv1,
                &mut self.rsv2,
                &mut self.rsv3,
                ext.name(),
                ext.rsv1(),
                ext.rsv2(),
                ext.rsv3(),
            );

            let params = session.generate_response();
            response.push(parser::serialize_params(ext.name(), &params));
            active.push(ActiveExt {
                rsv1: ext.rsv1(),
                rsv2: ext.rsv2(),
                rsv3: ext.rsv3(),
            });
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
        let mut allowed = (false, false, false);
        if MESSAGE_OPCODES.contains(&frame.opcode) {
            for ext in &self.sessions {
                allowed.0 |= ext.rsv1;
                allowed.1 |= ext.rsv2;
                allowed.2 |= ext.rsv3;
            }
        }
        (allowed.0 || !frame.rsv1) && (allowed.1 || !frame.rsv2) && (allowed.2 || !frame.rsv3)
    }

    /// Run a message toward the application through the pipeline.
    ///
    /// Panics if called before negotiation builds a pipeline, matching the
    /// source, which assumes a pipeline exists.
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
    /// Panics if called before negotiation builds a pipeline.
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

/// Record the first extension to claim each RSV bit it uses.
#[allow(clippy::too_many_arguments)]
fn reserve(
    rsv1: &mut Option<String>,
    rsv2: &mut Option<String>,
    rsv3: &mut Option<String>,
    name: &str,
    uses1: bool,
    uses2: bool,
    uses3: bool,
) {
    if rsv1.is_none() && uses1 {
        *rsv1 = Some(name.to_string());
    }
    if rsv2.is_none() && uses2 {
        *rsv2 = Some(name.to_string());
    }
    if rsv3.is_none() && uses3 {
        *rsv3 = Some(name.to_string());
    }
}

/// Return the first taken RSV bit this extension also wants.
///
/// Yields `(bit, reserving extension name)`, checked in order 1, 2, 3.
#[allow(clippy::too_many_arguments)]
fn reserved(
    rsv1: &Option<String>,
    rsv2: &Option<String>,
    rsv3: &Option<String>,
    uses1: bool,
    uses2: bool,
    uses3: bool,
) -> Option<(u8, String)> {
    if uses1 {
        if let Some(name) = rsv1 {
            return Some((1, name.clone()));
        }
    }
    if uses2 {
        if let Some(name) = rsv2 {
            return Some((2, name.clone()));
        }
    }
    if uses3 {
        if let Some(name) = rsv3 {
            return Some((3, name.clone()));
        }
    }
    None
}
