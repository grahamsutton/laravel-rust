//! The built-in, pure Rust image driver, powered by the `image` crate.

use std::borrow::Cow;
use std::io::Cursor;

use ::image::codecs::bmp::BmpEncoder;
use ::image::codecs::gif::GifEncoder;
use ::image::codecs::jpeg::JpegEncoder;
use ::image::codecs::png::PngEncoder;
use ::image::codecs::webp::WebPEncoder;
use ::image::imageops::{self, FilterType};
use ::image::metadata::Orientation;
use ::image::{
    DynamicImage, GenericImageView, ImageDecoder, ImageFormat as CodecFormat, ImageReader, Rgba,
    RgbaImage,
};

use illuminate_support::Result;

use crate::color::{parse_color, to_hex};
use crate::driver::ImageDriver;
use crate::exception::ImageException;
use crate::mime::mime_type_of;
use crate::pipeline::{ImageFormat, ImagePipeline};
use crate::transformations::{
    AnyTransformationHandler, Blur, Contain, Cover, Crop, FlipHorizontally, FlipVertically,
    Grayscale, Orient, Resize, Rotate, Scale, Sharpen, Transformation, TransformationHandlers,
};

/// The resampling filter used when resizing (bicubic, like GD).
const FILTER: FilterType = FilterType::CatmullRom;

/// The background used when none is given (Intervention's default).
const WHITE: Rgba<u8> = Rgba([255, 255, 255, 255]);

/// The built-in image driver: pure Rust, no system libraries required.
///
/// It's registered as the `image` driver. So that configuration written
/// for Laravel keeps working, `gd` and `imagick` are aliases of it.
///
/// - Reads JPEG, PNG, GIF (first frame), WebP and BMP images.
/// - Writes JPEG, PNG, GIF, WebP and BMP images. WebP output is
///   **lossless** (the encoder has no lossy mode), so quality only affects
///   JPEG output. AVIF and HEIC can't be encoded by this driver: converting
///   to them fails when the image is processed.
/// - Images are re-encoded without their metadata, so call
///   [`orient`](crate::Image::orient) before transforming photos that rely
///   on EXIF orientation.
///
/// Custom transformations receive and return a [`DynamicImage`]:
///
/// ```
/// use illuminate_image::{ImageDriver, ImagePipeline, NativeDriver, Transformation, AnyTransformationHandler};
/// use illuminate_image::image::DynamicImage;
///
/// #[derive(Debug)]
/// struct Invert;
/// impl Transformation for Invert {}
///
/// let driver = NativeDriver::new();
/// driver.transform_using(AnyTransformationHandler::new(|mut image: DynamicImage, _: &Invert| {
///     image.invert();
///     Ok(image)
/// })).unwrap();
///
/// let mut png = Vec::new();
/// DynamicImage::new_rgb8(4, 2).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
///
/// let mut pipeline = ImagePipeline::new();
/// pipeline.add(Invert);
/// let inverted = driver.process(&png, &pipeline).unwrap();
///
/// assert_eq!(driver.dimensions(&inverted).unwrap(), (4, 2));
/// assert_eq!(driver.dominant_color(&inverted).unwrap(), "#ffffff");
/// ```
#[derive(Debug, Default)]
pub struct NativeDriver {
    handlers: TransformationHandlers<DynamicImage>,
}

/// A decoded image, with what we learned while decoding it.
struct Decoded {
    image: DynamicImage,
    format: ImageFormat,
    orientation: Orientation,
}

impl NativeDriver {
    /// The name the driver is registered under.
    pub const NAME: &'static str = "image";

    /// The MIME types the driver can read.
    pub const SUPPORTED_MIME_TYPES: [&'static str; 5] = [
        "image/jpeg",
        "image/png",
        "image/bmp",
        "image/gif",
        "image/webp",
    ];

    /// Create a new driver instance.
    pub fn new() -> Self {
        Self::default()
    }

    /// Decode the given contents into a [`DynamicImage`].
    pub fn decode(&self, contents: &[u8]) -> Result<DynamicImage> {
        Ok(decode(contents)?.image)
    }

    /// Encode an image in the given format and quality.
    pub fn encode(
        &self,
        image: &DynamicImage,
        format: ImageFormat,
        quality: u8,
    ) -> Result<Vec<u8>> {
        encode(image, format, quality)
    }

    fn apply(
        &self,
        image: DynamicImage,
        transformation: &dyn Transformation,
        orientation: &mut Orientation,
    ) -> Result<DynamicImage> {
        if let Some(handler) = self.handlers.handler_for(transformation) {
            return handler.handle(image, transformation);
        }

        if transformation.is::<Orient>() {
            let mut image = image;
            image.apply_orientation(*orientation);
            *orientation = Orientation::NoTransforms;
            return Ok(image);
        }
        if let Some(cover) = transformation.downcast_ref::<Cover>() {
            return Ok(image.resize_to_fill(cover.width.max(1), cover.height.max(1), FILTER));
        }
        if let Some(contain) = transformation.downcast_ref::<Contain>() {
            return contain_image(image, contain);
        }
        if let Some(crop) = transformation.downcast_ref::<Crop>() {
            return Ok(crop_image(image, crop));
        }
        if let Some(resize) = transformation.downcast_ref::<Resize>() {
            let width = resize.width.unwrap_or(image.width()).max(1);
            let height = resize.height.unwrap_or(image.height()).max(1);
            return Ok(resize_to(image, width, height));
        }
        if let Some(rotate) = transformation.downcast_ref::<Rotate>() {
            return rotate_image(image, rotate);
        }
        if let Some(scale) = transformation.downcast_ref::<Scale>() {
            let (width, height) = fit(
                image.width(),
                image.height(),
                scale.width,
                scale.height,
                false,
            );
            return Ok(resize_to(image, width, height));
        }
        if let Some(blur) = transformation.downcast_ref::<Blur>() {
            return Ok(blur_image(image, blur.amount));
        }
        if transformation.is::<Grayscale>() {
            return Ok(grayscale(image));
        }
        if let Some(sharpen) = transformation.downcast_ref::<Sharpen>() {
            return Ok(sharpen_image(image, sharpen.amount));
        }
        if transformation.is::<FlipVertically>() {
            return Ok(image.flipv());
        }
        if transformation.is::<FlipHorizontally>() {
            return Ok(image.fliph());
        }

        Err(ImageException::new(format!(
            "The image transformation [{}] is not supported.",
            transformation.name()
        ))
        .into())
    }
}

impl ImageDriver for NativeDriver {
    fn process(&self, contents: &[u8], pipeline: &ImagePipeline) -> Result<Vec<u8>> {
        let Decoded {
            mut image,
            format,
            mut orientation,
        } = decode(contents)?;

        for transformation in &pipeline.transformations {
            image = self.apply(image, transformation.as_ref(), &mut orientation)?;
        }

        encode(
            &image,
            pipeline.output.format.unwrap_or(format),
            pipeline.output.quality_or_default(),
        )
    }

    fn dimensions(&self, contents: &[u8]) -> Result<(u32, u32)> {
        Ok(ImageReader::new(Cursor::new(contents))
            .with_guessed_format()?
            .into_dimensions()?)
    }

    fn dominant_color(&self, contents: &[u8]) -> Result<String> {
        let [red, green, blue] = average_color(&decode(contents)?.image);
        Ok(to_hex(red, green, blue))
    }

    fn transform_using(&self, handler: AnyTransformationHandler) -> Result<()> {
        Ok(self.handlers.register(&handler)?)
    }
}

// ----------------------------------------------------------------------
// Decoding & encoding
// ----------------------------------------------------------------------

fn decode(contents: &[u8]) -> Result<Decoded> {
    let mime_type = mime_type_of(contents);
    let (format, codec) = match mime_type {
        "image/jpeg" => (ImageFormat::Jpeg, CodecFormat::Jpeg),
        "image/png" => (ImageFormat::Png, CodecFormat::Png),
        "image/gif" => (ImageFormat::Gif, CodecFormat::Gif),
        "image/webp" => (ImageFormat::Webp, CodecFormat::WebP),
        "image/bmp" => (ImageFormat::Bmp, CodecFormat::Bmp),
        _ => {
            return Err(ImageException::new(format!(
                "The image format [{mime_type}] is not supported."
            ))
            .into());
        }
    };

    let mut decoder = ImageReader::with_format(Cursor::new(contents), codec).into_decoder()?;
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let image = DynamicImage::from_decoder(decoder)?;

    Ok(Decoded {
        image,
        format,
        orientation,
    })
}

fn encode(image: &DynamicImage, format: ImageFormat, quality: u8) -> Result<Vec<u8>> {
    let mut output = Vec::new();

    match format {
        ImageFormat::Jpeg => flatten(image, WHITE).write_with_encoder(
            JpegEncoder::new_with_quality(&mut output, quality.clamp(1, 100)),
        )?,
        ImageFormat::Png => image.write_with_encoder(PngEncoder::new(&mut output))?,
        ImageFormat::Gif => DynamicImage::ImageRgba8(image.to_rgba8())
            .write_with_encoder(GifEncoder::new_with_speed(&mut output, 10))?,
        ImageFormat::Webp => image.write_with_encoder(WebPEncoder::new_lossless(&mut output))?,
        ImageFormat::Bmp => image.write_with_encoder(BmpEncoder::new(&mut output))?,
        ImageFormat::Avif | ImageFormat::Heic => {
            return Err(ImageException::new(format!(
                "The [{format}] format is not supported by the [{}] image driver.",
                NativeDriver::NAME
            ))
            .into());
        }
    }

    Ok(output)
}

/// Composite an image with transparency onto a solid background, for
/// formats that can't store an alpha channel.
fn flatten(image: &DynamicImage, background: Rgba<u8>) -> Cow<'_, DynamicImage> {
    if !image.color().has_alpha() {
        return Cow::Borrowed(image);
    }

    let mut canvas = RgbaImage::from_pixel(image.width(), image.height(), background);
    imageops::overlay(&mut canvas, &image.to_rgba8(), 0, 0);
    Cow::Owned(DynamicImage::ImageRgb8(
        DynamicImage::ImageRgba8(canvas).into_rgb8(),
    ))
}

// ----------------------------------------------------------------------
// Transformations
// ----------------------------------------------------------------------

fn resize_to(image: DynamicImage, width: u32, height: u32) -> DynamicImage {
    if (width, height) == image.dimensions() {
        image
    } else {
        image.resize_exact(width, height, FILTER)
    }
}

/// The dimensions of an image proportionally resized to fit within the
/// given width and/or height, optionally never increasing its size.
fn fit(
    width: u32,
    height: u32,
    target_width: Option<u32>,
    target_height: Option<u32>,
    upscale: bool,
) -> (u32, u32) {
    let width_ratio = target_width.map(|target| f64::from(target.max(1)) / f64::from(width));
    let height_ratio = target_height.map(|target| f64::from(target.max(1)) / f64::from(height));

    let mut ratio = match (width_ratio, height_ratio) {
        (Some(w), Some(h)) => w.min(h),
        (Some(w), None) => w,
        (None, Some(h)) => h,
        (None, None) => 1.0,
    };
    if !upscale {
        ratio = ratio.min(1.0);
    }

    let side =
        |original: u32, target: Option<u32>, side_ratio: Option<f64>| match (target, side_ratio) {
            (Some(target), Some(side_ratio)) if side_ratio == ratio => target.max(1),
            _ => ((f64::from(original) * ratio).round() as u32).max(1),
        };

    (
        side(width, target_width, width_ratio),
        side(height, target_height, height_ratio),
    )
}

fn contain_image(image: DynamicImage, contain: &Contain) -> Result<DynamicImage> {
    let background = background(&image, contain.background.as_deref())?;
    let (width, height) = (contain.width.max(1), contain.height.max(1));
    let (fitted_width, fitted_height) = fit(
        image.width(),
        image.height(),
        Some(width),
        Some(height),
        true,
    );
    let resized = resize_to(image, fitted_width, fitted_height);

    let mut canvas = RgbaImage::from_pixel(width, height, background);
    imageops::overlay(
        &mut canvas,
        &resized.to_rgba8(),
        i64::from(width.saturating_sub(fitted_width) / 2),
        i64::from(height.saturating_sub(fitted_height) / 2),
    );

    Ok(DynamicImage::ImageRgba8(canvas))
}

fn crop_image(image: DynamicImage, crop: &Crop) -> DynamicImage {
    let (width, height) = (crop.width.max(1), crop.height.max(1));
    let within_bounds = crop.x >= 0
        && crop.y >= 0
        && crop.x + i64::from(width) <= i64::from(image.width())
        && crop.y + i64::from(height) <= i64::from(image.height());

    if within_bounds {
        return image.crop_imm(crop.x as u32, crop.y as u32, width, height);
    }

    // The crop reaches past the image: fill the uncovered area with white.
    let mut canvas = RgbaImage::from_pixel(width, height, WHITE);
    imageops::replace(&mut canvas, &image.to_rgba8(), -crop.x, -crop.y);
    DynamicImage::ImageRgba8(canvas)
}

fn rotate_image(image: DynamicImage, rotate: &Rotate) -> Result<DynamicImage> {
    if !rotate.angle.is_finite() {
        return Err(ImageException::new(format!(
            "The rotation angle [{}] is not a valid number.",
            rotate.angle
        ))
        .into());
    }

    let angle = rotate.angle.rem_euclid(360.0);
    let near = |degrees: f64| (angle - degrees).abs() < 1e-9;

    if near(0.0) || near(360.0) {
        return Ok(image);
    }
    if near(90.0) {
        return Ok(image.rotate90());
    }
    if near(180.0) {
        return Ok(image.rotate180());
    }
    if near(270.0) {
        return Ok(image.rotate270());
    }

    let background = background(&image, rotate.background.as_deref())?;
    Ok(rotate_freely(&image, angle, background))
}

/// Rotate clockwise by any angle, growing the canvas to fit the rotated
/// image and filling the corners with the background color.
fn rotate_freely(image: &DynamicImage, angle: f64, background: Rgba<u8>) -> DynamicImage {
    let source = image.to_rgba8();
    let (width, height) = (f64::from(source.width()), f64::from(source.height()));
    let (sin, cos) = angle.to_radians().sin_cos();

    let new_width = ((width * cos.abs() + height * sin.abs()).round() as u32).max(1);
    let new_height = ((width * sin.abs() + height * cos.abs()).round() as u32).max(1);
    let (center_x, center_y) = (width / 2.0, height / 2.0);
    let (new_center_x, new_center_y) = (f64::from(new_width) / 2.0, f64::from(new_height) / 2.0);

    let mut rotated = RgbaImage::from_pixel(new_width, new_height, background);
    for (x, y, pixel) in rotated.enumerate_pixels_mut() {
        let dx = f64::from(x) + 0.5 - new_center_x;
        let dy = f64::from(y) + 0.5 - new_center_y;

        // The inverse of a clockwise rotation (the y axis points down)...
        let source_x = dx * cos + dy * sin + center_x - 0.5;
        let source_y = -dx * sin + dy * cos + center_y - 0.5;

        *pixel = sample_bilinear(&source, source_x, source_y, background);
    }

    DynamicImage::ImageRgba8(rotated)
}

fn sample_bilinear(image: &RgbaImage, x: f64, y: f64, background: Rgba<u8>) -> Rgba<u8> {
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let (width, height) = (i64::from(image.width()), i64::from(image.height()));

    let at = |px: i64, py: i64| -> Rgba<u8> {
        if px < 0 || py < 0 || px >= width || py >= height {
            background
        } else {
            *image.get_pixel(px as u32, py as u32)
        }
    };

    let (x0, y0) = (x0 as i64, y0 as i64);
    let corners = [
        (at(x0, y0), (1.0 - fx) * (1.0 - fy)),
        (at(x0 + 1, y0), fx * (1.0 - fy)),
        (at(x0, y0 + 1), (1.0 - fx) * fy),
        (at(x0 + 1, y0 + 1), fx * fy),
    ];

    let mut channels = [0.0f64; 4];
    for (pixel, weight) in corners {
        for (channel, value) in channels.iter_mut().zip(pixel.0) {
            *channel += f64::from(value) * weight;
        }
    }

    Rgba(channels.map(|channel| channel.round().clamp(0.0, 255.0) as u8))
}

/// Blur like GD: `amount` passes of its 3x3 gaussian filter, which add up
/// to a gaussian blur with a variance of `amount / 2`.
fn blur_image(image: DynamicImage, amount: u8) -> DynamicImage {
    if amount == 0 {
        return image;
    }
    image.blur((f32::from(amount) / 2.0).sqrt())
}

/// Sharpen with Intervention's GD sharpening matrix.
fn sharpen_image(image: DynamicImage, amount: u8) -> DynamicImage {
    if amount == 0 {
        return image;
    }

    let amount = f32::from(amount);
    let min = if amount >= 10.0 { amount * -0.01 } else { 0.0 };
    let max = amount * -0.025;
    let center = -(4.0 * min + 4.0 * max) + 1.0;

    convolve(
        &image,
        [[min, max, min], [max, center, max], [min, max, min]],
    )
}

/// Apply a 3x3 convolution to the color channels, clamping at the edges
/// and leaving the alpha channel alone.
fn convolve(image: &DynamicImage, kernel: [[f32; 3]; 3]) -> DynamicImage {
    let had_alpha = image.color().has_alpha();
    let source = image.to_rgba8();
    let (width, height) = source.dimensions();
    let mut output = source.clone();

    for (x, y, pixel) in output.enumerate_pixels_mut() {
        let mut sums = [0.0f32; 3];
        for (row, weights) in kernel.iter().enumerate() {
            for (column, weight) in weights.iter().enumerate() {
                let sx = (i64::from(x) + column as i64 - 1).clamp(0, i64::from(width) - 1) as u32;
                let sy = (i64::from(y) + row as i64 - 1).clamp(0, i64::from(height) - 1) as u32;
                let neighbour = source.get_pixel(sx, sy);
                for (sum, value) in sums.iter_mut().zip(neighbour.0) {
                    *sum += f32::from(value) * weight;
                }
            }
        }
        for (channel, sum) in pixel.0.iter_mut().zip(sums) {
            *channel = sum.round().clamp(0.0, 255.0) as u8;
        }
    }

    let output = DynamicImage::ImageRgba8(output);
    if had_alpha {
        output
    } else {
        DynamicImage::ImageRgb8(output.into_rgb8())
    }
}

/// Convert to grayscale, keeping the alpha channel (like GD).
fn grayscale(image: DynamicImage) -> DynamicImage {
    let had_alpha = image.color().has_alpha();
    let gray = image.grayscale();
    if had_alpha {
        DynamicImage::ImageRgba8(gray.to_rgba8())
    } else {
        DynamicImage::ImageRgb8(gray.to_rgb8())
    }
}

/// Resolve a background color, expanding the "dominant" sentinel.
fn background(image: &DynamicImage, background: Option<&str>) -> Result<Rgba<u8>> {
    match background {
        None => Ok(WHITE),
        Some("dominant") => {
            let [red, green, blue] = average_color(image);
            Ok(Rgba([red, green, blue, 255]))
        }
        Some(color) => Ok(parse_color(color)?),
    }
}

/// The image's average color, weighting each pixel by its opacity (like
/// GD's resampling) and ignoring the alpha channel itself.
fn average_color(image: &DynamicImage) -> [u8; 3] {
    let mut weighted = [0u64; 3];
    let mut plain = [0u64; 3];
    let mut total_alpha = 0u64;
    let mut count = 0u64;

    for (_, _, pixel) in image.pixels() {
        let alpha = u64::from(pixel[3]);
        for channel in 0..3 {
            weighted[channel] += u64::from(pixel[channel]) * alpha;
            plain[channel] += u64::from(pixel[channel]);
        }
        total_alpha += alpha;
        count += 1;
    }

    let (sums, divisor) = if total_alpha > 0 {
        (weighted, total_alpha)
    } else {
        (plain, count.max(1))
    };

    sums.map(|sum| ((sum + divisor / 2) / divisor).min(255) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transformations::Transformation;

    fn png(image: &DynamicImage) -> Vec<u8> {
        let mut bytes = Vec::new();
        image
            .write_to(&mut Cursor::new(&mut bytes), CodecFormat::Png)
            .unwrap();
        bytes
    }

    fn solid(width: u32, height: u32, color: [u8; 4]) -> DynamicImage {
        DynamicImage::ImageRgba8(RgbaImage::from_pixel(width, height, Rgba(color)))
    }

    fn process(image: &DynamicImage, transformation: impl Transformation) -> DynamicImage {
        let mut pipeline = ImagePipeline::new();
        pipeline.add(transformation);
        let driver = NativeDriver::new();
        driver
            .decode(&driver.process(&png(image), &pipeline).unwrap())
            .unwrap()
    }

    #[test]
    fn fit_matches_intervention() {
        assert_eq!(fit(400, 200, Some(200), Some(200), false), (200, 100));
        assert_eq!(fit(400, 200, Some(200), None, false), (200, 100));
        assert_eq!(fit(400, 200, None, Some(100), false), (200, 100));
        assert_eq!(fit(100, 80, Some(800), Some(600), false), (100, 80));
        assert_eq!(fit(100, 80, Some(800), Some(600), true), (750, 600));
        assert_eq!(fit(400, 200, Some(200), Some(200), true), (200, 100));
        assert_eq!(fit(3, 1000, Some(10), Some(10), false), (1, 10));
    }

    #[test]
    fn it_resizes_with_one_dimension() {
        let image = solid(40, 20, [1, 2, 3, 255]);
        assert_eq!(
            process(&image, Resize::new(Some(10), None)).dimensions(),
            (10, 20)
        );
        assert_eq!(
            process(&image, Resize::new(None, Some(5))).dimensions(),
            (40, 5)
        );
    }

    #[test]
    fn it_contains_with_a_background() {
        let image = solid(40, 20, [255, 0, 0, 255]);
        let contained = process(&image, Contain::new(20, 20, Some("#0000ff".into()))).to_rgba8();

        assert_eq!(contained.dimensions(), (20, 20));
        assert_eq!(contained.get_pixel(0, 0), &Rgba([0, 0, 255, 255]));
        assert_eq!(contained.get_pixel(10, 10), &Rgba([255, 0, 0, 255]));

        let dominant = process(&image, Contain::new(20, 20, Some("dominant".into()))).to_rgba8();
        assert_eq!(dominant.get_pixel(0, 0), &Rgba([255, 0, 0, 255]));

        let default = process(&image, Contain::new(20, 20, None)).to_rgba8();
        assert_eq!(default.get_pixel(0, 0), &WHITE);
    }

    #[test]
    fn it_crops_past_the_edges() {
        let image = solid(10, 10, [0, 0, 0, 255]);
        let cropped = process(&image, Crop::new(10, 10, 5, 5)).to_rgba8();

        assert_eq!(cropped.dimensions(), (10, 10));
        assert_eq!(cropped.get_pixel(0, 0), &Rgba([0, 0, 0, 255]));
        assert_eq!(cropped.get_pixel(9, 9), &WHITE);

        let negative = process(&image, Crop::new(4, 4, -2, -2)).to_rgba8();
        assert_eq!(negative.get_pixel(0, 0), &WHITE);
        assert_eq!(negative.get_pixel(3, 3), &Rgba([0, 0, 0, 255]));
    }

    #[test]
    fn it_rotates_clockwise() {
        let mut image = RgbaImage::from_pixel(2, 1, Rgba([0, 0, 0, 255]));
        image.put_pixel(0, 0, Rgba([255, 0, 0, 255]));
        let image = DynamicImage::ImageRgba8(image);

        // The red pixel on the left ends up on top after a clockwise turn.
        let rotated = process(&image, Rotate::new(90.0, None)).to_rgba8();
        assert_eq!(rotated.dimensions(), (1, 2));
        assert_eq!(rotated.get_pixel(0, 0), &Rgba([255, 0, 0, 255]));

        let back = process(&image, Rotate::new(-270.0, None)).to_rgba8();
        assert_eq!(back.get_pixel(0, 0), &Rgba([255, 0, 0, 255]));

        assert_eq!(
            process(&image, Rotate::new(360.0, None)).dimensions(),
            (2, 1)
        );
        assert_eq!(
            process(&image, Rotate::new(180.0, None)).dimensions(),
            (2, 1)
        );
    }

    #[test]
    fn it_rotates_by_any_angle_onto_a_background() {
        let image = solid(100, 50, [0, 255, 0, 255]);
        let rotated = process(&image, Rotate::new(45.0, Some("#ff0000".into()))).to_rgba8();

        assert_eq!(rotated.dimensions(), (106, 106));
        assert_eq!(rotated.get_pixel(0, 0), &Rgba([255, 0, 0, 255]));
        assert_eq!(rotated.get_pixel(53, 53), &Rgba([0, 255, 0, 255]));

        let error = NativeDriver::new()
            .process(&png(&image), &{
                let mut pipeline = ImagePipeline::new();
                pipeline.add(Rotate::new(f64::NAN, None));
                pipeline
            })
            .unwrap_err();
        assert!(error.to_string().contains("is not a valid number"));
    }

    #[test]
    fn it_sharpens_and_blurs_without_touching_edges_or_alpha() {
        let mut image = RgbaImage::from_pixel(9, 9, Rgba([100, 100, 100, 128]));
        image.put_pixel(4, 4, Rgba([200, 200, 200, 128]));
        let image = DynamicImage::ImageRgba8(image);

        let sharpened = process(&image, Sharpen::new(50)).to_rgba8();
        assert!(sharpened.get_pixel(4, 4)[0] > 200);
        assert!(sharpened.get_pixel(3, 4)[0] < 100);
        assert_eq!(sharpened.get_pixel(0, 0), &Rgba([100, 100, 100, 128]));
        assert!(sharpened.pixels().all(|pixel| pixel[3] == 128));

        let blurred = process(&image, Blur::new(10)).to_rgba8();
        assert!(blurred.get_pixel(4, 4)[0] < 200);

        assert_eq!(
            process(&image, Sharpen::new(0)),
            process(&image, Blur::new(0))
        );
    }

    #[test]
    fn it_converts_to_grayscale_and_flips() {
        let mut image = RgbaImage::from_pixel(2, 2, Rgba([0, 0, 255, 255]));
        image.put_pixel(0, 0, Rgba([255, 0, 0, 255]));
        let image = DynamicImage::ImageRgba8(image);

        let gray = process(&image, Grayscale).to_rgba8();
        assert!(gray.pixels().all(|p| p[0] == p[1] && p[1] == p[2]));

        let flipped = process(&image, FlipHorizontally).to_rgba8();
        assert_eq!(flipped.get_pixel(1, 0), &Rgba([255, 0, 0, 255]));

        let flipped = process(&image, FlipVertically).to_rgba8();
        assert_eq!(flipped.get_pixel(0, 1), &Rgba([255, 0, 0, 255]));
    }

    #[test]
    fn dominant_color_is_the_opacity_weighted_average() {
        let driver = NativeDriver::new();
        assert_eq!(
            driver
                .dominant_color(&png(&solid(4, 4, [0, 128, 255, 255])))
                .unwrap(),
            "#0080ff"
        );
        assert_eq!(
            driver
                .dominant_color(&png(&solid(4, 4, [0, 128, 255, 128])))
                .unwrap(),
            "#0080ff"
        );
        assert_eq!(
            driver
                .dominant_color(&png(&solid(4, 4, [10, 20, 30, 0])))
                .unwrap(),
            "#0a141e"
        );

        let mut half = RgbaImage::from_pixel(2, 1, Rgba([0, 0, 0, 255]));
        half.put_pixel(1, 0, Rgba([255, 255, 255, 0]));
        assert_eq!(
            driver
                .dominant_color(&png(&DynamicImage::ImageRgba8(half)))
                .unwrap(),
            "#000000"
        );
    }

    #[test]
    fn jpeg_output_is_flattened_onto_white() {
        let transparent = solid(4, 4, [0, 0, 0, 0]);
        let driver = NativeDriver::new();
        let jpeg = driver.encode(&transparent, ImageFormat::Jpeg, 90).unwrap();

        assert_eq!(mime_type_of(&jpeg), "image/jpeg");
        assert_eq!(driver.dominant_color(&jpeg).unwrap(), "#ffffff");
    }

    #[test]
    fn unsupported_inputs_and_outputs_fail() {
        let driver = NativeDriver::new();

        assert_eq!(
            driver
                .process(b"not-an-image", &ImagePipeline::new())
                .unwrap_err()
                .to_string(),
            "The image format [text/plain] is not supported."
        );
        assert_eq!(
            driver
                .encode(&solid(1, 1, [0, 0, 0, 255]), ImageFormat::Avif, 70)
                .unwrap_err()
                .to_string(),
            "The [avif] format is not supported by the [image] image driver."
        );

        #[derive(Debug)]
        struct Unknown;
        impl Transformation for Unknown {}

        let mut pipeline = ImagePipeline::new();
        pipeline.add(Unknown);
        let error = driver
            .process(&png(&solid(1, 1, [0, 0, 0, 255])), &pipeline)
            .unwrap_err();
        assert!(error.to_string().starts_with("The image transformation ["));
        assert!(error.to_string().ends_with("Unknown] is not supported."));
    }
}
