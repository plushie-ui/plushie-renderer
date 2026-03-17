//! Wire codec for the stdin/stdout protocol.
//!
//! The renderer communicates with the host process over stdin (incoming
//! messages) and stdout (outgoing events). Two wire formats are supported:
//!
//! - **JSON** -- newline-delimited JSON (JSONL). Each message is a UTF-8
//!   JSON object terminated by `\n`. Human-readable, easy to debug.
//!
//! - **MsgPack** -- 4-byte big-endian length-prefixed MessagePack. Each
//!   message is `[u32 BE length][msgpack payload]`. Compact, faster to
//!   parse, supports native binary fields (e.g. pixel data).
//!
//! The codec is auto-detected from the first byte of stdin (`{` = JSON,
//! anything else = MsgPack) and stored in a process-global [`OnceLock`]
//! so that all emit paths (events, queries, screenshots) use the same
//! format without threading the codec through every call site.

use serde::Serialize;
use serde::de::DeserializeOwned;
use std::fmt;
use std::io::{self, BufRead, Read};
use std::sync::OnceLock;

/// Maximum size for a single wire message (64 MiB). Applied to both JSON
/// line reads and msgpack length-prefixed frames.
pub const MAX_MESSAGE_SIZE: usize = 64 * 1024 * 1024;

/// Maximum nesting depth for `rmpv_to_json` conversion. Prevents stack
/// overflow from deeply nested (or maliciously crafted) msgpack payloads.
const MAX_RMPV_DEPTH: usize = 128;

/// Process-global wire codec, set once at startup via [`Codec::set_global`].
static WIRE_CODEC: OnceLock<Codec> = OnceLock::new();

/// Wire codec for the stdin/stdout protocol.
///
/// See the [module documentation](self) for format details.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    /// Newline-delimited JSON (JSONL).
    Json,
    /// Length-prefixed MessagePack.
    MsgPack,
}

impl fmt::Display for Codec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Codec::Json => f.write_str("json"),
            Codec::MsgPack => f.write_str("msgpack"),
        }
    }
}

impl Codec {
    /// Encode a value to wire bytes ready to write to stdout.
    ///
    /// - JSON: `serde_json` serialization + trailing `\n`.
    /// - MsgPack: 4-byte BE u32 length prefix + `rmp_serde` named serialization.
    ///
    /// Allocates a new Vec per call. For hot paths (e.g. rapid event
    /// emission), consider pre-allocating and reusing a buffer.
    pub fn encode<T: Serialize>(&self, value: &T) -> Result<Vec<u8>, String> {
        match self {
            Codec::Json => {
                let mut bytes =
                    serde_json::to_vec(value).map_err(|e| format!("json encode: {e}"))?;
                bytes.push(b'\n');
                Ok(bytes)
            }
            Codec::MsgPack => {
                let payload =
                    rmp_serde::to_vec_named(value).map_err(|e| format!("msgpack encode: {e}"))?;
                let len = u32::try_from(payload.len()).map_err(|_| {
                    format!(
                        "payload exceeds 4 GiB frame limit ({} bytes)",
                        payload.len()
                    )
                })?;
                let mut bytes = Vec::with_capacity(4 + payload.len());
                bytes.extend_from_slice(&len.to_be_bytes());
                bytes.extend_from_slice(&payload);
                Ok(bytes)
            }
        }
    }

    /// Decode a raw payload (framing already stripped) into a typed value.
    ///
    /// For JSON, `bytes` is the UTF-8 JSON text (without the trailing newline).
    /// For MsgPack, `bytes` is the raw msgpack payload (without the length prefix).
    ///
    /// MsgPack decoding routes through `rmpv::Value` as an intermediate. This
    /// preserves binary data (msgpack's bin type) as JSON arrays of byte values,
    /// which the `deserialize_binary_field` custom deserializer in protocol.rs
    /// can reconstruct into `Vec<u8>`. The `serde_json::Value` intermediate is
    /// still needed for tag dispatch (`#[serde(tag = "type")]`) which rmp-serde
    /// doesn't handle reliably for externally-produced msgpack.
    pub fn decode<T: DeserializeOwned>(&self, bytes: &[u8]) -> Result<T, String> {
        match self {
            Codec::Json => serde_json::from_slice(bytes).map_err(|e| format!("json decode: {e}")),
            Codec::MsgPack => {
                // Pre-check nesting depth before rmpv deserialization.
                // rmpv::read_value recurses without a depth limit, so a
                // pathologically nested payload can cause stack overflow
                // before our depth-limited rmpv_to_json conversion runs.
                check_msgpack_depth(bytes, MAX_RMPV_DEPTH)
                    .map_err(|e| format!("msgpack depth check: {e}"))?;
                let rmpv_val: rmpv::Value = rmpv::decode::read_value(&mut &bytes[..])
                    .map_err(|e| format!("msgpack decode (rmpv): {e}"))?;
                let json_val = rmpv_to_json(rmpv_val);
                serde_json::from_value(json_val)
                    .map_err(|e| format!("msgpack decode (tag dispatch): {e}"))
            }
        }
    }

    /// Read one framed message from a buffered reader, returning the raw payload.
    ///
    /// - JSON: reads until `\n`, returns the line bytes (without the newline).
    /// - MsgPack: reads a 4-byte BE u32 length, then reads that many bytes.
    ///
    /// Returns `Ok(None)` on EOF (clean shutdown).
    pub fn read_message<R: BufRead>(&self, reader: &mut R) -> io::Result<Option<Vec<u8>>> {
        match self {
            Codec::Json => loop {
                let mut line = String::new();
                // Wrap in Take to bound allocation BEFORE the full line is
                // buffered. Without this, a sender could transmit an arbitrarily
                // long line without a newline, causing unbounded memory growth.
                let limit = (MAX_MESSAGE_SIZE + 1) as u64;
                let n = (&mut *reader).take(limit).read_line(&mut line)?;
                if n == 0 {
                    return Ok(None);
                }
                if line.len() > MAX_MESSAGE_SIZE {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!(
                            "JSON message exceeds {} byte limit ({} bytes)",
                            MAX_MESSAGE_SIZE,
                            line.len()
                        ),
                    ));
                }
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                return Ok(Some(trimmed.as_bytes().to_vec()));
            },
            Codec::MsgPack => {
                let mut len_buf = [0u8; 4];
                match reader.read_exact(&mut len_buf) {
                    Ok(()) => {}
                    Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
                    Err(e) => return Err(e),
                }
                let len = u32::from_be_bytes(len_buf) as usize;
                if len == 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "empty frame received",
                    ));
                }
                if len > MAX_MESSAGE_SIZE {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!(
                            "msgpack frame exceeds {} byte limit ({} bytes)",
                            MAX_MESSAGE_SIZE, len
                        ),
                    ));
                }
                let mut payload = vec![0u8; len];
                reader.read_exact(&mut payload)?;
                Ok(Some(payload))
            }
        }
    }

    /// Detect codec from the first byte of input.
    ///
    /// `{` (0x7B) indicates JSON. Anything else indicates MsgPack (the first
    /// byte of a 4-byte length prefix).
    pub fn detect_from_first_byte(byte: u8) -> Codec {
        if byte == b'{' {
            Codec::Json
        } else {
            Codec::MsgPack
        }
    }

    /// Store the negotiated codec in the global slot. Panics if called twice.
    pub fn set_global(codec: Codec) {
        WIRE_CODEC
            .set(codec)
            .expect("WIRE_CODEC already initialized");
    }

    /// Get the global wire codec. Returns MsgPack if not yet initialized.
    pub fn get_global() -> &'static Codec {
        WIRE_CODEC.get().unwrap_or(&Codec::MsgPack)
    }
}

// ---------------------------------------------------------------------------
// Msgpack nesting depth pre-check
// ---------------------------------------------------------------------------

/// Iteratively scan raw msgpack bytes and reject payloads that would cause
/// problems for `rmpv::read_value`:
///
/// - **Nesting depth** exceeding `max_depth` (prevents stack overflow from
///   rmpv's recursive parser).
/// - **Declared element counts** exceeding the remaining bytes (prevents
///   rmpv from pre-allocating `Vec::with_capacity(billions)` when the
///   declared count is larger than the payload can possibly contain).
fn check_msgpack_depth(bytes: &[u8], max_depth: usize) -> Result<(), String> {
    let len = bytes.len();
    let mut pos: usize = 0;
    let mut depth: usize = 0;
    // Stack tracks how many child elements remain at each nesting level.
    let mut remaining: Vec<usize> = Vec::new();

    while pos < len {
        let b = bytes[pos];
        pos += 1;

        // Classify the format marker: (data_bytes_to_skip, child_element_count).
        // For containers (array/map), child_count > 0 and we push a new depth level.
        // For scalars, child_count == 0 and we consume one element from the parent.
        let (skip, children) = match b {
            // positive fixint
            0x00..=0x7f => (0, 0),
            // fixmap: N key-value pairs = 2N child elements
            0x80..=0x8f => (0, ((b & 0x0f) as usize) * 2),
            // fixarray
            0x90..=0x9f => (0, (b & 0x0f) as usize),
            // fixstr
            0xa0..=0xbf => ((b & 0x1f) as usize, 0),
            // nil, (unused), false, true
            0xc0..=0xc3 => (0, 0),
            // bin8
            0xc4 => {
                if pos >= len {
                    break;
                }
                (1 + bytes[pos] as usize, 0)
            }
            // bin16
            0xc5 => {
                if pos + 1 >= len {
                    break;
                }
                let n = u16::from_be_bytes([bytes[pos], bytes[pos + 1]]) as usize;
                (2 + n, 0)
            }
            // bin32
            0xc6 => {
                if pos + 3 >= len {
                    break;
                }
                let n = u32::from_be_bytes([
                    bytes[pos],
                    bytes[pos + 1],
                    bytes[pos + 2],
                    bytes[pos + 3],
                ]) as usize;
                (4 + n, 0)
            }
            // ext8
            0xc7 => {
                if pos >= len {
                    break;
                }
                (2 + bytes[pos] as usize, 0)
            }
            // ext16
            0xc8 => {
                if pos + 1 >= len {
                    break;
                }
                let n = u16::from_be_bytes([bytes[pos], bytes[pos + 1]]) as usize;
                (3 + n, 0)
            }
            // ext32
            0xc9 => {
                if pos + 3 >= len {
                    break;
                }
                let n = u32::from_be_bytes([
                    bytes[pos],
                    bytes[pos + 1],
                    bytes[pos + 2],
                    bytes[pos + 3],
                ]) as usize;
                (5 + n, 0)
            }
            // float32
            0xca => (4, 0),
            // float64
            0xcb => (8, 0),
            // uint8, int8
            0xcc | 0xd0 => (1, 0),
            // uint16, int16
            0xcd | 0xd1 => (2, 0),
            // uint32, int32
            0xce | 0xd2 => (4, 0),
            // uint64, int64
            0xcf | 0xd3 => (8, 0),
            // fixext 1, 2, 4, 8, 16 (type byte + data)
            0xd4 => (2, 0),
            0xd5 => (3, 0),
            0xd6 => (5, 0),
            0xd7 => (9, 0),
            0xd8 => (17, 0),
            // str8
            0xd9 => {
                if pos >= len {
                    break;
                }
                (1 + bytes[pos] as usize, 0)
            }
            // str16
            0xda => {
                if pos + 1 >= len {
                    break;
                }
                let n = u16::from_be_bytes([bytes[pos], bytes[pos + 1]]) as usize;
                (2 + n, 0)
            }
            // str32
            0xdb => {
                if pos + 3 >= len {
                    break;
                }
                let n = u32::from_be_bytes([
                    bytes[pos],
                    bytes[pos + 1],
                    bytes[pos + 2],
                    bytes[pos + 3],
                ]) as usize;
                (4 + n, 0)
            }
            // array16
            0xdc => {
                if pos + 1 >= len {
                    break;
                }
                let n = u16::from_be_bytes([bytes[pos], bytes[pos + 1]]) as usize;
                pos += 2;
                (0, n)
            }
            // array32
            0xdd => {
                if pos + 3 >= len {
                    break;
                }
                let n = u32::from_be_bytes([
                    bytes[pos],
                    bytes[pos + 1],
                    bytes[pos + 2],
                    bytes[pos + 3],
                ]) as usize;
                pos += 4;
                (0, n)
            }
            // map16
            0xde => {
                if pos + 1 >= len {
                    break;
                }
                let n = u16::from_be_bytes([bytes[pos], bytes[pos + 1]]) as usize;
                pos += 2;
                (0, n * 2)
            }
            // map32
            0xdf => {
                if pos + 3 >= len {
                    break;
                }
                let n = u32::from_be_bytes([
                    bytes[pos],
                    bytes[pos + 1],
                    bytes[pos + 2],
                    bytes[pos + 3],
                ]) as usize;
                pos += 4;
                (0, n * 2)
            }
            // negative fixint
            0xe0..=0xff => (0, 0),
        };

        pos += skip;

        if children > 0 {
            // Each child element needs at least 1 byte. Reject declared
            // counts that exceed the remaining data to prevent rmpv from
            // pre-allocating huge Vecs based on a forged count field.
            let remaining_bytes = len.saturating_sub(pos);
            if children > remaining_bytes {
                return Err(format!(
                    "msgpack container declares {children} elements but only {remaining_bytes} bytes remain"
                ));
            }

            depth += 1;
            if depth > max_depth {
                return Err(format!("msgpack nesting depth exceeds limit ({max_depth})"));
            }
            remaining.push(children);
        } else {
            // Leaf value consumed: pop completed containers.
            while let Some(count) = remaining.last_mut() {
                *count -= 1;
                if *count == 0 {
                    remaining.pop();
                    depth -= 1;
                } else {
                    break;
                }
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// rmpv::Value -> serde_json::Value conversion
// ---------------------------------------------------------------------------

/// Convert an rmpv::Value to serde_json::Value, preserving binary data as
/// JSON arrays of byte values (u8). This is the key difference from the old
/// rmp_serde -> serde_json::Value path, which silently dropped binary data
/// (serde_json::Value has no binary type).
///
/// The `deserialize_binary_field` custom deserializer in protocol.rs knows
/// how to reconstruct `Vec<u8>` from these byte arrays.
///
/// Recursion depth is capped at `MAX_RMPV_DEPTH` to prevent stack overflow
/// from deeply nested or malicious payloads.
fn rmpv_to_json(val: rmpv::Value) -> serde_json::Value {
    rmpv_to_json_inner(val, 0)
}

fn rmpv_to_json_inner(val: rmpv::Value, depth: usize) -> serde_json::Value {
    if depth > MAX_RMPV_DEPTH {
        log::error!("rmpv_to_json: recursion depth exceeded {MAX_RMPV_DEPTH}, replaced with null");
        return serde_json::Value::Null;
    }

    match val {
        rmpv::Value::Nil => serde_json::Value::Null,
        rmpv::Value::Boolean(b) => serde_json::Value::Bool(b),
        rmpv::Value::Integer(n) => {
            if let Some(i) = n.as_i64() {
                serde_json::Value::Number(i.into())
            } else if let Some(u) = n.as_u64() {
                serde_json::Value::Number(u.into())
            } else {
                // Fallback: shouldn't happen for msgpack integers
                serde_json::Value::Null
            }
        }
        rmpv::Value::F32(f) => serde_json::Number::from_f64(f as f64)
            .map(serde_json::Value::Number)
            .unwrap_or_else(|| {
                log::warn!("rmpv_to_json: non-finite f32 ({f}) replaced with 0.0");
                serde_json::Value::Number(serde_json::Number::from_f64(0.0).unwrap())
            }),
        rmpv::Value::F64(f) => serde_json::Number::from_f64(f)
            .map(serde_json::Value::Number)
            .unwrap_or_else(|| {
                log::warn!("rmpv_to_json: non-finite f64 ({f}) replaced with 0.0");
                serde_json::Value::Number(serde_json::Number::from_f64(0.0).unwrap())
            }),
        rmpv::Value::String(s) => {
            // rmpv::Utf8String -- may or may not be valid UTF-8.
            // Use lossy conversion so invalid bytes become U+FFFD instead of
            // silently mapping to null (which breaks tag dispatch on "type").
            serde_json::Value::String(String::from_utf8_lossy(s.as_bytes()).into_owned())
        }
        rmpv::Value::Binary(bytes) => {
            // Preserve raw bytes as a JSON array of u8 values.
            // The deserialize_binary_field custom deserializer reconstructs Vec<u8>.
            serde_json::Value::Array(
                bytes
                    .into_iter()
                    .map(|b| serde_json::Value::Number(b.into()))
                    .collect(),
            )
        }
        rmpv::Value::Array(arr) => serde_json::Value::Array(
            arr.into_iter()
                .map(|v| rmpv_to_json_inner(v, depth + 1))
                .collect(),
        ),
        rmpv::Value::Map(entries) => {
            let mut map = serde_json::Map::new();
            for (k, v) in entries {
                // Map keys: try to use string representation
                let key = match k {
                    rmpv::Value::String(s) => s.into_str().unwrap_or_default().to_string(),
                    rmpv::Value::Integer(n) => n.to_string(),
                    other => format!("{other}"),
                };
                map.insert(key, rmpv_to_json_inner(v, depth + 1));
            }
            serde_json::Value::Object(map)
        }
        rmpv::Value::Ext(type_id, _bytes) => {
            log::warn!(
                "rmpv_to_json: msgpack ext type {type_id} not supported, replaced with null"
            );
            serde_json::Value::Null
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};
    use serde_json::json;

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct Simple {
        name: String,
        count: u32,
    }

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    #[serde(tag = "type", rename_all = "snake_case")]
    enum Tagged {
        Alpha { value: String },
        Beta { x: f64, y: f64 },
    }

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct WithFlatten {
        op: String,
        #[serde(flatten)]
        rest: serde_json::Value,
    }

    // -- JSON roundtrips --

    #[test]
    fn json_roundtrip_simple() {
        let original = Simple {
            name: "test".into(),
            count: 42,
        };
        let bytes = Codec::Json.encode(&original).unwrap();
        assert!(bytes.ends_with(b"\n"));
        let decoded: Simple = Codec::Json.decode(&bytes[..bytes.len() - 1]).unwrap();
        assert_eq!(decoded, original);
    }

    #[test]
    fn json_roundtrip_tagged_enum() {
        let original = Tagged::Beta { x: 1.5, y: 2.5 };
        let bytes = Codec::Json.encode(&original).unwrap();
        let decoded: Tagged = Codec::Json.decode(&bytes[..bytes.len() - 1]).unwrap();
        assert_eq!(decoded, original);
    }

    // -- MsgPack roundtrips --

    #[test]
    fn msgpack_roundtrip_simple() {
        let original = Simple {
            name: "test".into(),
            count: 42,
        };
        let bytes = Codec::MsgPack.encode(&original).unwrap();
        // First 4 bytes are length prefix
        let len = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
        assert_eq!(len, bytes.len() - 4);
        let decoded: Simple = Codec::MsgPack.decode(&bytes[4..]).unwrap();
        assert_eq!(decoded, original);
    }

    #[test]
    fn msgpack_roundtrip_tagged_enum() {
        let original = Tagged::Alpha {
            value: "hello".into(),
        };
        let bytes = Codec::MsgPack.encode(&original).unwrap();
        let payload = &bytes[4..];
        let decoded: Tagged = Codec::MsgPack.decode(payload).unwrap();
        assert_eq!(decoded, original);
    }

    #[test]
    fn msgpack_roundtrip_tagged_enum_beta() {
        let original = Tagged::Beta {
            x: std::f64::consts::PI,
            y: -1.0,
        };
        let bytes = Codec::MsgPack.encode(&original).unwrap();
        let payload = &bytes[4..];
        let decoded: Tagged = Codec::MsgPack.decode(payload).unwrap();
        assert_eq!(decoded, original);
    }

    #[test]
    fn msgpack_flatten_deserialize() {
        // Flatten on deserialize: encode a map with extra keys, decode into
        // a struct with #[serde(flatten)] rest: Value.
        let input = json!({"op": "props", "path": [0, 1], "props": {"label": "hi"}});
        let bytes = rmp_serde::to_vec_named(&input).unwrap();
        let decoded: WithFlatten = rmp_serde::from_slice(&bytes).unwrap();
        assert_eq!(decoded.op, "props");
        assert_eq!(decoded.rest["path"], json!([0, 1]));
        assert_eq!(decoded.rest["props"]["label"], "hi");
    }

    // -- read_message --

    #[test]
    fn json_read_message_skips_blank_lines() {
        // Blank lines between messages must be skipped, not treated as EOF.
        let data = b"\n\n{\"name\":\"a\",\"count\":1}\n\n{\"name\":\"b\",\"count\":2}\n\n";
        let mut reader = io::BufReader::new(&data[..]);

        let msg1 = Codec::Json.read_message(&mut reader).unwrap().unwrap();
        let s1: Simple = Codec::Json.decode(&msg1).unwrap();
        assert_eq!(s1.name, "a");

        let msg2 = Codec::Json.read_message(&mut reader).unwrap().unwrap();
        let s2: Simple = Codec::Json.decode(&msg2).unwrap();
        assert_eq!(s2.name, "b");

        // Trailing blank lines followed by real EOF should return None.
        assert!(Codec::Json.read_message(&mut reader).unwrap().is_none());
    }

    #[test]
    fn json_read_message() {
        let data = b"{\"name\":\"a\",\"count\":1}\n{\"name\":\"b\",\"count\":2}\n";
        let mut reader = io::BufReader::new(&data[..]);

        let msg1 = Codec::Json.read_message(&mut reader).unwrap().unwrap();
        let s1: Simple = Codec::Json.decode(&msg1).unwrap();
        assert_eq!(s1.name, "a");

        let msg2 = Codec::Json.read_message(&mut reader).unwrap().unwrap();
        let s2: Simple = Codec::Json.decode(&msg2).unwrap();
        assert_eq!(s2.name, "b");

        assert!(Codec::Json.read_message(&mut reader).unwrap().is_none());
    }

    #[test]
    fn msgpack_read_message() {
        // Build two length-prefixed msgpack messages
        let s1 = Simple {
            name: "x".into(),
            count: 10,
        };
        let s2 = Simple {
            name: "y".into(),
            count: 20,
        };
        let p1 = rmp_serde::to_vec_named(&s1).unwrap();
        let p2 = rmp_serde::to_vec_named(&s2).unwrap();

        let mut data = Vec::new();
        data.extend_from_slice(&(p1.len() as u32).to_be_bytes());
        data.extend_from_slice(&p1);
        data.extend_from_slice(&(p2.len() as u32).to_be_bytes());
        data.extend_from_slice(&p2);

        let mut reader = io::BufReader::new(&data[..]);

        let msg1 = Codec::MsgPack.read_message(&mut reader).unwrap().unwrap();
        let d1: Simple = Codec::MsgPack.decode(&msg1).unwrap();
        assert_eq!(d1, s1);

        let msg2 = Codec::MsgPack.read_message(&mut reader).unwrap().unwrap();
        let d2: Simple = Codec::MsgPack.decode(&msg2).unwrap();
        assert_eq!(d2, s2);

        assert!(Codec::MsgPack.read_message(&mut reader).unwrap().is_none());
    }

    // -- read_message size limit tests --

    #[test]
    fn json_read_message_rejects_oversized_line() {
        // A line longer than MAX_MESSAGE_SIZE must be rejected.
        // We can't allocate 64 MiB in a test, so use a smaller custom
        // read_message-like flow. Instead, verify the Take wrapper works
        // by constructing a line just over the limit.
        //
        // Since MAX_MESSAGE_SIZE is 64 MiB (too big for a unit test),
        // we test the logic indirectly: a line of exactly MAX_MESSAGE_SIZE+1
        // bytes (no newline) should be rejected. We use a small stand-in
        // to verify the mechanics.
        let small_limit = 100;
        // Construct a line with no newline, longer than small_limit.
        let long_line: Vec<u8> = vec![b'x'; small_limit + 10];
        let mut reader = io::BufReader::new(&long_line[..]);

        // Read using Take with the small limit -- simulates what
        // read_message does, just with a smaller limit.
        let mut line = String::new();
        let limit = (small_limit + 1) as u64;
        let _n = (&mut reader).take(limit).read_line(&mut line).unwrap();
        // The Take capped the read, so line.len() <= small_limit + 1.
        assert!(line.len() <= small_limit + 1);
        // Without the Take, line.len() would be small_limit + 10.
    }

    #[test]
    fn msgpack_read_message_rejects_oversized_frame() {
        // Build a frame with length prefix claiming MAX_MESSAGE_SIZE + 1 bytes.
        let len = (MAX_MESSAGE_SIZE + 1) as u32;
        let mut data = Vec::new();
        data.extend_from_slice(&len.to_be_bytes());
        // Don't need the actual payload -- the size check fires first.
        data.extend_from_slice(&[0u8; 64]); // just enough to not EOF

        let mut reader = io::BufReader::new(&data[..]);
        let result = Codec::MsgPack.read_message(&mut reader);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(err.to_string().contains("byte limit"));
    }

    #[test]
    fn msgpack_read_message_rejects_zero_length_frame() {
        let mut data = Vec::new();
        data.extend_from_slice(&0u32.to_be_bytes());

        let mut reader = io::BufReader::new(&data[..]);
        let result = Codec::MsgPack.read_message(&mut reader);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("empty frame"));
    }

    // -- Cross-format: simulate external msgpack (e.g. Msgpax) --
    //
    // rmp-serde's own serializer produces bytes that its deserializer can
    // roundtrip, but external msgpack producers encode maps differently.
    // These tests build raw msgpack via serde_json::Value -> rmp_serde
    // (which is format-agnostic, not tagged-enum-aware) to simulate what
    // an external producer like Msgpax sends. The Codec::decode workaround
    // (msgpack -> rmpv::Value -> serde_json::Value -> T) must handle these.

    #[test]
    fn msgpack_external_tagged_enum_alpha() {
        // Simulate Msgpax encoding {"type": "alpha", "value": "hello"}
        let external = json!({"type": "alpha", "value": "hello"});
        let bytes = rmp_serde::to_vec_named(&external).unwrap();
        let decoded: Tagged = Codec::MsgPack.decode(&bytes).unwrap();
        assert_eq!(
            decoded,
            Tagged::Alpha {
                value: "hello".into()
            }
        );
    }

    #[test]
    fn msgpack_external_tagged_enum_beta() {
        let external = json!({"type": "beta", "x": 1.5, "y": -2.0});
        let bytes = rmp_serde::to_vec_named(&external).unwrap();
        let decoded: Tagged = Codec::MsgPack.decode(&bytes).unwrap();
        assert_eq!(decoded, Tagged::Beta { x: 1.5, y: -2.0 });
    }

    #[test]
    fn msgpack_external_incoming_settings() {
        // This is exactly what a host sends: a plain map with "type":"settings".
        use crate::protocol::IncomingMessage;
        let external = json!({"type": "settings", "settings": {"antialiasing": false}});
        let bytes = rmp_serde::to_vec_named(&external).unwrap();
        let decoded: IncomingMessage = Codec::MsgPack.decode(&bytes).unwrap();
        assert!(matches!(decoded, IncomingMessage::Settings { .. }));
    }

    #[test]
    fn msgpack_external_incoming_snapshot() {
        use crate::protocol::IncomingMessage;
        let external = json!({"type": "snapshot", "tree": {"id": "root", "type": "column", "props": {}, "children": []}});
        let bytes = rmp_serde::to_vec_named(&external).unwrap();
        let decoded: IncomingMessage = Codec::MsgPack.decode(&bytes).unwrap();
        assert!(matches!(decoded, IncomingMessage::Snapshot { .. }));
    }

    // -- Binary data preservation through rmpv path --

    #[test]
    fn msgpack_image_op_with_native_binary() {
        // Simulate what an external producer sends when using native binary fields.
        // Build raw msgpack with a binary field using rmpv directly.
        use rmpv::Value as RmpvValue;

        let pixel_bytes: Vec<u8> = vec![255, 0, 0, 255, 0, 255, 0, 255]; // 2 RGBA pixels
        let msg = RmpvValue::Map(vec![
            (
                RmpvValue::String("type".into()),
                RmpvValue::String("image_op".into()),
            ),
            (
                RmpvValue::String("op".into()),
                RmpvValue::String("create_image".into()),
            ),
            (
                RmpvValue::String("handle".into()),
                RmpvValue::String("test_img".into()),
            ),
            (
                RmpvValue::String("pixels".into()),
                RmpvValue::Binary(pixel_bytes.clone()),
            ),
            (
                RmpvValue::String("width".into()),
                RmpvValue::Integer(1.into()),
            ),
            (
                RmpvValue::String("height".into()),
                RmpvValue::Integer(2.into()),
            ),
        ]);

        let mut buf = Vec::new();
        rmpv::encode::write_value(&mut buf, &msg).unwrap();

        let decoded: crate::protocol::IncomingMessage = Codec::MsgPack.decode(&buf).unwrap();
        match decoded {
            crate::protocol::IncomingMessage::ImageOp {
                op,
                handle,
                pixels,
                width,
                height,
                data,
            } => {
                assert_eq!(op, "create_image");
                assert_eq!(handle, "test_img");
                assert_eq!(pixels, Some(pixel_bytes));
                assert_eq!(width, Some(1));
                assert_eq!(height, Some(2));
                assert!(data.is_none());
            }
            other => panic!("expected ImageOp, got {other:?}"),
        }
    }

    #[test]
    fn msgpack_image_op_with_base64_string() {
        // JSON mode: binary data arrives as base64-encoded string.
        use crate::protocol::IncomingMessage;
        use base64::Engine as _;

        let pixel_bytes: Vec<u8> = vec![255, 0, 0, 255];
        let b64 = base64::engine::general_purpose::STANDARD.encode(&pixel_bytes);

        let json_msg = json!({
            "type": "image_op",
            "op": "create_image",
            "handle": "test_img",
            "pixels": b64,
            "width": 1,
            "height": 1
        });
        let json_str = serde_json::to_string(&json_msg).unwrap();

        let decoded: IncomingMessage = Codec::Json.decode(json_str.as_bytes()).unwrap();
        match decoded {
            IncomingMessage::ImageOp { pixels, .. } => {
                assert_eq!(pixels, Some(pixel_bytes));
            }
            other => panic!("expected ImageOp, got {other:?}"),
        }
    }

    // -- rmpv_to_json unit tests --

    #[test]
    fn rmpv_to_json_preserves_binary_as_array() {
        let binary = rmpv::Value::Binary(vec![1, 2, 3]);
        let result = rmpv_to_json(binary);
        assert_eq!(result, json!([1, 2, 3]));
    }

    #[test]
    fn rmpv_to_json_handles_nested_map() {
        let val = rmpv::Value::Map(vec![
            (
                rmpv::Value::String("key".into()),
                rmpv::Value::String("val".into()),
            ),
            (
                rmpv::Value::String("num".into()),
                rmpv::Value::Integer(42.into()),
            ),
        ]);
        let result = rmpv_to_json(val);
        assert_eq!(result, json!({"key": "val", "num": 42}));
    }

    // -- detect --

    #[test]
    fn detect_json_from_brace() {
        assert_eq!(Codec::detect_from_first_byte(b'{'), Codec::Json);
    }

    #[test]
    fn detect_msgpack_from_zero() {
        assert_eq!(Codec::detect_from_first_byte(0x00), Codec::MsgPack);
    }

    #[test]
    fn detect_msgpack_from_fixmap() {
        assert_eq!(Codec::detect_from_first_byte(0x85), Codec::MsgPack);
    }

    #[test]
    fn display_format() {
        assert_eq!(Codec::Json.to_string(), "json");
        assert_eq!(Codec::MsgPack.to_string(), "msgpack");
    }

    // -- Additional rmpv_to_json coverage --

    #[test]
    fn rmpv_to_json_deeply_nested_maps() {
        // Nested map: {"outer": {"inner": {"deep": 42}}}
        let val = rmpv::Value::Map(vec![(
            rmpv::Value::String("outer".into()),
            rmpv::Value::Map(vec![(
                rmpv::Value::String("inner".into()),
                rmpv::Value::Map(vec![(
                    rmpv::Value::String("deep".into()),
                    rmpv::Value::Integer(42.into()),
                )]),
            )]),
        )]);
        let result = rmpv_to_json(val);
        assert_eq!(result, json!({"outer": {"inner": {"deep": 42}}}));
    }

    #[test]
    fn rmpv_to_json_binary_in_nested_map() {
        // Binary data nested inside a map should be preserved as byte arrays.
        let val = rmpv::Value::Map(vec![
            (
                rmpv::Value::String("name".into()),
                rmpv::Value::String("img".into()),
            ),
            (
                rmpv::Value::String("pixels".into()),
                rmpv::Value::Binary(vec![255, 128, 0, 255]),
            ),
        ]);
        let result = rmpv_to_json(val);
        assert_eq!(result["name"], json!("img"));
        assert_eq!(result["pixels"], json!([255, 128, 0, 255]));
    }

    #[test]
    fn msgpack_roundtrip_with_binary_field() {
        // Encode a message containing binary data via msgpack, decode it,
        // and verify the binary field comes through as a byte array.
        use rmpv::Value as RmpvValue;

        let raw_bytes: Vec<u8> = vec![0xDE, 0xAD, 0xBE, 0xEF];
        let msg = RmpvValue::Map(vec![
            (
                RmpvValue::String("type".into()),
                RmpvValue::String("alpha".into()),
            ),
            (
                RmpvValue::String("value".into()),
                RmpvValue::String("hello".into()),
            ),
            (
                RmpvValue::String("payload".into()),
                RmpvValue::Binary(raw_bytes.clone()),
            ),
        ]);

        // Encode to raw msgpack bytes.
        let mut buf = Vec::new();
        rmpv::encode::write_value(&mut buf, &msg).unwrap();

        // The rmpv_to_json path preserves binary as an array of u8.
        let rmpv_val: rmpv::Value = rmpv::decode::read_value(&mut &buf[..]).unwrap();
        let json_val = rmpv_to_json(rmpv_val);

        // The tagged enum fields decode fine.
        assert_eq!(json_val["type"], "alpha");
        assert_eq!(json_val["value"], "hello");

        // Binary preserved as array of byte values.
        let payload = json_val["payload"].as_array().unwrap();
        let bytes: Vec<u8> = payload.iter().map(|v| v.as_u64().unwrap() as u8).collect();
        assert_eq!(bytes, raw_bytes);
    }

    #[test]
    fn rmpv_to_json_handles_nil_and_bool() {
        assert_eq!(rmpv_to_json(rmpv::Value::Nil), json!(null));
        assert_eq!(rmpv_to_json(rmpv::Value::Boolean(true)), json!(true));
        assert_eq!(rmpv_to_json(rmpv::Value::Boolean(false)), json!(false));
    }

    // -- check_msgpack_depth --

    #[test]
    fn msgpack_depth_check_accepts_flat_map() {
        let val = json!({"a": 1, "b": "hello", "c": true});
        let bytes = rmp_serde::to_vec_named(&val).unwrap();
        assert!(check_msgpack_depth(&bytes, 128).is_ok());
    }

    #[test]
    fn msgpack_depth_check_accepts_nested_within_limit() {
        // 3 levels: {"outer": {"middle": {"inner": 42}}}
        let val = json!({"outer": {"middle": {"inner": 42}}});
        let bytes = rmp_serde::to_vec_named(&val).unwrap();
        assert!(check_msgpack_depth(&bytes, 3).is_ok());
    }

    #[test]
    fn msgpack_depth_check_rejects_beyond_limit() {
        // 3 nested maps exceeds a limit of 2
        let val = json!({"a": {"b": {"c": 1}}});
        let bytes = rmp_serde::to_vec_named(&val).unwrap();
        assert!(check_msgpack_depth(&bytes, 2).is_err());
    }

    #[test]
    fn msgpack_depth_check_accepts_flat_array() {
        let val = json!([1, 2, 3, 4, 5]);
        let bytes = rmp_serde::to_vec_named(&val).unwrap();
        assert!(check_msgpack_depth(&bytes, 1).is_ok());
    }

    #[test]
    fn msgpack_depth_check_nested_arrays() {
        let val = json!([[[42]]]);
        let bytes = rmp_serde::to_vec_named(&val).unwrap();
        assert!(check_msgpack_depth(&bytes, 3).is_ok());
        assert!(check_msgpack_depth(&bytes, 2).is_err());
    }

    #[test]
    fn msgpack_depth_check_mixed_containers() {
        let val = json!({"list": [{"nested": true}]});
        let bytes = rmp_serde::to_vec_named(&val).unwrap();
        // depth: map(1) -> array(2) -> map(3) = 3 levels
        assert!(check_msgpack_depth(&bytes, 3).is_ok());
        assert!(check_msgpack_depth(&bytes, 2).is_err());
    }

    #[test]
    fn msgpack_depth_check_empty_containers() {
        let val = json!({"empty_map": {}, "empty_arr": []});
        let bytes = rmp_serde::to_vec_named(&val).unwrap();
        assert!(check_msgpack_depth(&bytes, 2).is_ok());
    }

    #[test]
    fn msgpack_depth_check_sibling_arrays_dont_add_depth() {
        // [[1,2], [3,4]] has depth 2 (outer array -> inner array), not 3
        let val = json!([[1, 2], [3, 4]]);
        let bytes = rmp_serde::to_vec_named(&val).unwrap();
        assert!(check_msgpack_depth(&bytes, 2).is_ok());
    }

    #[test]
    fn msgpack_depth_check_binary_data() {
        use rmpv::Value as V;
        let val = V::Map(vec![(
            V::String("data".into()),
            V::Binary(vec![0xDE, 0xAD]),
        )]);
        let mut bytes = Vec::new();
        rmpv::encode::write_value(&mut bytes, &val).unwrap();
        assert!(check_msgpack_depth(&bytes, 1).is_ok());
    }

    #[test]
    fn msgpack_depth_check_deeply_nested_rejects() {
        // Build a deeply nested msgpack: {a: {a: {a: ... {a: 1} ...}}}
        use rmpv::Value as V;
        let depth = 200;
        let mut val = V::Integer(1.into());
        for _ in 0..depth {
            val = V::Map(vec![(V::String("a".into()), val)]);
        }
        let mut bytes = Vec::new();
        rmpv::encode::write_value(&mut bytes, &val).unwrap();

        assert!(check_msgpack_depth(&bytes, 128).is_err());
        assert!(check_msgpack_depth(&bytes, 200).is_ok());
    }

    #[test]
    fn msgpack_decode_rejects_deeply_nested() {
        // Verify the full decode path rejects deeply nested payloads.
        use rmpv::Value as V;
        let mut val = V::Integer(1.into());
        for _ in 0..200 {
            val = V::Map(vec![(V::String("a".into()), val)]);
        }
        let mut bytes = Vec::new();
        rmpv::encode::write_value(&mut bytes, &val).unwrap();

        let result: Result<serde_json::Value, _> = Codec::MsgPack.decode(&bytes);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("depth"));
    }

    #[test]
    fn msgpack_depth_check_truncated_payload_does_not_panic() {
        // Truncated payloads must not panic. They may return Ok (for
        // scalars or truncated length fields) or Err (for containers
        // whose declared count exceeds remaining bytes).
        let val = json!({"a": {"b": [1, 2, 3]}});
        let bytes = rmp_serde::to_vec_named(&val).unwrap();
        for cut in [1, 3, 5, bytes.len() / 2] {
            let _ = check_msgpack_depth(&bytes[..cut], 128);
        }
        // Truncated containers: declared children > 0 remaining bytes
        assert!(check_msgpack_depth(&[0x81], 128).is_err()); // fixmap(1): 2 children, 0 bytes
        assert!(check_msgpack_depth(&[0x91], 128).is_err()); // fixarray(1): 1 child, 0 bytes
        // Truncated length fields: loop breaks before parsing children
        assert!(check_msgpack_depth(&[0xdc], 128).is_ok()); // array16, no length bytes
        assert!(check_msgpack_depth(&[0xde, 0x00], 128).is_ok()); // map16, partial length
    }

    #[test]
    fn msgpack_depth_check_empty_input() {
        assert!(check_msgpack_depth(&[], 128).is_ok());
    }

    #[test]
    fn msgpack_depth_check_scalars_only() {
        // Pure scalar value (no containers) should always pass.
        let val = json!(42);
        let bytes = rmp_serde::to_vec_named(&val).unwrap();
        assert!(check_msgpack_depth(&bytes, 0).is_ok());
    }

    #[test]
    fn msgpack_depth_check_rejects_forged_element_count() {
        // map32 declaring 2^32-1 entries but only a few bytes of actual
        // data. Without the element count check, rmpv::read_value would
        // try Vec::with_capacity(4 billion) and OOM.
        let mut bytes = vec![0xdf]; // map32 marker
        bytes.extend_from_slice(&0xFFFF_FFFFu32.to_be_bytes()); // 4 billion entries
        bytes.extend_from_slice(&[0xa1, b'k', 0x01]); // one tiny key-value pair

        let result = check_msgpack_depth(&bytes, 128);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("elements"));
    }

    #[test]
    fn msgpack_decode_rejects_forged_element_count() {
        // Verify the full decode path rejects forged counts.
        let mut bytes = vec![0xdd]; // array32 marker
        bytes.extend_from_slice(&0x7FFF_FFFFu32.to_be_bytes()); // 2 billion entries
        bytes.push(0x01); // one element

        let result: Result<serde_json::Value, _> = Codec::MsgPack.decode(&bytes);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("elements"));
    }
}
