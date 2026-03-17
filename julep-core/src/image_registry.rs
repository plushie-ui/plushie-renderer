//! In-memory image handle storage.
//!
//! The host creates images by sending encoded bytes (PNG, JPEG, etc.)
//! or raw RGBA pixel data via `image_op` messages. Each image is stored
//! as an iced [`image::Handle`] keyed by a host-chosen name. Widget
//! nodes reference images by name through the `source` prop, and the
//! renderer resolves them through [`ImageRegistry::get`].

use std::collections::HashMap;

use iced::widget::image;

/// Sniff the image format from the first few bytes (magic bytes).
/// Returns `None` if the format is not recognized.
fn sniff_image_format(data: &[u8]) -> Option<&'static str> {
    if data.len() < 4 {
        return None;
    }
    if data.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
        return Some("PNG");
    }
    if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some("JPEG");
    }
    if data.starts_with(b"GIF8") {
        return Some("GIF");
    }
    if data.starts_with(b"RIFF") && data.len() >= 12 && &data[8..12] == b"WEBP" {
        return Some("WebP");
    }
    if data.starts_with(b"BM") {
        return Some("BMP");
    }
    None
}

/// In-memory registry for image handles. Allows the host to send raw pixel
/// or encoded image data and reference them by name in the UI tree.
pub struct ImageRegistry {
    handles: HashMap<String, image::Handle>,
}

impl Default for ImageRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ImageRegistry {
    pub fn new() -> Self {
        Self {
            handles: HashMap::new(),
        }
    }

    /// Maximum dimension (width or height) for a single image.
    const MAX_DIMENSION: u32 = 16384;

    /// Maximum pixel data size in bytes (256 MB).
    const MAX_PIXEL_BYTES: usize = 256 * 1024 * 1024;

    /// Store an image from encoded bytes (PNG, JPEG, etc.).
    pub fn create_from_bytes(&mut self, name: &str, data: Vec<u8>) -> Result<(), String> {
        if data.len() > Self::MAX_PIXEL_BYTES {
            let msg = format!(
                "encoded data for '{}' exceeds 256 MB limit ({} bytes)",
                name,
                data.len()
            );
            log::error!("image registry: {msg}");
            return Err(msg);
        }
        if sniff_image_format(&data).is_none() && !data.is_empty() {
            log::warn!(
                "image: unrecognized format (first bytes: {:02x?}), passing through [id={}]",
                &data[..data.len().min(4)],
                name
            );
        }
        self.handles
            .insert(name.to_owned(), image::Handle::from_bytes(data));
        Ok(())
    }

    /// Store an image from raw RGBA pixel data.
    pub fn create_from_rgba(
        &mut self,
        name: &str,
        width: u32,
        height: u32,
        pixels: Vec<u8>,
    ) -> Result<(), String> {
        if width > Self::MAX_DIMENSION || height > Self::MAX_DIMENSION {
            let msg = format!(
                "dimensions {}x{} for '{}' exceed max {}",
                width,
                height,
                name,
                Self::MAX_DIMENSION
            );
            log::error!("image registry: {msg}");
            return Err(msg);
        }

        let expected = (width as usize)
            .checked_mul(height as usize)
            .and_then(|n| n.checked_mul(4))
            .ok_or_else(|| format!("dimensions {}x{} overflow for '{name}'", width, height))?;
        if pixels.len() != expected {
            let msg = format!(
                "RGBA data size mismatch for '{}': expected {} bytes ({}x{}x4), got {}",
                name,
                expected,
                width,
                height,
                pixels.len()
            );
            log::error!("image registry: {msg}");
            return Err(msg);
        }

        if pixels.len() > Self::MAX_PIXEL_BYTES {
            let msg = format!(
                "pixel data for '{}' exceeds 256 MB limit ({} bytes)",
                name,
                pixels.len()
            );
            log::error!("image registry: {msg}");
            return Err(msg);
        }

        self.handles.insert(
            name.to_owned(),
            image::Handle::from_rgba(width, height, pixels),
        );
        Ok(())
    }

    /// Remove a named image handle.
    pub fn delete(&mut self, name: &str) {
        self.handles.remove(name);
    }

    /// Look up a named image handle.
    pub fn get(&self, name: &str) -> Option<&image::Handle> {
        self.handles.get(name)
    }

    /// Return the names of all registered image handles.
    pub fn handle_names(&self) -> Vec<String> {
        self.handles.keys().cloned().collect()
    }

    /// Remove all registered image handles.
    pub fn clear(&mut self) {
        self.handles.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_registry_is_empty() {
        let reg = ImageRegistry::new();
        assert!(reg.get("nope").is_none());
    }

    #[test]
    fn create_from_bytes_and_get() {
        let mut reg = ImageRegistry::new();
        assert!(
            reg.create_from_bytes("test", vec![0x89, 0x50, 0x4e, 0x47])
                .is_ok()
        );
        assert!(reg.get("test").is_some());
    }

    #[test]
    fn create_from_rgba_and_get() {
        let mut reg = ImageRegistry::new();
        // 1x1 RGBA pixel
        assert!(
            reg.create_from_rgba("pixel", 1, 1, vec![255, 0, 0, 255])
                .is_ok()
        );
        assert!(reg.get("pixel").is_some());
    }

    #[test]
    fn delete_removes_handle() {
        let mut reg = ImageRegistry::new();
        let _ = reg.create_from_bytes("gone", vec![1, 2, 3]);
        reg.delete("gone");
        assert!(reg.get("gone").is_none());
    }

    #[test]
    fn delete_nonexistent_is_noop() {
        let mut reg = ImageRegistry::new();
        reg.delete("never_existed");
        // no panic
    }

    #[test]
    fn overwrite_replaces_handle() {
        let mut reg = ImageRegistry::new();
        let _ = reg.create_from_bytes("img", vec![1]);
        let _ = reg.create_from_bytes("img", vec![2, 3]);
        assert!(reg.get("img").is_some());
    }

    #[test]
    fn rgba_size_mismatch_rejected() {
        let mut reg = ImageRegistry::new();
        // 2x2 RGBA should be 16 bytes, providing only 4
        let result = reg.create_from_rgba("bad", 2, 2, vec![255, 0, 0, 255]);
        assert!(result.is_err());
        assert!(reg.get("bad").is_none());
    }

    #[test]
    fn rgba_dimension_too_large_rejected() {
        let mut reg = ImageRegistry::new();
        let result = reg.create_from_rgba("huge", 16385, 1, vec![0; 16385 * 4]);
        assert!(result.is_err());
        assert!(reg.get("huge").is_none());
    }

    #[test]
    fn rgba_valid_dimensions_accepted() {
        let mut reg = ImageRegistry::new();
        // 2x2 RGBA = 16 bytes
        assert!(reg.create_from_rgba("ok", 2, 2, vec![0; 16]).is_ok());
        assert!(reg.get("ok").is_some());
    }

    #[test]
    fn sniff_png() {
        assert_eq!(
            sniff_image_format(&[0x89, 0x50, 0x4E, 0x47, 0x0D]),
            Some("PNG")
        );
    }

    #[test]
    fn sniff_jpeg() {
        assert_eq!(sniff_image_format(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("JPEG"));
    }

    #[test]
    fn sniff_gif() {
        assert_eq!(sniff_image_format(b"GIF89a"), Some("GIF"));
    }

    #[test]
    fn sniff_webp() {
        let mut data = vec![0u8; 12];
        data[..4].copy_from_slice(b"RIFF");
        data[8..12].copy_from_slice(b"WEBP");
        assert_eq!(sniff_image_format(&data), Some("WebP"));
    }

    #[test]
    fn sniff_bmp() {
        assert_eq!(sniff_image_format(b"BM\x00\x00"), Some("BMP"));
    }

    #[test]
    fn sniff_unknown() {
        assert_eq!(sniff_image_format(&[0x00, 0x01, 0x02, 0x03]), None);
    }

    #[test]
    fn sniff_too_short() {
        assert_eq!(sniff_image_format(&[0x89, 0x50]), None);
    }
}
