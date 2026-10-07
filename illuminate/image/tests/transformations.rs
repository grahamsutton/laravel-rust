mod common;

use common::{app, decode, gradient, jpeg, jpeg_with_orientation, png, solid_png};
use illuminate_image::Image;
use illuminate_image::transformations::{Contain, Cover, Crop, Resize, Rotate, Scale};
use illuminate_support::Conditionable;
use image::{GenericImageView, Rgba};

#[tokio::test]
async fn cover_resizes_and_crops_to_the_exact_dimensions() {
    let _app = app();
    let image = Image::from_bytes(jpeg(200, 200));

    assert_eq!(image.cover(100, 50).dimensions().await.unwrap(), (100, 50));
    assert_eq!(
        image.cover(400, 100).dimensions().await.unwrap(),
        (400, 100)
    );
    assert_eq!(image.cover(0, 0).dimensions().await.unwrap(), (1, 1));
}

#[tokio::test]
async fn contain_fits_the_image_onto_a_background() {
    let _app = app();
    let image = Image::from_bytes(png(400, 200));

    assert_eq!(
        image.contain(200, 200).dimensions().await.unwrap(),
        (200, 200)
    );
    assert_eq!(
        image.contain(800, 800).dimensions().await.unwrap(),
        (800, 800)
    );

    let contained = decode(
        &image
            .contain_with(200, 200, "#ff0000")
            .to_bytes()
            .await
            .unwrap(),
    );
    assert_eq!(contained.get_pixel(0, 0), Rgba([255, 0, 0, 255]));
    assert_eq!(contained.get_pixel(199, 199), Rgba([255, 0, 0, 255]));

    let white = decode(&image.contain(200, 200).to_bytes().await.unwrap());
    assert_eq!(white.get_pixel(0, 0), Rgba([255, 255, 255, 255]));
}

#[tokio::test]
async fn contain_can_use_the_dominant_color_as_background() {
    let _app = app();
    let image = Image::from_bytes(solid_png(400, 200, [255, 0, 0, 255]));

    let contained = image.contain_with(200, 200, "dominant");
    assert_eq!(contained.dimensions().await.unwrap(), (200, 200));
    assert_eq!(contained.dominant_color().await.unwrap(), "#ff0000");
}

#[tokio::test]
async fn invalid_background_colors_fail_when_processed() {
    let _app = app();
    let error = Image::from_bytes(png(10, 10))
        .contain_with(20, 20, "blurple")
        .to_bytes()
        .await
        .unwrap_err();

    assert_eq!(error.to_string(), "Unable to parse the color [blurple].");
}

#[tokio::test]
async fn crop_takes_dimensions_and_an_optional_position() {
    let _app = app();
    let image = Image::from_bytes(png(400, 200));

    assert_eq!(image.crop(100, 50).dimensions().await.unwrap(), (100, 50));
    assert_eq!(
        image.crop_at(100, 50, 10, 20).dimensions().await.unwrap(),
        (100, 50)
    );
    assert_eq!(
        image
            .crop_at(300, 300, 350, 150)
            .dimensions()
            .await
            .unwrap(),
        (300, 300)
    );

    // The crop starts at the given position...
    let source = gradient(400, 200);
    let cropped = decode(&image.crop_at(10, 10, 50, 25).to_bytes().await.unwrap());
    assert_eq!(cropped.get_pixel(0, 0), source.get_pixel(50, 25));

    assert_eq!(
        image.crop_at(300, 200, 50, 25).pipeline().transformations[0]
            .downcast_ref::<Crop>()
            .unwrap(),
        &Crop::new(300, 200, 50, 25)
    );
}

#[tokio::test]
async fn resize_sets_one_or_both_dimensions() {
    let _app = app();
    let image = Image::from_bytes(png(400, 200));

    assert_eq!(
        image.resize(200, 200).dimensions().await.unwrap(),
        (200, 200)
    );
    assert_eq!(
        image.resize_width(100).dimensions().await.unwrap(),
        (100, 200)
    );
    assert_eq!(
        image.resize_height(50).dimensions().await.unwrap(),
        (400, 50)
    );
    assert_eq!(
        image.resize(800, 600).dimensions().await.unwrap(),
        (800, 600)
    );
    assert_eq!(
        image
            .resize_with(Some(30), None)
            .unwrap()
            .dimensions()
            .await
            .unwrap(),
        (30, 200)
    );
    assert_eq!(
        image.resize_with(None, None).unwrap_err().to_string(),
        "At least one resize dimension must be specified."
    );
    assert_eq!(
        image.resize_width(0).pipeline().transformations[0].downcast_ref::<Resize>(),
        Some(&Resize::new(Some(1), None))
    );
}

#[tokio::test]
async fn scale_shrinks_proportionally_but_never_upscales() {
    let _app = app();
    let image = Image::from_bytes(png(400, 200));

    assert_eq!(
        image.scale(200, 200).dimensions().await.unwrap(),
        (200, 100)
    );
    assert_eq!(
        image.scale_width(200).dimensions().await.unwrap(),
        (200, 100)
    );
    assert_eq!(
        image.scale_height(100).dimensions().await.unwrap(),
        (200, 100)
    );
    assert_eq!(
        image.scale(800, 600).dimensions().await.unwrap(),
        (400, 200)
    );
    assert_eq!(
        image.scale_width(1000).dimensions().await.unwrap(),
        (400, 200)
    );
    assert_eq!(
        image
            .scale_with(None, Some(20))
            .unwrap()
            .dimensions()
            .await
            .unwrap(),
        (40, 20)
    );
    assert_eq!(
        image.scale_with(None, None).unwrap_err().to_string(),
        "At least one scale dimension must be specified."
    );

    let small = Image::from_bytes(png(100, 80));
    assert_eq!(small.scale(800, 600).dimensions().await.unwrap(), (100, 80));
}

#[tokio::test]
async fn orient_applies_the_exif_orientation() {
    let _app = app();
    let rotated = Image::from_bytes(jpeg_with_orientation(100, 50, 6));

    // The raw image is 100x50, but the camera was turned...
    assert_eq!(rotated.dimensions().await.unwrap(), (100, 50));
    assert_eq!(rotated.orient().dimensions().await.unwrap(), (50, 100));
    assert_eq!(
        rotated.orient().orient().dimensions().await.unwrap(),
        (50, 100)
    );

    let upright = Image::from_bytes(jpeg_with_orientation(100, 50, 1));
    assert_eq!(upright.orient().dimensions().await.unwrap(), (100, 50));
    assert_eq!(
        Image::from_bytes(png(100, 100))
            .orient()
            .dimensions()
            .await
            .unwrap(),
        (100, 100)
    );
}

#[tokio::test]
async fn rotate_turns_the_image_clockwise() {
    let _app = app();
    let image = Image::from_bytes(png(100, 50));

    assert_eq!(image.rotate(90).dimensions().await.unwrap(), (50, 100));
    assert_eq!(image.rotate(-90).dimensions().await.unwrap(), (50, 100));
    assert_eq!(image.rotate(180).dimensions().await.unwrap(), (100, 50));
    assert_eq!(image.rotate(45.0).dimensions().await.unwrap(), (106, 106));

    // Clockwise: the top left corner ends up in the top right...
    let source = gradient(100, 50);
    let turned = decode(&image.rotate(90).to_bytes().await.unwrap());
    assert_eq!(turned.get_pixel(49, 0), source.get_pixel(0, 0));

    // Branching after processing doesn't re-apply anything...
    let once = image.rotate(90);
    once.to_bytes().await.unwrap();
    assert_eq!(once.rotate(90).dimensions().await.unwrap(), (100, 50));
}

#[tokio::test]
async fn rotate_fills_the_corners_with_a_background() {
    let _app = app();
    let image = Image::from_bytes(solid_png(100, 50, [0, 255, 0, 255]));

    let rotated = decode(&image.rotate_with(45, "#0000ff").to_bytes().await.unwrap());
    assert_eq!(rotated.get_pixel(0, 0), Rgba([0, 0, 255, 255]));

    let dominant = decode(&image.rotate_with(45, "dominant").to_bytes().await.unwrap());
    assert_eq!(dominant.get_pixel(0, 0), Rgba([0, 255, 0, 255]));
    assert_eq!(
        image.rotate_with(45, "dominant").pipeline().transformations[0].downcast_ref::<Rotate>(),
        Some(&Rotate::new(45.0, Some("dominant".into())))
    );
}

#[tokio::test]
async fn blur_sharpen_and_grayscale_keep_the_dimensions() {
    let _app = app();
    let contents = png(100, 100);
    let image = Image::from_bytes(contents.clone());

    for variant in [
        image.blur(10),
        image.sharpen(10),
        image.grayscale(),
        image.blur(0).sharpen(100),
    ] {
        let bytes = variant.to_bytes().await.unwrap();
        assert_ne!(bytes, contents);
        assert_eq!(variant.dimensions().await.unwrap(), (100, 100));
    }

    let gray = decode(&image.grayscale().to_bytes().await.unwrap()).to_rgb8();
    assert!(
        gray.pixels()
            .all(|pixel| pixel[0] == pixel[1] && pixel[1] == pixel[2])
    );
}

#[tokio::test]
async fn amounts_are_clamped() {
    let image = Image::from_bytes(Vec::new());

    let blur = image.blur(500);
    let sharpen = image.sharpen(-5);

    assert_eq!(
        format!("{:?}", blur.pipeline().transformations[0]),
        "Blur { amount: 100 }"
    );
    assert_eq!(
        format!("{:?}", sharpen.pipeline().transformations[0]),
        "Sharpen { amount: 0 }"
    );
}

#[tokio::test]
async fn flips_mirror_the_image() {
    let _app = app();
    let source = gradient(100, 50);
    let image = Image::from_bytes(png(100, 50));

    let vertical = decode(&image.flip_vertically().to_bytes().await.unwrap());
    assert_eq!(vertical.dimensions(), (100, 50));
    assert_eq!(vertical.get_pixel(0, 49), source.get_pixel(0, 0));

    let horizontal = decode(&image.flip_horizontally().to_bytes().await.unwrap());
    assert_eq!(horizontal.get_pixel(99, 0), source.get_pixel(0, 0));

    let both = decode(&image.flip().flop().to_bytes().await.unwrap());
    assert_eq!(both.get_pixel(99, 49), source.get_pixel(0, 0));
}

#[tokio::test]
async fn images_are_immutable() {
    let _app = app();
    let image = Image::from_bytes(jpeg(300, 300));

    let avatar = image.cover(100, 100);
    let banner = image.cover(300, 100).to_png();
    let thumbnail = image.scale(50, 50);

    assert!(image.pipeline().transformations.is_empty());
    assert!(!image.pipeline().has_changes());
    assert_eq!(avatar.dimensions().await.unwrap(), (100, 100));
    assert_eq!(banner.dimensions().await.unwrap(), (300, 100));
    assert_eq!(banner.mime_type().await.unwrap(), "image/png");
    assert_eq!(thumbnail.dimensions().await.unwrap(), (50, 50));
    assert_eq!(image.dimensions().await.unwrap(), (300, 300));
    assert_eq!(image.mime_type().await.unwrap(), "image/jpeg");
}

#[tokio::test]
async fn transformations_are_applied_in_order() {
    let _app = app();
    let image = Image::from_bytes(png(400, 400))
        .cover(200, 100)
        .rotate(90)
        .scale_width(25)
        .cover(10, 10)
        .cover(20, 30);

    let transformations = &image.pipeline().transformations;
    assert_eq!(transformations.len(), 5);
    assert!(transformations[0].is::<Cover>());
    assert!(transformations[1].is::<Rotate>());
    assert!(transformations[2].is::<Scale>());
    assert_eq!(image.dimensions().await.unwrap(), (20, 30));

    let contain = Image::from_bytes(Vec::new()).contain_with(10, 20, "#fff");
    assert_eq!(
        contain.pipeline().transformations[0].downcast_ref::<Contain>(),
        Some(&Contain::new(10, 20, Some("#fff".into())))
    );
}

#[tokio::test]
async fn images_are_conditionable() {
    let _app = app();
    let image = Image::from_bytes(png(100, 100));

    let cropped = image
        .clone()
        .when(true, |image| image.cover(40, 40))
        .unless(true, |image| image.to_webp());
    assert_eq!(cropped.dimensions().await.unwrap(), (40, 40));
    assert_eq!(cropped.mime_type().await.unwrap(), "image/png");

    let converted = image
        .when(false, |image| image.cover(40, 40))
        .unless(false, |image| image.to_webp());
    assert_eq!(converted.dimensions().await.unwrap(), (100, 100));
    assert_eq!(converted.mime_type().await.unwrap(), "image/webp");
}

#[tokio::test]
async fn the_full_avatar_pipeline() {
    let _app = app();
    let image = Image::from_bytes(jpeg_with_orientation(640, 480, 6))
        .orient()
        .cover(400, 400)
        .sharpen(10)
        .to_webp()
        .quality(80);

    assert_eq!(image.dimensions().await.unwrap(), (400, 400));
    assert_eq!(image.mime_type().await.unwrap(), "image/webp");
    assert_eq!(image.extension().await.unwrap(), "webp");
}
