//! Inspecting uploaded files: guessing MIME types from their contents and
//! reading image dimensions from PNG, JPEG, GIF, BMP and WebP headers.

use illuminate_http::UploadedFile;

/// Guess a file's MIME type from its leading bytes ("magic numbers").
pub(crate) fn sniff_mime(bytes: &[u8]) -> Option<&'static str> {
    let starts = |prefix: &[u8]| bytes.starts_with(prefix);
    if starts(b"\x89PNG\r\n\x1a\n") {
        return Some("image/png");
    }
    if starts(&[0xFF, 0xD8, 0xFF]) {
        return Some("image/jpeg");
    }
    if starts(b"GIF87a") || starts(b"GIF89a") {
        return Some("image/gif");
    }
    if starts(b"BM") && bytes.len() > 26 {
        return Some("image/bmp");
    }
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" {
        match &bytes[8..12] {
            b"WEBP" => return Some("image/webp"),
            b"WAVE" => return Some("audio/wav"),
            b"AVI " => return Some("video/x-msvideo"),
            _ => {}
        }
    }
    if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" {
        return Some(match &bytes[8..12] {
            b"avif" | b"avis" => "image/avif",
            b"heic" | b"heix" | b"hevc" | b"hevx" => "image/heic",
            b"mif1" | b"msf1" => "image/heif",
            b"qt  " => "video/quicktime",
            b"M4A " => "audio/mp4",
            _ => "video/mp4",
        });
    }
    if starts(b"%PDF-") {
        return Some("application/pdf");
    }
    if starts(b"PK\x03\x04") {
        return Some("application/zip");
    }
    if starts(&[0x1F, 0x8B]) {
        return Some("application/gzip");
    }
    if starts(b"II*\0") || starts(b"MM\0*") {
        return Some("image/tiff");
    }
    if starts(&[0, 0, 1, 0]) && bytes.len() > 6 {
        return Some("image/vnd.microsoft.icon");
    }
    if starts(b"ID3") || starts(&[0xFF, 0xFB]) {
        return Some("audio/mpeg");
    }
    if starts(b"OggS") {
        return Some("audio/ogg");
    }
    if starts(b"fLaC") {
        return Some("audio/flac");
    }
    if starts(&[0x1A, 0x45, 0xDF, 0xA3]) {
        return Some("video/webm");
    }
    if is_svg(bytes) {
        return Some("image/svg+xml");
    }
    None
}

fn is_svg(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(1024)];
    let Ok(text) = std::str::from_utf8(head) else {
        return false;
    };
    let text = text.trim_start_matches('\u{feff}').trim_start();
    (text.starts_with("<?xml") || text.starts_with("<svg") || text.starts_with("<!DOCTYPE svg"))
        && text.contains("<svg")
}

/// The MIME type of the file: sniffed from its contents, falling back to
/// the type reported by the client (or guessed from its name).
pub(crate) fn mime_type(file: &UploadedFile) -> String {
    if let Some(sniffed) = sniff_mime(file.bytes()) {
        return sniffed.to_string();
    }
    let reported = file.mime_type();
    if !reported.is_empty() && reported != "application/octet-stream" {
        return reported.to_ascii_lowercase();
    }
    mime_guess::from_path(file.client_original_name())
        .first_raw()
        .unwrap_or("application/octet-stream")
        .to_string()
}

/// The extensions a MIME type is commonly known by (`image/jpeg` gives
/// `jpg`, `jpeg`, `jpe`, ...).
pub(crate) fn extensions_for_mime(mime: &str) -> Vec<String> {
    let mut extensions: Vec<String> = mime_guess::get_mime_extensions_str(mime)
        .map(|exts| exts.iter().map(|e| e.to_string()).collect())
        .unwrap_or_default();
    let extra: &[&str] = match mime {
        "image/jpeg" => &["jpg", "jpeg"],
        "image/heic" | "image/heif" => &["heic", "heif"],
        "image/avif" => &["avif"],
        "image/webp" => &["webp"],
        "image/svg+xml" => &["svg"],
        "image/bmp" => &["bmp"],
        "audio/wav" => &["wav"],
        "application/gzip" => &["gz"],
        "text/plain" => &["txt"],
        "text/csv" => &["csv"],
        "image/vnd.microsoft.icon" => &["ico"],
        _ => &[],
    };
    for ext in extra {
        if !extensions.iter().any(|e| e == ext) {
            extensions.push(ext.to_string());
        }
    }
    extensions
}

/// Read the width and height of an image from its header.
pub(crate) fn dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    let be16 = |i: usize| {
        bytes
            .get(i..i + 2)
            .map(|b| u16::from_be_bytes([b[0], b[1]]) as u32)
    };
    let le16 = |i: usize| {
        bytes
            .get(i..i + 2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]) as u32)
    };
    let be32 = |i: usize| {
        bytes
            .get(i..i + 4)
            .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    };
    let le32 = |i: usize| {
        bytes
            .get(i..i + 4)
            .map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    let le24 = |i: usize| {
        bytes
            .get(i..i + 3)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], 0]))
    };

    match sniff_mime(bytes)? {
        "image/png" => Some((be32(16)?, be32(20)?)),
        "image/gif" => Some((le16(6)?, le16(8)?)),
        "image/bmp" => Some((le32(18)?.unsigned_abs(), le32(22)?.unsigned_abs())),
        "image/webp" => match bytes.get(12..16)? {
            b"VP8 " => Some((le16(26)? & 0x3FFF, le16(28)? & 0x3FFF)),
            b"VP8L" => {
                let b = bytes.get(21..25)?;
                let width = 1 + (((b[1] as u32 & 0x3F) << 8) | b[0] as u32);
                let height = 1
                    + (((b[3] as u32 & 0xF) << 10)
                        | ((b[2] as u32) << 2)
                        | ((b[1] as u32 & 0xC0) >> 6));
                Some((width, height))
            }
            b"VP8X" => Some((1 + le24(24)?, 1 + le24(27)?)),
            _ => None,
        },
        "image/jpeg" => {
            let mut i = 2;
            while i + 9 < bytes.len() {
                if bytes[i] != 0xFF {
                    i += 1;
                    continue;
                }
                let marker = bytes[i + 1];
                if marker == 0xFF {
                    i += 1;
                    continue;
                }
                if matches!(marker, 0xD8 | 0x01 | 0xD0..=0xD7) {
                    i += 2;
                    continue;
                }
                let length = be16(i + 2)? as usize;
                if matches!(marker, 0xC0..=0xCF) && !matches!(marker, 0xC4 | 0xC8 | 0xCC) {
                    let height = be16(i + 5)?;
                    let width = be16(i + 7)?;
                    return Some((width, height));
                }
                i += 2 + length;
            }
            None
        }
        _ => None,
    }
}

#[cfg(test)]
pub(crate) mod fixtures {
    /// A minimal PNG header with the given dimensions.
    pub(crate) fn png(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        bytes.extend_from_slice(&width.to_be_bytes());
        bytes.extend_from_slice(&height.to_be_bytes());
        bytes.extend_from_slice(&[8, 6, 0, 0, 0]);
        bytes
    }

    /// A minimal GIF header with the given dimensions.
    pub(crate) fn gif(width: u16, height: u16) -> Vec<u8> {
        let mut bytes = b"GIF89a".to_vec();
        bytes.extend_from_slice(&width.to_le_bytes());
        bytes.extend_from_slice(&height.to_le_bytes());
        bytes.extend_from_slice(&[0, 0, 0]);
        bytes
    }

    /// A minimal baseline JPEG with the given dimensions.
    pub(crate) fn jpeg(width: u16, height: u16) -> Vec<u8> {
        let mut bytes = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
        bytes.extend_from_slice(b"JFIF\0\x01\x01\0\0\x01\0\x01\0\0");
        bytes.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x11, 0x08]);
        bytes.extend_from_slice(&height.to_be_bytes());
        bytes.extend_from_slice(&width.to_be_bytes());
        bytes.extend_from_slice(&[
            0x03, 0x01, 0x22, 0x00, 0x02, 0x11, 0x01, 0x03, 0x11, 0x01, 0xFF, 0xD9,
        ]);
        bytes
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::*;
    use super::*;

    #[test]
    fn it_reads_image_dimensions() {
        assert_eq!(dimensions(&png(640, 480)), Some((640, 480)));
        assert_eq!(dimensions(&gif(32, 16)), Some((32, 16)));
        assert_eq!(dimensions(&jpeg(1024, 768)), Some((1024, 768)));
        assert_eq!(dimensions(b"not an image"), None);
    }

    #[test]
    fn it_sniffs_mime_types() {
        assert_eq!(sniff_mime(&png(1, 1)), Some("image/png"));
        assert_eq!(sniff_mime(b"%PDF-1.7"), Some("application/pdf"));
        assert_eq!(
            sniff_mime(b"<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>"),
            Some("image/svg+xml")
        );
        assert_eq!(sniff_mime(b"hello"), None);
        assert!(extensions_for_mime("image/jpeg").contains(&"jpg".to_string()));
    }
}
