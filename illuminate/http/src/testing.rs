//! Fake uploaded files for your tests: `UploadedFile::fake()`.
//!
//! ```
//! use illuminate_http::UploadedFile;
//!
//! let avatar = UploadedFile::fake().image("avatar.jpg", 200, 200);
//! let document = UploadedFile::fake().create("document.pdf", 512);
//! let notes = UploadedFile::fake().create_with_content("notes.txt", "Remember the milk");
//!
//! assert_eq!(avatar.mime_type(), "image/jpeg");
//! assert_eq!(document.size(), 512 * 1024);
//! assert_eq!(notes.get(), "Remember the milk");
//! ```

use std::io::Cursor;
use std::path::Path;

use bytes::Bytes;

use crate::uploaded_file::UploadedFile;

/// Creates fake uploaded files — returned by [`UploadedFile::fake`].
#[derive(Clone, Copy, Debug, Default)]
pub struct FileFactory;

impl FileFactory {
    /// Create a fake file reporting the given size in kilobytes. Its MIME
    /// type is guessed from the name; use
    /// [`with_mime_type`](UploadedFile::with_mime_type) to choose another.
    ///
    /// ```
    /// use illuminate_http::UploadedFile;
    ///
    /// let file = UploadedFile::fake()
    ///     .create("document.pdf", 100)
    ///     .with_mime_type("application/pdf");
    ///
    /// assert_eq!(file.size(), 102_400);
    /// assert_eq!(file.client_original_name(), "document.pdf");
    /// ```
    pub fn create(&self, name: &str, kilobytes: usize) -> UploadedFile {
        UploadedFile::new(name, MimeType::from(name), Bytes::new()).with_size(kilobytes)
    }

    /// Create a fake file reporting the given size in kilobytes, with the
    /// given MIME type.
    pub fn create_with_mime_type(
        &self,
        name: &str,
        kilobytes: usize,
        mime_type: &str,
    ) -> UploadedFile {
        self.create(name, kilobytes).with_mime_type(mime_type)
    }

    /// Create a fake file with the given contents.
    pub fn create_with_content(&self, name: &str, content: impl Into<Bytes>) -> UploadedFile {
        UploadedFile::new(name, MimeType::from(name), content.into())
    }

    /// Create a real (all black) image of the given dimensions. The format
    /// follows the file's extension — `png`, `gif`, `webp`, `bmp` or
    /// `wbmp` — and is JPEG for anything else.
    ///
    /// ```
    /// use illuminate_http::UploadedFile;
    ///
    /// let png = UploadedFile::fake().image("logo.png", 64, 32);
    /// assert!(png.bytes().starts_with(b"\x89PNG"));
    /// assert_eq!(png.dimensions(), Some((64, 32)));
    ///
    /// let jpeg = UploadedFile::fake().image("avatar.jpg", 10, 10);
    /// assert!(jpeg.bytes().starts_with(&[0xFF, 0xD8, 0xFF]));
    /// ```
    pub fn image(&self, name: &str, width: u32, height: u32) -> UploadedFile {
        let extension = Path::new(name)
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        UploadedFile::new(
            name,
            MimeType::from(name),
            generate_image(width, height, &extension),
        )
    }
}

/// Generate a black image of the given size, encoded for the extension.
fn generate_image(width: u32, height: u32, extension: &str) -> Bytes {
    let (width, height) = (width.max(1), height.max(1));
    let format = match extension {
        "png" => image::ImageFormat::Png,
        "gif" => image::ImageFormat::Gif,
        "webp" => image::ImageFormat::WebP,
        "bmp" => image::ImageFormat::Bmp,
        "wbmp" => return Bytes::from(wbmp(width, height)),
        _ => image::ImageFormat::Jpeg,
    };
    let canvas = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        width,
        height,
        image::Rgba([0, 0, 0, 255]),
    ));
    let canvas = match format {
        image::ImageFormat::Jpeg | image::ImageFormat::Bmp => {
            image::DynamicImage::ImageRgb8(canvas.to_rgb8())
        }
        _ => canvas,
    };
    let mut buffer = Cursor::new(Vec::new());
    canvas
        .write_to(&mut buffer, format)
        .expect("encoding an in-memory image cannot fail");
    Bytes::from(buffer.into_inner())
}

/// A wireless bitmap (WBMP type 0): a tiny header and one bit per pixel.
fn wbmp(width: u32, height: u32) -> Vec<u8> {
    fn multibyte(mut value: u32) -> Vec<u8> {
        let mut bytes = vec![(value & 0x7F) as u8];
        value >>= 7;
        while value > 0 {
            bytes.push(((value & 0x7F) as u8) | 0x80);
            value >>= 7;
        }
        bytes.reverse();
        bytes
    }
    let mut bytes = vec![0, 0];
    bytes.extend(multibyte(width));
    bytes.extend(multibyte(height));
    let row = width.div_ceil(8) as usize;
    bytes.extend(std::iter::repeat_n(0u8, row * height as usize));
    bytes
}

/// Guess MIME types from file names and extensions, and extensions from
/// MIME types.
///
/// ```
/// use illuminate_http::testing::MimeType;
///
/// assert_eq!(MimeType::from("avatar.jpg"), "image/jpeg");
/// assert_eq!(MimeType::get("pdf"), "application/pdf");
/// assert_eq!(MimeType::get("unknown-extension"), "application/octet-stream");
/// assert_eq!(MimeType::search("image/jpeg").as_deref(), Some("jpg"));
/// ```
pub struct MimeType;

impl MimeType {
    /// The MIME type for a file name, from its extension.
    pub fn from(filename: &str) -> String {
        let extension = Path::new(filename)
            .extension()
            .map(|e| e.to_string_lossy().into_owned())
            .unwrap_or_default();
        Self::get(&extension)
    }

    /// The MIME type for an extension (`application/octet-stream` when unknown).
    pub fn get(extension: &str) -> String {
        let extension = extension.to_ascii_lowercase();
        match extension.as_str() {
            "md" | "markdown" => return "text/markdown".into(),
            "webp" => return "image/webp".into(),
            "avif" => return "image/avif".into(),
            "heic" => return "image/heic".into(),
            "wbmp" => return "image/vnd.wap.wbmp".into(),
            _ => {}
        }
        mime_guess::from_ext(&extension)
            .first_raw()
            .unwrap_or("application/octet-stream")
            .to_string()
    }

    /// The most common extension for a MIME type.
    pub fn search(mime_type: &str) -> Option<String> {
        let mime_type = mime_type
            .split(';')
            .next()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        let preferred = match mime_type.as_str() {
            "image/jpeg" | "image/pjpeg" => Some("jpg"),
            "text/plain" => Some("txt"),
            "text/html" => Some("html"),
            "text/javascript" | "application/javascript" => Some("js"),
            "text/markdown" => Some("md"),
            "image/svg+xml" => Some("svg"),
            "image/tiff" => Some("tif"),
            "image/bmp" | "image/x-ms-bmp" => Some("bmp"),
            "image/webp" => Some("webp"),
            "image/avif" => Some("avif"),
            "image/heic" => Some("heic"),
            "image/vnd.wap.wbmp" => Some("wbmp"),
            "image/x-icon" | "image/vnd.microsoft.icon" => Some("ico"),
            "audio/mpeg" => Some("mp3"),
            "audio/ogg" => Some("oga"),
            "audio/wav" | "audio/x-wav" => Some("wav"),
            "video/mp4" => Some("mp4"),
            "video/mpeg" => Some("mpeg"),
            "video/quicktime" => Some("mov"),
            "application/octet-stream" => Some("bin"),
            "application/json" => Some("json"),
            "application/xml" | "text/xml" => Some("xml"),
            "application/msword" => Some("doc"),
            "application/vnd.ms-excel" => Some("xls"),
            "application/gzip" => Some("gz"),
            "text/calendar" => Some("ics"),
            _ => None,
        };
        if let Some(extension) = preferred {
            return Some(extension.to_string());
        }
        mime_guess::get_mime_extensions_str(&mime_type)
            .and_then(|extensions| extensions.first())
            .map(|extension| extension.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(file: &UploadedFile) -> (image::ImageFormat, u32, u32) {
        let reader = image::ImageReader::new(Cursor::new(file.bytes().to_vec()))
            .with_guessed_format()
            .unwrap();
        let format = reader.format().unwrap();
        let decoded = reader.decode().unwrap();
        (format, decoded.width(), decoded.height())
    }

    #[test]
    fn it_generates_real_images_for_each_extension() {
        let factory = FileFactory;
        let cases = [
            ("a.jpg", image::ImageFormat::Jpeg),
            ("a.jpeg", image::ImageFormat::Jpeg),
            ("a.png", image::ImageFormat::Png),
            ("a.gif", image::ImageFormat::Gif),
            ("a.webp", image::ImageFormat::WebP),
            ("a.bmp", image::ImageFormat::Bmp),
            ("a.PNG", image::ImageFormat::Png),
            ("a.txt", image::ImageFormat::Jpeg),
        ];
        for (name, format) in cases {
            let file = factory.image(name, 20, 12);
            assert_eq!(decode(&file), (format, 20, 12), "{name}");
            assert_eq!(file.dimensions(), Some((20, 12)), "{name}");
        }
        assert_eq!(factory.image("a.png", 0, 0).dimensions(), Some((1, 1)));
    }

    #[test]
    fn it_generates_wireless_bitmaps() {
        let file = FileFactory.image("a.wbmp", 200, 3);
        // Type 0, fixed header 0, width 200 (two bytes), height 3, 25 bytes per row.
        assert_eq!(&file.bytes()[..5], &[0, 0, 0x81, 0x48, 3]);
        assert_eq!(file.size(), 5 + 25 * 3);
        assert_eq!(file.mime_type(), "image/vnd.wap.wbmp");
    }

    #[test]
    fn fake_images_report_mime_types_from_their_names() {
        let file = FileFactory.image("avatar.jpg", 10, 10);
        assert_eq!(file.mime_type(), "image/jpeg");
        assert_eq!(file.extension(), "jpg");
        let file = FileFactory.image("avatar.png", 10, 10).with_size(100);
        assert_eq!(file.mime_type(), "image/png");
        assert_eq!(file.size(), 102_400);
    }

    #[test]
    fn mime_types_are_guessed() {
        assert_eq!(MimeType::from("song.mp3"), "audio/mpeg");
        assert_eq!(MimeType::from("README.md"), "text/markdown");
        assert_eq!(MimeType::from("no-extension"), "application/octet-stream");
        assert_eq!(MimeType::search("application/pdf").as_deref(), Some("pdf"));
        assert_eq!(
            MimeType::search("text/plain; charset=UTF-8").as_deref(),
            Some("txt")
        );
        assert_eq!(MimeType::search("application/x-nothing"), None);
    }
}
