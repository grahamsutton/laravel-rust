//! Detecting an image's MIME type from its contents.
//!
//! Laravel asks `finfo` for the MIME type of the processed bytes. Images
//! announce themselves in their first few bytes, so we read those magic
//! numbers the same way.

/// Detect the MIME type of the given contents from their magic bytes.
///
/// Unknown binary data is `application/octet-stream`, readable text is
/// `text/plain`, and nothing at all is `application/x-empty` — just like
/// `finfo`.
///
/// ```
/// use illuminate_image::mime_type_of;
///
/// assert_eq!(mime_type_of(b"\x89PNG\r\n\x1a\n...."), "image/png");
/// assert_eq!(mime_type_of(b"GIF89a...."), "image/gif");
/// assert_eq!(mime_type_of(b"not-an-image"), "text/plain");
/// ```
pub fn mime_type_of(contents: &[u8]) -> &'static str {
    let starts_with = |signature: &[u8]| contents.starts_with(signature);

    if contents.is_empty() {
        return "application/x-empty";
    }
    if starts_with(&[0xFF, 0xD8, 0xFF]) {
        return "image/jpeg";
    }
    if starts_with(b"\x89PNG\r\n\x1a\n") {
        return "image/png";
    }
    if starts_with(b"GIF87a") || starts_with(b"GIF89a") {
        return "image/gif";
    }
    if contents.len() >= 12 && &contents[..4] == b"RIFF" && &contents[8..12] == b"WEBP" {
        return "image/webp";
    }
    if contents.len() >= 12 && &contents[4..8] == b"ftyp" {
        match &contents[8..12] {
            b"avif" | b"avis" => return "image/avif",
            b"heic" | b"heix" | b"hevc" | b"hevx" | b"heim" | b"heis" | b"hevm" | b"hevs" => {
                return "image/heic";
            }
            b"mif1" | b"msf1" => return "image/heif",
            _ => {}
        }
    }
    if starts_with(b"BM") && contents.len() >= 14 {
        return "image/bmp";
    }
    if starts_with(b"II*\0") || starts_with(b"MM\0*") {
        return "image/tiff";
    }
    if starts_with(b"%PDF-") {
        return "application/pdf";
    }
    if looks_like_svg(contents) {
        return "image/svg+xml";
    }
    if looks_like_text(contents) {
        return "text/plain";
    }

    "application/octet-stream"
}

/// The file extension Laravel uses for the given MIME type (`bin` when unknown).
///
/// ```
/// use illuminate_image::extension_for;
///
/// assert_eq!(extension_for("image/jpeg"), "jpg");
/// assert_eq!(extension_for("image/x-avif"), "avif");
/// assert_eq!(extension_for("application/zip"), "bin");
/// ```
pub fn extension_for(mime_type: &str) -> &'static str {
    match mime_type {
        "image/jpeg" => "jpg",
        "image/png" => "png",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/avif" | "image/x-avif" => "avif",
        "image/heic" | "image/x-heic" | "image/heif" => "heic",
        "image/bmp" => "bmp",
        "image/svg+xml" => "svg",
        "image/tiff" => "tiff",
        _ => "bin",
    }
}

fn looks_like_svg(contents: &[u8]) -> bool {
    let head = &contents[..contents.len().min(1024)];
    let Ok(text) = std::str::from_utf8(head) else {
        return false;
    };
    let text = text.trim_start_matches('\u{feff}').trim_start();
    (text.starts_with("<svg") || text.starts_with("<?xml") || text.starts_with("<!DOCTYPE svg"))
        && text.contains("<svg")
}

fn looks_like_text(contents: &[u8]) -> bool {
    let head = &contents[..contents.len().min(4096)];
    let text = match std::str::from_utf8(head) {
        Ok(text) => text,
        // A multi-byte character may have been cut off at the boundary.
        Err(error) if error.error_len().is_none() && head.len() < contents.len() => {
            match std::str::from_utf8(&head[..error.valid_up_to()]) {
                Ok(text) => text,
                Err(_) => return false,
            }
        }
        Err(_) => return false,
    };

    text.chars()
        .all(|c| !c.is_control() || matches!(c, '\n' | '\r' | '\t' | '\u{c}'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_detects_image_formats() {
        assert_eq!(mime_type_of(&[0xFF, 0xD8, 0xFF, 0xE0, 0, 0]), "image/jpeg");
        assert_eq!(mime_type_of(b"RIFF\0\0\0\0WEBPVP8L"), "image/webp");
        assert_eq!(mime_type_of(b"\0\0\0\x1cftypavif\0\0\0\0"), "image/avif");
        assert_eq!(mime_type_of(b"\0\0\0\x18ftypheic\0\0\0\0"), "image/heic");
        assert_eq!(mime_type_of(b"\0\0\0\x18ftypmif1\0\0\0\0"), "image/heif");
        assert_eq!(mime_type_of(b"BM\0\0\0\0\0\0\0\0\0\0\0\0\0\0"), "image/bmp");
        assert_eq!(mime_type_of(b"II*\0\x08\0\0\0"), "image/tiff");
        assert_eq!(
            mime_type_of(b"<?xml version=\"1.0\"?><svg></svg>"),
            "image/svg+xml"
        );
        assert_eq!(
            mime_type_of(b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>"),
            "image/svg+xml"
        );
    }

    #[test]
    fn it_detects_non_images() {
        assert_eq!(mime_type_of(b""), "application/x-empty");
        assert_eq!(mime_type_of(b"not-an-image"), "text/plain");
        assert_eq!(
            mime_type_of(&[0, 1, 2, 3, 4, 5]),
            "application/octet-stream"
        );
        assert_eq!(mime_type_of(b"%PDF-1.7"), "application/pdf");
    }

    #[test]
    fn it_maps_mime_types_to_extensions() {
        assert_eq!(extension_for("image/png"), "png");
        assert_eq!(extension_for("image/gif"), "gif");
        assert_eq!(extension_for("image/webp"), "webp");
        assert_eq!(extension_for("image/heif"), "heic");
        assert_eq!(extension_for("image/bmp"), "bmp");
        assert_eq!(extension_for("image/svg+xml"), "svg");
        assert_eq!(extension_for("image/tiff"), "tiff");
        assert_eq!(extension_for("text/plain"), "bin");
    }
}
