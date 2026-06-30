//! Parse and serialize the `Sec-WebSocket-Extensions` HTTP header.
//!
//! The header grammar comes from RFC 6455 section 9.1, built on the RFC 7230
//! `token` and `quoted-string` productions:
//!
//! ```text
//! Sec-WebSocket-Extensions = extension-list
//! extension-list  = 1#extension
//! extension       = extension-token *( ";" extension-param )
//! extension-token = token
//! extension-param = token [ "=" ( token | quoted-string ) ]
//! ```
//!
//! [`parse_header`] turns a header string into [`Offers`]. [`serialize_params`]
//! turns one extension name plus its parameters back into a header fragment.
//! The two are inverses for the common token-only case.

use std::fmt;

/// A parameter value carried by one extension offer.
///
/// Parsing only ever produces three shapes:
///
/// - [`Value::Bool`] with `true` for a valueless flag such as `b` in `a; b`.
/// - [`Value::Number`] for a value that matches the numeric grammar, including
///   a quoted numeric value such as `b="1"`.
/// - [`Value::Str`] for any other value.
///
/// Boolean `false` never appears from parsing. Serialization treats each shape
/// differently: a `true` flag prints as a bare key, a number prints unquoted,
/// and a string is quoted only when it contains a non-token character.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// A valueless flag. Always `true` when produced by parsing.
    Bool(bool),
    /// A numeric value, stored as an IEEE double.
    Number(f64),
    /// A string value.
    Str(String),
}

impl Value {
    fn is_true(&self) -> bool {
        matches!(self, Value::Bool(true))
    }
}

/// One parameter slot inside an offer.
///
/// A key seen once holds a [`Slot::One`]. A key seen again is promoted to a
/// [`Slot::Many`] that keeps every value in encounter order. This mirrors the
/// way a repeated parameter collapses into an array.
#[derive(Debug, Clone, PartialEq)]
pub enum Slot {
    /// A single value for a key seen once.
    One(Value),
    /// Two or more values for a repeated key, in encounter order.
    Many(Vec<Value>),
}

/// Parameters for one extension offer, in insertion order.
///
/// Keys keep the order they were parsed or inserted. A repeated key promotes
/// its slot to hold a list. Lookups are linear, which is fine for the handful
/// of parameters a real header carries.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Params {
    entries: Vec<(String, Slot)>,
}

impl Params {
    /// Create an empty parameter set.
    pub fn new() -> Self {
        Params {
            entries: Vec::new(),
        }
    }

    /// Insert a value under `key`.
    ///
    /// A new key stores the value directly. An existing key is promoted to a
    /// list and the value is appended, preserving order.
    pub fn insert(&mut self, key: &str, value: Value) {
        for (k, slot) in &mut self.entries {
            if k == key {
                match slot {
                    Slot::One(first) => {
                        *slot = Slot::Many(vec![first.clone(), value]);
                    }
                    Slot::Many(list) => list.push(value),
                }
                return;
            }
        }
        self.entries.push((key.to_string(), Slot::One(value)));
    }

    /// Look up the slot stored under `key`, if any.
    pub fn get(&self, key: &str) -> Option<&Slot> {
        self.entries
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, slot)| slot)
    }

    /// Number of distinct keys.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True when no parameters are present.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Iterate keys and slots in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Slot)> {
        self.entries.iter().map(|(k, slot)| (k.as_str(), slot))
    }
}

/// One parsed extension: a name plus its parameters.
#[derive(Debug, Clone, PartialEq)]
pub struct Offer {
    /// The extension name.
    pub name: String,
    /// The extension parameters, in insertion order.
    pub params: Params,
}

/// The result of parsing a header: every offer in wire order.
///
/// The same extension name may appear more than once. Order is preserved.
/// [`Offers::by_name`] groups every offer sharing a name, which the server
/// negotiation path uses to hand all offers for one extension to its plugin.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Offers {
    in_order: Vec<Offer>,
}

impl Offers {
    /// Create an empty offer list.
    pub fn new() -> Self {
        Offers {
            in_order: Vec::new(),
        }
    }

    /// Append an offer.
    pub fn push(&mut self, name: &str, params: Params) {
        self.in_order.push(Offer {
            name: name.to_string(),
            params,
        });
    }

    /// Iterate every offer in wire order.
    pub fn iter(&self) -> std::slice::Iter<'_, Offer> {
        self.in_order.iter()
    }

    /// Return every parameter set parsed under `name`, in order.
    ///
    /// An unknown name yields an empty vector.
    pub fn by_name(&self, name: &str) -> Vec<&Params> {
        self.in_order
            .iter()
            .filter(|offer| offer.name == name)
            .map(|offer| &offer.params)
            .collect()
    }

    /// View every offer in wire order as a slice.
    pub fn as_slice(&self) -> &[Offer] {
        &self.in_order
    }
}

impl std::ops::Deref for Offers {
    type Target = [Offer];

    fn deref(&self) -> &Self::Target {
        &self.in_order
    }
}

impl<'a> IntoIterator for &'a Offers {
    type Item = &'a Offer;
    type IntoIter = std::slice::Iter<'a, Offer>;

    fn into_iter(self) -> Self::IntoIter {
        self.in_order.iter()
    }
}

/// A malformed `Sec-WebSocket-Extensions` header.
///
/// Carries the offending header so the error message can quote it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    header: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Invalid Sec-WebSocket-Extensions header: {}",
            self.header
        )
    }
}

impl std::error::Error for ParseError {}

/// True when `c` is an RFC 7230 `tchar`.
///
/// The set is `! # $ % & ' * + - . ^ _ ` | ~` plus ASCII digits and letters.
fn is_token_char(c: char) -> bool {
    matches!(
        c,
        '!' | '#' | '$' | '%' | '&' | '\'' | '*' | '+' | '-' | '.' | '^' | '_' | '`' | '|' | '~'
    ) || c.is_ascii_digit()
        || c.is_ascii_alphabetic()
}

/// True when the value matches the numeric grammar `^-?(0|[1-9][0-9]*)(\.[0-9]+)?$`.
///
/// Leading zeros are rejected, so `01` and `00` stay strings. There is no
/// exponent, no `+` sign, and no bare leading or trailing dot.
fn is_number(s: &str) -> bool {
    let bytes = s.as_bytes();
    let mut i = 0;
    let n = bytes.len();
    if n == 0 {
        return false;
    }
    if bytes[i] == b'-' {
        i += 1;
    }
    // Integer part: a lone 0, or a nonzero digit followed by more digits.
    if i >= n {
        return false;
    }
    if bytes[i] == b'0' {
        i += 1;
    } else if bytes[i].is_ascii_digit() {
        i += 1;
        while i < n && bytes[i].is_ascii_digit() {
            i += 1;
        }
    } else {
        return false;
    }
    // Optional fractional part.
    if i < n {
        if bytes[i] != b'.' {
            return false;
        }
        i += 1;
        if i >= n || !bytes[i].is_ascii_digit() {
            return false;
        }
        while i < n && bytes[i].is_ascii_digit() {
            i += 1;
        }
    }
    i == n
}

/// Convert a token or quoted-string value into a typed [`Value`].
///
/// A value matching the numeric grammar becomes a number, unless its magnitude
/// overflows f64, in which case it stays a string so it round-trips instead of
/// serializing as "Infinity". Everything else stays a string. This runs for
/// both unquoted and quoted values.
fn coerce(data: String) -> Value {
    if is_number(&data) {
        if let Ok(n) = data.parse::<f64>() {
            if n.is_finite() {
                return Value::Number(n);
            }
        }
    }
    Value::Str(data)
}

/// A linear scanner over a header string.
///
/// The scanner walks the grammar directly. It never backtracks, so a long run
/// of escapes inside an unterminated quote fails fast rather than blowing up.
struct Scanner<'a> {
    src: &'a [u8],
    pos: usize,
}

impl<'a> Scanner<'a> {
    fn new(src: &'a str) -> Self {
        Scanner {
            src: src.as_bytes(),
            pos: 0,
        }
    }

    fn peek(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }

    fn at_end(&self) -> bool {
        self.pos >= self.src.len()
    }

    /// Skip ASCII spaces.
    fn skip_spaces(&mut self) {
        while self.peek() == Some(b' ') {
            self.pos += 1;
        }
    }

    /// Read a token. Returns `None` when no token char is present.
    fn read_token(&mut self) -> Option<String> {
        let start = self.pos;
        while let Some(c) = self.peek() {
            if is_token_char(c as char) {
                self.pos += 1;
            } else {
                break;
            }
        }
        if self.pos == start {
            None
        } else {
            // Token chars are ASCII, so the slice is valid UTF-8.
            Some(String::from_utf8(self.src[start..self.pos].to_vec()).unwrap())
        }
    }

    /// Read a quoted string. Assumes the opening quote is the next byte.
    ///
    /// The content allows an escape `\` plus any ASCII byte, or any byte that is
    /// not a control char, `"`, or `\`. Every backslash is dropped from the
    /// value, whatever follows it. So `\"` yields `"`, `\a` yields `a`, and the
    /// two-byte sequence `\\` yields nothing. Returns `None` on a malformed or
    /// unterminated string.
    fn read_quoted(&mut self) -> Option<String> {
        if self.peek() != Some(b'"') {
            return None;
        }
        self.pos += 1;
        let mut out: Vec<u8> = Vec::new();
        loop {
            match self.peek() {
                None => return None, // unterminated
                Some(b'"') => {
                    self.pos += 1;
                    // The value may carry non-ASCII bytes, so decode via
                    // from_utf8 and reject an invalid sequence.
                    return String::from_utf8(out).ok();
                }
                Some(b'\\') => {
                    // An escape unit is the backslash plus the next byte, which
                    // must be ASCII. Drop the backslash. Keep the escaped byte
                    // unless it is itself a backslash, which is also dropped. So
                    // `\"` yields `"`, `\a` yields `a`, and `\\` yields nothing.
                    self.pos += 1;
                    match self.peek() {
                        Some(b'\\') => self.pos += 1,
                        Some(c) if c <= 0x7f => {
                            out.push(c);
                            self.pos += 1;
                        }
                        _ => return None,
                    }
                }
                Some(c) => {
                    // Reject control chars. Backslash and the closing quote are
                    // handled above. Allowed: any byte except 0x00-0x08,
                    // 0x0a-0x1f, 0x7f, '"', '\'.
                    let is_ctl = c <= 0x08 || (0x0a..=0x1f).contains(&c) || c == 0x7f;
                    if is_ctl {
                        return None;
                    }
                    out.push(c);
                    self.pos += 1;
                }
            }
        }
    }
}

/// Validate the whole header against the anchored grammar.
///
/// The header is a comma-separated list of extensions. Each extension is a name
/// followed by optional `; param` pairs, with optional spaces around the
/// separators. Returns false on any deviation.
fn valid_header(header: &str) -> bool {
    let mut s = Scanner::new(header);

    loop {
        // One extension.
        if s.read_token().is_none() {
            return false;
        }
        // Zero or more "; param" pairs.
        loop {
            let mark = s.pos;
            s.skip_spaces();
            if s.peek() != Some(b';') {
                s.pos = mark;
                break;
            }
            s.pos += 1; // consume ';'
            s.skip_spaces();
            // param = token [ "=" ( token | quoted-string ) ]
            if s.read_token().is_none() {
                return false;
            }
            if s.peek() == Some(b'=') {
                s.pos += 1;
                let ok = if s.peek() == Some(b'"') {
                    s.read_quoted().is_some()
                } else {
                    s.read_token().is_some()
                };
                if !ok {
                    return false;
                }
            }
        }
        // Optional ", " then another extension, else end.
        let mark = s.pos;
        s.skip_spaces();
        if s.peek() == Some(b',') {
            s.pos += 1;
            s.skip_spaces();
            continue;
        }
        s.pos = mark;
        break;
    }

    s.at_end()
}

/// Parse a `Sec-WebSocket-Extensions` header into [`Offers`].
///
/// `None` and `Some("")` both yield an empty offer list. The parser preserves
/// offer order, preserves parameter insertion order, coerces numeric values,
/// and collapses repeated parameter keys into lists.
///
/// # Errors
///
/// Returns [`ParseError`] when the header does not match the grammar. The error
/// carries the offending header for the message.
///
/// # Examples
///
/// ```
/// use websocket_extensions::parser::{parse_header, Slot, Value};
///
/// let offers = parse_header(Some("a; b=1, c")).unwrap();
/// assert_eq!(offers[0].name, "a");
/// assert_eq!(offers[0].params.get("b"), Some(&Slot::One(Value::Number(1.0))));
/// assert_eq!(offers[1].name, "c");
/// ```
pub fn parse_header(header: Option<&str>) -> Result<Offers, ParseError> {
    let header = match header {
        None | Some("") => return Ok(Offers::new()),
        Some(h) => h,
    };

    if !valid_header(header) {
        return Err(ParseError {
            header: header.to_string(),
        });
    }

    let mut offers = Offers::new();
    let mut s = Scanner::new(header);

    loop {
        // Extension name.
        let name = s.read_token().expect("validated header has a name");
        let mut params = Params::new();

        // Parameters.
        loop {
            let mark = s.pos;
            s.skip_spaces();
            if s.peek() != Some(b';') {
                s.pos = mark;
                break;
            }
            s.pos += 1;
            s.skip_spaces();
            let key = s.read_token().expect("validated header has a param key");
            let value = if s.peek() == Some(b'=') {
                s.pos += 1;
                if s.peek() == Some(b'"') {
                    coerce(
                        s.read_quoted()
                            .expect("validated header has a quoted value"),
                    )
                } else {
                    coerce(s.read_token().expect("validated header has a token value"))
                }
            } else {
                Value::Bool(true)
            };
            params.insert(&key, value);
        }

        offers.push(&name, params);

        // Next extension or end. A validated header ends here or at a comma.
        s.skip_spaces();
        if s.peek() == Some(b',') {
            s.pos += 1;
            s.skip_spaces();
            continue;
        }
        break;
    }

    Ok(offers)
}

/// Serialize one extension name and its parameters into a header fragment.
///
/// Each value prints by shape:
///
/// - A list prints the key once per element.
/// - `true` prints as a bare key.
/// - A number prints unquoted.
/// - A string with any non-token character is quoted, and inner `"` is escaped.
/// - Any other string prints unquoted.
///
/// The quoting decision is independent per value. A string is quoted whenever
/// it holds a non-token character, so two values in one call that both need
/// quoting both get quoted. Empty parameters yield just the name.
///
/// # Examples
///
/// ```
/// use websocket_extensions::parser::{serialize_params, Params, Value};
///
/// let mut params = Params::new();
/// params.insert("b", Value::Bool(true));
/// params.insert("c", Value::Number(1.0));
/// params.insert("d", Value::Str("hi".into()));
/// assert_eq!(serialize_params("a", &params), "a; b; c=1; d=hi");
/// ```
pub fn serialize_params(name: &str, params: &Params) -> String {
    let mut values: Vec<String> = Vec::new();

    for (key, slot) in params.iter() {
        match slot {
            Slot::One(value) => print_value(&mut values, key, value),
            Slot::Many(list) => {
                for value in list {
                    print_value(&mut values, key, value);
                }
            }
        }
    }

    if values.is_empty() {
        name.to_string()
    } else {
        format!("{}; {}", name, values.join("; "))
    }
}

/// Emit one `key` / `value` pair into `out`.
fn print_value(out: &mut Vec<String>, key: &str, value: &Value) {
    if value.is_true() {
        out.push(key.to_string());
    } else if let Value::Number(n) = value {
        out.push(format!("{}={}", key, format_number(*n)));
    } else if let Value::Str(s) = value {
        if s.chars().any(|c| !is_token_char(c)) {
            let escaped = s.replace('"', "\\\"");
            out.push(format!("{}=\"{}\"", key, escaped));
        } else {
            out.push(format!("{}={}", key, s));
        }
    }
    // Value::Bool(false) never occurs from parsing and prints nothing.
}

/// Format a numeric value for a header.
///
/// Integer-valued numbers print without a decimal point, including values past
/// the `i64` range. Other finite values use the shortest round-tripping form,
/// which `f64`'s default formatting provides. `NaN` and the infinities print as
/// `NaN`, `Infinity`, and `-Infinity`.
fn format_number(n: f64) -> String {
    if n.is_nan() {
        "NaN".to_string()
    } else if n.is_infinite() {
        if n > 0.0 {
            "Infinity".to_string()
        } else {
            "-Infinity".to_string()
        }
    } else {
        // f64's default formatting prints an integer value with no decimal
        // point and a fraction in its shortest form, so 1.0 prints as 1 and
        // 1.5 prints as 1.5. No cast to a narrower integer type, which would
        // saturate for large values.
        format!("{}", n)
    }
}
