//! Negotiate and run WebSocket extensions.
//!
//! This crate is a generic framework for the `Sec-WebSocket-Extensions` header
//! and the extension processing pipeline of RFC 6455 section 9. It implements
//! no specific extension. It provides the pieces a WebSocket driver needs to
//! support extensions written as plugins:
//!
//! 1. A parser and serializer for the `Sec-WebSocket-Extensions` header. See
//!    [`parser`].
//! 2. An [`Extensions`] container that registers plugins, builds client offers,
//!    activates server responses, builds server responses, detects RSV-bit
//!    conflicts, and validates frame RSV bits.
//! 3. An ordered async pipeline that runs each message through every active
//!    session, preserves input order even under out-of-order completion, and
//!    closes sessions gracefully after in-flight messages drain. See
//!    [`pipeline`].
//!
//! # Roles
//!
//! A driver holds one [`Extensions`] per socket. A client calls
//! [`Extensions::generate_offer`] to advertise extensions, then
//! [`Extensions::activate`] with the server's response. A server calls
//! [`Extensions::generate_response`] with the client's offer. Both then call
//! [`Extensions::process_incoming_message`] and
//! [`Extensions::process_outgoing_message`] to transform messages, and
//! [`Extensions::close`] to shut down.
//!
//! # Plugins
//!
//! A plugin implements [`Extension`]. Its sessions implement [`ClientSession`]
//! or [`ServerSession`], which extend [`Session`]. The message type is generic,
//! so a driver chooses its own frame representation.
//!
//! # Example
//!
//! ```
//! use websocket_extensions::parser::{parse_header, serialize_params};
//!
//! let offers = parse_header(Some("permessage-deflate; client_max_window_bits")).unwrap();
//! assert_eq!(offers[0].name, "permessage-deflate");
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod parser;
pub mod pipeline;

mod extensions;

pub use extensions::{
    ClientSession, Extension, ExtensionError, Extensions, Frame, ServerSession, MESSAGE_OPCODES,
};
pub use pipeline::{Callback, Direction, Outcome, PipelineError, Session, SessionRecord};
