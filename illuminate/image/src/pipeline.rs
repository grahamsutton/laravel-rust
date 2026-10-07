//! The ordered list of transformations (and output options) applied to an image.

use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

use crate::exception::ImageException;
use crate::transformations::Transformation;

/// The formats an image may be encoded as.
///
/// ```
/// use illuminate_image::ImageFormat;
///
/// let format: ImageFormat = "jpg".parse().unwrap();
///
/// assert_eq!(format, ImageFormat::Jpeg);
/// assert_eq!(format.extension(), "jpg");
/// assert_eq!(format.mime_type(), "image/jpeg");
/// assert_eq!("heif".parse::<ImageFormat>().unwrap(), ImageFormat::Heic);
/// assert_eq!(
///     "tiff".parse::<ImageFormat>().unwrap_err().to_string(),
///     "The [tiff] format is not supported.",
/// );
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ImageFormat {
    Webp,
    Jpeg,
    Png,
    Gif,
    Avif,
    Heic,
    Bmp,
}

impl ImageFormat {
    /// Every format an image may be converted to.
    pub const ALL: [ImageFormat; 7] = [
        ImageFormat::Webp,
        ImageFormat::Jpeg,
        ImageFormat::Png,
        ImageFormat::Gif,
        ImageFormat::Avif,
        ImageFormat::Heic,
        ImageFormat::Bmp,
    ];

    /// Parse a format name: `webp`, `jpg`, `jpeg`, `png`, `gif`, `avif`,
    /// `heic`, `heif` (an alias of `heic`) or `bmp`.
    pub fn parse(format: &str) -> Result<Self, ImageException> {
        match format {
            "webp" => Ok(Self::Webp),
            "jpg" | "jpeg" => Ok(Self::Jpeg),
            "png" => Ok(Self::Png),
            "gif" => Ok(Self::Gif),
            "avif" => Ok(Self::Avif),
            "heic" | "heif" => Ok(Self::Heic),
            "bmp" => Ok(Self::Bmp),
            _ => Err(ImageException::new(format!(
                "The [{format}] format is not supported."
            ))),
        }
    }

    /// The format of the given MIME type, if it's one images can be encoded as.
    pub fn from_mime_type(mime_type: &str) -> Option<Self> {
        match mime_type {
            "image/webp" => Some(Self::Webp),
            "image/jpeg" => Some(Self::Jpeg),
            "image/png" => Some(Self::Png),
            "image/gif" | "image/x-gif" => Some(Self::Gif),
            "image/avif" | "image/x-avif" => Some(Self::Avif),
            "image/heic" | "image/x-heic" | "image/heif" => Some(Self::Heic),
            "image/bmp" | "image/x-ms-bmp" => Some(Self::Bmp),
            _ => None,
        }
    }

    /// The format's name, as Laravel spells it.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Webp => "webp",
            Self::Jpeg => "jpg",
            Self::Png => "png",
            Self::Gif => "gif",
            Self::Avif => "avif",
            Self::Heic => "heic",
            Self::Bmp => "bmp",
        }
    }

    /// The file extension for the format.
    pub fn extension(&self) -> &'static str {
        self.as_str()
    }

    /// The MIME type of the format.
    pub fn mime_type(&self) -> &'static str {
        match self {
            Self::Webp => "image/webp",
            Self::Jpeg => "image/jpeg",
            Self::Png => "image/png",
            Self::Gif => "image/gif",
            Self::Avif => "image/avif",
            Self::Heic => "image/heic",
            Self::Bmp => "image/bmp",
        }
    }

    /// Whether the format can store transparency.
    pub fn supports_transparency(&self) -> bool {
        !matches!(self, Self::Jpeg)
    }
}

impl FromStr for ImageFormat {
    type Err = ImageException;

    fn from_str(format: &str) -> Result<Self, Self::Err> {
        Self::parse(format)
    }
}

impl fmt::Display for ImageFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How the processed image should be encoded.
///
/// When no format is set, images keep their original format. When no
/// quality is set, lossy encoders use [`ImageOutputOptions::DEFAULT_QUALITY`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ImageOutputOptions {
    /// The output format (`None` keeps the original format).
    pub format: Option<ImageFormat>,
    /// The output quality, between 1 and 100.
    pub quality: Option<u8>,
}

impl ImageOutputOptions {
    /// The default output quality.
    pub const DEFAULT_QUALITY: u8 = 70;

    /// Determine if any output options have been set.
    pub fn has_changes(&self) -> bool {
        self.format.is_some() || self.quality.is_some()
    }

    /// The quality to encode with: the configured one, or the default.
    pub fn quality_or_default(&self) -> u8 {
        self.quality.unwrap_or(Self::DEFAULT_QUALITY)
    }
}

/// The ordered image transformations and output options that a driver
/// applies when it processes an image.
///
/// ```
/// use illuminate_image::{ImageFormat, ImagePipeline};
/// use illuminate_image::transformations::Cover;
///
/// let mut pipeline = ImagePipeline::new();
/// assert!(!pipeline.has_changes());
///
/// pipeline.add(Cover::new(400, 400));
/// pipeline.output.format = Some(ImageFormat::Webp);
///
/// assert!(pipeline.has_changes());
/// assert_eq!(pipeline.transformations.len(), 1);
/// assert!(pipeline.transformations[0].is::<Cover>());
/// ```
#[derive(Clone, Debug, Default)]
pub struct ImagePipeline {
    /// The ordered image transformations.
    pub transformations: Vec<Arc<dyn Transformation>>,
    /// The output options.
    pub output: ImageOutputOptions,
}

impl ImagePipeline {
    /// Create a new, empty pipeline.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a transformation to the pipeline.
    pub fn add(&mut self, transformation: impl Transformation) -> &mut Self {
        self.transformations.push(Arc::new(transformation));
        self
    }

    /// Add a shared transformation to the pipeline.
    pub fn add_arc(&mut self, transformation: Arc<dyn Transformation>) -> &mut Self {
        self.transformations.push(transformation);
        self
    }

    /// Determine if the pipeline has transformations or output changes.
    pub fn has_changes(&self) -> bool {
        !self.transformations.is_empty() || self.output.has_changes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transformations::{Blur, Grayscale};

    #[test]
    fn formats_parse_like_laravel() {
        for (name, format) in [
            ("webp", ImageFormat::Webp),
            ("jpg", ImageFormat::Jpeg),
            ("jpeg", ImageFormat::Jpeg),
            ("png", ImageFormat::Png),
            ("gif", ImageFormat::Gif),
            ("avif", ImageFormat::Avif),
            ("heic", ImageFormat::Heic),
            ("heif", ImageFormat::Heic),
            ("bmp", ImageFormat::Bmp),
        ] {
            assert_eq!(ImageFormat::parse(name).unwrap(), format);
        }

        assert_eq!(
            ImageFormat::parse("jpge").unwrap_err().to_string(),
            "The [jpge] format is not supported."
        );
        assert!(ImageFormat::parse("JPG").is_err());
    }

    #[test]
    fn formats_know_their_mime_types() {
        for format in ImageFormat::ALL {
            assert_eq!(
                ImageFormat::from_mime_type(format.mime_type()),
                Some(format)
            );
            assert_eq!(format.to_string(), format.extension());
        }
        assert_eq!(ImageFormat::from_mime_type("text/plain"), None);
        assert!(!ImageFormat::Jpeg.supports_transparency());
        assert!(ImageFormat::Png.supports_transparency());
    }

    #[test]
    fn output_options_track_changes() {
        let mut output = ImageOutputOptions::default();
        assert!(!output.has_changes());
        assert_eq!(output.quality_or_default(), 70);

        output.quality = Some(1);
        assert!(output.has_changes());
        assert_eq!(output.quality_or_default(), 1);
    }

    #[test]
    fn pipelines_track_changes() {
        let mut pipeline = ImagePipeline::new();
        assert!(!pipeline.has_changes());

        pipeline.add(Blur::new(0)).add(Grayscale);
        assert!(pipeline.has_changes());
        assert_eq!(pipeline.transformations.len(), 2);

        let mut quality_only = ImagePipeline::new();
        quality_only.output.quality = Some(50);
        assert!(quality_only.has_changes());
    }
}
