//! Clipboard images live on the terminal's host, just like HERDR client images.
use base64::Engine;
use std::io::Write;

const MAX_IMAGE_BYTES: usize = 8 * 1024 * 1024;

pub fn stage(png_base64: &str) -> Result<String, String> {
    if png_base64.len() > MAX_IMAGE_BYTES.div_ceil(3) * 4 {
        return Err("clipboard-image-too-large".into());
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(png_base64)
        .map_err(|_| "clipboard-image-invalid")?;
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err("clipboard-image-too-large".into());
    }
    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err("clipboard-image-invalid".into());
    }
    let mut file = tempfile::Builder::new()
        .prefix("yuzora-clipboard-")
        .suffix(".png")
        .tempfile()
        .map_err(|e| e.to_string())?;
    file.write_all(&bytes).map_err(|e| e.to_string())?;
    // The agent may read the image after Yuzora disconnects. Keep the temp
    // file, as HERDR/pi do, rather than tying its lifetime to a connector.
    let (_, path) = file.keep().map_err(|e| e.to_string())?;
    path.into_os_string()
        .into_string()
        .map_err(|_| "clipboard-image-path-not-utf8".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stages_png_bytes_once_without_interpreting_them_as_terminal_input() {
        let bytes = b"\x89PNG\r\n\x1a\nfixture";
        let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
        let first = stage(&encoded).unwrap();
        let second = stage(&encoded).unwrap();
        assert_ne!(first, second);
        assert_eq!(std::fs::read(&first).unwrap(), bytes);
        assert!(first.ends_with(".png"));
        std::fs::remove_file(first).unwrap();
        std::fs::remove_file(second).unwrap();
    }

    #[test]
    fn refuses_non_images_and_oversized_clipboard_payloads() {
        assert!(stage("not base64").is_err());
        assert!(stage("ZWNobyBoZWxsbw==").is_err());
        assert_eq!(
            stage(&"A".repeat(MAX_IMAGE_BYTES.div_ceil(3) * 4 + 1)).unwrap_err(),
            "clipboard-image-too-large"
        );
    }
}
