//! In-memory image handle storage.
//!
//! The host creates images by sending encoded bytes (PNG, JPEG, etc.)
//! or raw RGBA pixel data via `image_op` messages. Each image is stored
//! as an iced [`image::Handle`] keyed by a host-chosen name. Widget
//! nodes reference images by name through the `source` prop, and the
//! renderer resolves them through [`ImageRegistry::get`].

use std::collections::HashMap;

use iced::widget::image;

/// Maximum number of images the registry will hold.
const MAX_IMAGES: usize = 4096;

/// Maximum aggregate byte usage across all images (1 GiB).
const MAX_TOTAL_BYTES: usize = 1024 * 1024 * 1024;

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
///
/// Enforces per-image size limits and aggregate limits (count and total bytes)
/// to prevent unbounded memory growth from a misbehaving host.
pub struct ImageRegistry {
    handles: HashMap<String, image::Handle>,
    /// Per-image byte size tracking (parallel to `handles`).
    sizes: HashMap<String, usize>,
    /// Running total of all image bytes in the registry.
    total_bytes: usize,
}

impl std::fmt::Debug for ImageRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImageRegistry")
            .field("count", &self.handles.len())
            .field("total_bytes", &self.total_bytes)
            .field("names", &self.handles.keys().collect::<Vec<_>>())
            .finish()
    }
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
            sizes: HashMap::new(),
            total_bytes: 0,
        }
    }

    /// Maximum dimension (width or height) for a single image.
    const MAX_DIMENSION: u32 = 16384;

    /// Maximum pixel data size in bytes (256 MB).
    const MAX_PIXEL_BYTES: usize = 256 * 1024 * 1024;

    /// Check aggregate limits before inserting. Accounts for the case where
    /// an existing image with the same name is being replaced (its bytes
    /// will be freed).
    fn check_aggregate_limits(&self, name: &str, new_bytes: usize) -> Result<(), String> {
        let existing_bytes = self.sizes.get(name).copied().unwrap_or(0);
        let is_new_entry = existing_bytes == 0;

        if is_new_entry && self.handles.len() >= MAX_IMAGES {
            let msg = format!(
                "image registry: count limit reached ({MAX_IMAGES}), \
                 cannot add '{name}'"
            );
            log::error!("{msg}");
            return Err(msg);
        }

        let projected = self.total_bytes - existing_bytes + new_bytes;
        if projected > MAX_TOTAL_BYTES {
            let msg = format!(
                "image registry: total byte limit exceeded \
                 (current={}, adding={new_bytes}, limit={MAX_TOTAL_BYTES}), \
                 cannot add '{name}'",
                self.total_bytes
            );
            log::error!("{msg}");
            return Err(msg);
        }

        Ok(())
    }

    /// Insert a handle and update size tracking. Handles replacement of
    /// existing entries by subtracting the old size first.
    fn insert(&mut self, name: &str, handle: image::Handle, byte_count: usize) {
        if let Some(old_size) = self.sizes.get(name) {
            self.total_bytes -= old_size;
        }
        self.handles.insert(name.to_owned(), handle);
        self.sizes.insert(name.to_owned(), byte_count);
        self.total_bytes += byte_count;
    }

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
        self.check_aggregate_limits(name, data.len())?;
        if sniff_image_format(&data).is_none() && !data.is_empty() {
            log::warn!(
                "image: unrecognized format (first bytes: {:02x?}), passing through [id={}]",
                &data[..data.len().min(4)],
                name
            );
        }
        let byte_count = data.len();
        self.insert(name, image::Handle::from_bytes(data), byte_count);
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

        self.check_aggregate_limits(name, pixels.len())?;
        let byte_count = pixels.len();
        self.insert(
            name,
            image::Handle::from_rgba(width, height, pixels),
            byte_count,
        );
        Ok(())
    }

    /// Remove a named image handle.
    pub fn delete(&mut self, name: &str) {
        self.handles.remove(name);
        if let Some(size) = self.sizes.remove(name) {
            self.total_bytes -= size;
        }
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
        self.sizes.clear();
        self.total_bytes = 0;
    }

    /// Dispatch an image operation by name.
    ///
    /// Supported ops:
    /// - `"create_image"` / `"update_image"` -- create or replace an image
    ///   from raw RGBA `pixels` or encoded `data` (PNG, JPEG, etc.).
    /// - `"delete_image"` -- remove the named image.
    pub fn apply_op(
        &mut self,
        op: &str,
        handle: &str,
        data: Option<Vec<u8>>,
        pixels: Option<Vec<u8>>,
        width: Option<u32>,
        height: Option<u32>,
    ) -> Result<(), String> {
        match op {
            "create_image" | "update_image" => {
                if let Some(pixel_bytes) = pixels {
                    let w = width.unwrap_or(0);
                    let h = height.unwrap_or(0);
                    self.create_from_rgba(handle, w, h, pixel_bytes)
                } else if let Some(image_bytes) = data {
                    self.create_from_bytes(handle, image_bytes)
                } else {
                    Err(format!("image_op {op}: missing data or pixels field"))
                }
            }
            "delete_image" => {
                self.delete(handle);
                Ok(())
            }
            other => Err(format!("unknown image_op: {other}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_registry_is_empty() {
        let reg = ImageRegistry::new();
        assert!(reg.get("nope").is_none());
        assert_eq!(reg.total_bytes, 0);
    }

    #[test]
    fn create_from_bytes_and_get() {
        let mut reg = ImageRegistry::new();
        assert!(
            reg.create_from_bytes("test", vec![0x89, 0x50, 0x4e, 0x47])
                .is_ok()
        );
        assert!(reg.get("test").is_some());
        assert_eq!(reg.total_bytes, 4);
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
        assert_eq!(reg.total_bytes, 4);
    }

    #[test]
    fn delete_removes_handle() {
        let mut reg = ImageRegistry::new();
        let _ = reg.create_from_bytes("gone", vec![1, 2, 3]);
        assert_eq!(reg.total_bytes, 3);
        reg.delete("gone");
        assert!(reg.get("gone").is_none());
        assert_eq!(reg.total_bytes, 0);
    }

    #[test]
    fn delete_nonexistent_is_noop() {
        let mut reg = ImageRegistry::new();
        reg.delete("never_existed");
        assert_eq!(reg.total_bytes, 0);
    }

    #[test]
    fn overwrite_replaces_handle() {
        let mut reg = ImageRegistry::new();
        let _ = reg.create_from_bytes("img", vec![1]);
        assert_eq!(reg.total_bytes, 1);
        let _ = reg.create_from_bytes("img", vec![2, 3]);
        assert!(reg.get("img").is_some());
        // Old size (1) replaced by new size (2)
        assert_eq!(reg.total_bytes, 2);
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

    // -- apply_op -------------------------------------------------------------

    #[test]
    fn apply_op_create_from_pixels() {
        let mut reg = ImageRegistry::new();
        assert!(
            reg.apply_op(
                "create_image",
                "img",
                None,
                Some(vec![0; 4]),
                Some(1),
                Some(1)
            )
            .is_ok()
        );
        assert!(reg.get("img").is_some());
    }

    #[test]
    fn apply_op_create_from_data() {
        let mut reg = ImageRegistry::new();
        assert!(
            reg.apply_op(
                "create_image",
                "img",
                Some(vec![0x89, 0x50, 0x4e, 0x47]),
                None,
                None,
                None
            )
            .is_ok()
        );
        assert!(reg.get("img").is_some());
    }

    #[test]
    fn apply_op_update_replaces() {
        let mut reg = ImageRegistry::new();
        let _ = reg.apply_op("create_image", "img", Some(vec![1]), None, None, None);
        let _ = reg.apply_op("update_image", "img", Some(vec![2]), None, None, None);
        assert!(reg.get("img").is_some());
    }

    #[test]
    fn apply_op_delete() {
        let mut reg = ImageRegistry::new();
        let _ = reg.apply_op("create_image", "img", Some(vec![1]), None, None, None);
        assert!(
            reg.apply_op("delete_image", "img", None, None, None, None)
                .is_ok()
        );
        assert!(reg.get("img").is_none());
    }

    #[test]
    fn apply_op_missing_data_and_pixels() {
        let mut reg = ImageRegistry::new();
        assert!(
            reg.apply_op("create_image", "img", None, None, None, None)
                .is_err()
        );
    }

    #[test]
    fn apply_op_unknown_op() {
        let mut reg = ImageRegistry::new();
        assert!(
            reg.apply_op("rotate_image", "img", None, None, None, None)
                .is_err()
        );
    }

    #[test]
    fn delete_then_recreate_same_handle() {
        let mut reg = ImageRegistry::new();
        // Create from bytes
        assert!(
            reg.create_from_bytes("reused", vec![0x89, 0x50, 0x4e, 0x47])
                .is_ok()
        );
        assert!(reg.get("reused").is_some());

        // Delete
        reg.delete("reused");
        assert!(reg.get("reused").is_none());

        // Recreate with same name, but from RGBA this time
        assert!(
            reg.create_from_rgba("reused", 1, 1, vec![255, 0, 0, 255])
                .is_ok()
        );
        assert!(reg.get("reused").is_some());
    }

    // -- Aggregate limit tests ------------------------------------------------

    #[test]
    fn count_limit_rejects_at_max() {
        let mut reg = ImageRegistry::new();
        // Fill to capacity with tiny images
        for i in 0..MAX_IMAGES {
            let name = format!("img_{i}");
            assert!(
                reg.create_from_bytes(&name, vec![0x89, 0x50, 0x4e, 0x47])
                    .is_ok(),
                "image {i} should succeed"
            );
        }
        assert_eq!(reg.handles.len(), MAX_IMAGES);
        // One more should fail
        let result = reg.create_from_bytes("one_too_many", vec![1]);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("count limit"));
    }

    #[test]
    fn update_existing_does_not_count_as_new() {
        let mut reg = ImageRegistry::new();
        for i in 0..MAX_IMAGES {
            let name = format!("img_{i}");
            let _ = reg.create_from_bytes(&name, vec![1]);
        }
        // Updating an existing image should succeed (not a new entry)
        assert!(reg.create_from_bytes("img_0", vec![2, 3]).is_ok());
    }

    #[test]
    fn total_bytes_tracking_across_operations() {
        let mut reg = ImageRegistry::new();
        let _ = reg.create_from_bytes("a", vec![1, 2, 3]);
        assert_eq!(reg.total_bytes, 3);
        let _ = reg.create_from_rgba("b", 1, 1, vec![0, 0, 0, 0]);
        assert_eq!(reg.total_bytes, 7);
        reg.delete("a");
        assert_eq!(reg.total_bytes, 4);
        reg.clear();
        assert_eq!(reg.total_bytes, 0);
    }
}
