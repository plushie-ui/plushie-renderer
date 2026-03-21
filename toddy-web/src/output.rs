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
///
/// # Safety
///
/// `Send` is implemented because WASM is single-threaded. The
/// `js_sys::Function` is only ever accessed from the main thread.
pub struct WebOutputWriter {
    callback: js_sys::Function,
}

// SAFETY: WASM is single-threaded. JsValue/Function are only accessed
// from the main (and only) thread.
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
        self.callback
            .call1(&JsValue::NULL, &js_str)
            .map_err(|_| io::Error::other("JS callback failed"))?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
