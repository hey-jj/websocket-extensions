# websocket-extensions

A framework for negotiating and running WebSocket extensions, per RFC 6455
section 9. It implements no specific extension. It gives a WebSocket driver the
pieces to support extensions written as plugins.

The crate provides three things:

1. A parser and serializer for the `Sec-WebSocket-Extensions` header.
2. An `Extensions` container that registers plugins, builds client offers,
   activates server responses, builds server responses, detects RSV-bit
   conflicts, and validates frame RSV bits.
3. An ordered async pipeline that runs each message through every active
   session, preserves input order under out-of-order completion, and closes
   sessions after in-flight messages drain.

## Install

```toml
[dependencies]
websocket-extensions = "0.1"
```

## Roles

A driver holds one `Extensions` per socket.

A client advertises extensions with `generate_offer`, sends the offer, then
calls `activate` with the server's response header.

A server calls `generate_response` with the client's offer header and sends the
result back.

Both sides then transform messages with `process_incoming_message` and
`process_outgoing_message`, and shut down with `close`.

## Plugins

A plugin implements `Extension`. Its sessions implement `ClientSession` or
`ServerSession`, which extend `Session`. The message type is generic, so a
driver chooses its own frame representation.

## Header parsing

```rust
use websocket_extensions::parser::{parse_header, serialize_params};

let offers = parse_header(Some("permessage-deflate; client_max_window_bits")).unwrap();
assert_eq!(offers[0].name, "permessage-deflate");
```

`parse_header` takes `Option<&str>`. `None` and an empty string both yield an
empty offer list. A malformed header returns an error. The parser is linear, so
an unterminated quoted string fails fast.

## License

Licensed under the [MIT license](LICENSE).
