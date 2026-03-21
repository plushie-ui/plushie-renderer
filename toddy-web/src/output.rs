//! Web output writer.
//!
//! Wraps a JavaScript callback function. When events are emitted,
//! the encoded bytes are converted to a string and passed to the
//! JS callback.

use std::io::{self, Write};

use wasm_bindgen::prelude::*;

/// Output writer that forwards encoded bytes to a JavaScript callback.
///
/// The callback receives a single string argument containing the
/// JSON-encoded event data.
pub struct WebOutputWriter {
    callback: js_sys::Function,
}

// SAFETY: WebOutputWriter holds a js_sys::Function which contains a
// raw pointer (JsValue). On wasm32-unknown-unknown, there is only one
// thread, so Send is trivially satisfied -- the value is never actually
// transferred between threads. This impl must NOT be used on multi-
// threaded WASM targets (e.g. wasm32-unknown-unknown with shared memory).
#[allow(unsafe_code)]
unsafe impl Send for WebOutputWriter {}

impl WebOutputWriter {
    pub fn new(callback: js_sys::Function) -> Self {
        Self { callback }
    }
}

impl Write for WebOutputWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let json = String::from_utf8_lossy(buf);
        let js_str = JsValue::from_str(&json);
        self.callback.call1(&JsValue::NULL, &js_str).map_err(|e| {
            let msg = e
                .as_string()
                .unwrap_or_else(|| "unknown JS error".to_string());
            io::Error::other(format!("JS callback failed: {msg}"))
        })?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
