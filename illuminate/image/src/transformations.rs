//! Image transformations: the built-in ones, and the contract for your own.
//!
//! A transformation is plain data describing *what* should happen to an
//! image. Drivers decide *how*: the built-in driver knows every
//! transformation in this module, and custom transformations are taught to
//! a driver with [`Image::transform_using`](crate::Image::transform_using).

use std::any::{Any, TypeId, type_name};
use std::collections::HashMap;
use std::fmt;
use std::marker::PhantomData;
use std::sync::{Arc, RwLock};

use illuminate_support::Result;

use crate::exception::ImageException;

/// An image transformation (Laravel's `Illuminate\Contracts\Image\Transformation`).
///
/// Implement it for your own transformations, register a handler for them
/// with [`Image::transform_using`](crate::Image::transform_using), and add
/// them to an image's pipeline with [`Image::transform`](crate::Image::transform):
///
/// ```
/// use illuminate_image::Transformation;
///
/// #[derive(Debug)]
/// pub struct Pixelate {
///     pub size: u32,
/// }
///
/// impl Transformation for Pixelate {}
/// ```
pub trait Transformation: Any + Send + Sync + fmt::Debug + 'static {
    /// The transformation's name, for error messages.
    fn name(&self) -> &'static str {
        type_name::<Self>()
    }
}

impl dyn Transformation {
    /// Determine if the transformation is a `T`.
    pub fn is<T: Transformation>(&self) -> bool {
        (self as &dyn Any).is::<T>()
    }

    /// Get the transformation as a `T`, if it is one.
    pub fn downcast_ref<T: Transformation>(&self) -> Option<&T> {
        (self as &dyn Any).downcast_ref::<T>()
    }
}

// ----------------------------------------------------------------------
// The built-in transformations
// ----------------------------------------------------------------------

/// Resize the image to the given dimensions. A missing dimension keeps
/// the image's current size on that side.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Resize {
    pub width: Option<u32>,
    pub height: Option<u32>,
}

impl Resize {
    /// Resize to the given width and/or height.
    pub fn new(width: Option<u32>, height: Option<u32>) -> Self {
        Self { width, height }
    }
}

/// Proportionally scale the image down to fit within the given dimensions.
/// Never increases the size of the image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Scale {
    pub width: Option<u32>,
    pub height: Option<u32>,
}

impl Scale {
    /// Scale down to the given width and/or height.
    pub fn new(width: Option<u32>, height: Option<u32>) -> Self {
        Self { width, height }
    }
}

/// Resize and crop the image to completely cover the given dimensions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cover {
    pub width: u32,
    pub height: u32,
}

impl Cover {
    /// Cover the given dimensions.
    pub fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }
}

/// Resize the image to fit within the given dimensions, filling the empty
/// space with a background color (a color, or `"dominant"`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Contain {
    pub width: u32,
    pub height: u32,
    pub background: Option<String>,
}

impl Contain {
    /// Contain the image within the given dimensions.
    pub fn new(width: u32, height: u32, background: Option<String>) -> Self {
        Self {
            width,
            height,
            background,
        }
    }
}

/// Crop the image to the given dimensions, starting at the given position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Crop {
    pub width: u32,
    pub height: u32,
    pub x: i64,
    pub y: i64,
}

impl Crop {
    /// Crop to the given dimensions and position.
    pub fn new(width: u32, height: u32, x: i64, y: i64) -> Self {
        Self {
            width,
            height,
            x,
            y,
        }
    }
}

/// Rotate the image according to its EXIF orientation data.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Orient;

/// Rotate the image clockwise by the given angle, filling any uncovered
/// area with a background color (a color, or `"dominant"`).
#[derive(Clone, Debug, PartialEq)]
pub struct Rotate {
    pub angle: f64,
    pub background: Option<String>,
}

impl Rotate {
    /// Rotate clockwise by the given angle, in degrees.
    pub fn new(angle: f64, background: Option<String>) -> Self {
        Self { angle, background }
    }
}

/// Blur the image by the given amount (0 to 100).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Blur {
    pub amount: u8,
}

impl Blur {
    /// Blur by the given amount, clamped to 0..=100.
    pub fn new(amount: i64) -> Self {
        Self {
            amount: amount.clamp(0, 100) as u8,
        }
    }
}

/// Sharpen the image by the given amount (0 to 100).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sharpen {
    pub amount: u8,
}

impl Sharpen {
    /// Sharpen by the given amount, clamped to 0..=100.
    pub fn new(amount: i64) -> Self {
        Self {
            amount: amount.clamp(0, 100) as u8,
        }
    }
}

/// Convert the image to grayscale.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Grayscale;

/// Flip the image vertically (top to bottom).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FlipVertically;

/// Flip the image horizontally (left to right).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FlipHorizontally;

macro_rules! builtin_transformations {
    ($($name:ident => $label:literal),* $(,)?) => {
        $(
            impl Transformation for $name {
                fn name(&self) -> &'static str {
                    $label
                }
            }
        )*
    };
}

builtin_transformations! {
    Resize => "Illuminate\\Image\\Transformations\\Resize",
    Scale => "Illuminate\\Image\\Transformations\\Scale",
    Cover => "Illuminate\\Image\\Transformations\\Cover",
    Contain => "Illuminate\\Image\\Transformations\\Contain",
    Crop => "Illuminate\\Image\\Transformations\\Crop",
    Orient => "Illuminate\\Image\\Transformations\\Orient",
    Rotate => "Illuminate\\Image\\Transformations\\Rotate",
    Blur => "Illuminate\\Image\\Transformations\\Blur",
    Sharpen => "Illuminate\\Image\\Transformations\\Sharpen",
    Grayscale => "Illuminate\\Image\\Transformations\\Grayscale",
    FlipVertically => "Illuminate\\Image\\Transformations\\FlipVertically",
    FlipHorizontally => "Illuminate\\Image\\Transformations\\FlipHorizontally",
}

// ----------------------------------------------------------------------
// Transformation handlers
// ----------------------------------------------------------------------

type HandlerFn<I> = dyn Fn(I, &dyn Transformation) -> Result<I> + Send + Sync;

/// A handler that applies a custom transformation to a driver's native
/// image type `I` (for the built-in driver, [`DynamicImage`](::image::DynamicImage)).
pub struct TransformationHandler<I> {
    handler: Arc<HandlerFn<I>>,
}

impl<I> Clone for TransformationHandler<I> {
    fn clone(&self) -> Self {
        Self {
            handler: self.handler.clone(),
        }
    }
}

impl<I> fmt::Debug for TransformationHandler<I> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TransformationHandler")
            .field("image", &type_name::<I>())
            .finish_non_exhaustive()
    }
}

impl<I: 'static> TransformationHandler<I> {
    /// Wrap a handler for transformations of type `T`.
    pub fn new<T: Transformation>(
        handler: impl Fn(I, &T) -> Result<I> + Send + Sync + 'static,
    ) -> Self {
        Self {
            handler: Arc::new(move |image, transformation: &dyn Transformation| {
                let transformation = transformation.downcast_ref::<T>().ok_or_else(|| {
                    ImageException::new(format!(
                        "The transformation handler for [{}] can't apply [{}].",
                        type_name::<T>(),
                        transformation.name(),
                    ))
                })?;
                handler(image, transformation)
            }),
        }
    }

    /// Apply the transformation to the given image.
    pub fn handle(&self, image: I, transformation: &dyn Transformation) -> Result<I> {
        (self.handler)(image, transformation)
    }
}

/// A transformation handler whose image type has been erased, so it may be
/// handed to any [`ImageDriver`](crate::ImageDriver). Drivers recover the
/// handler for their own image type with [`AnyTransformationHandler::downcast`].
#[derive(Clone)]
pub struct AnyTransformationHandler {
    transformation: TypeId,
    transformation_name: &'static str,
    image_type: &'static str,
    handler: Arc<dyn Any + Send + Sync>,
}

impl fmt::Debug for AnyTransformationHandler {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AnyTransformationHandler")
            .field("transformation", &self.transformation_name)
            .field("image", &self.image_type)
            .finish_non_exhaustive()
    }
}

impl AnyTransformationHandler {
    /// Create a handler applying transformations of type `T` to images of type `I`.
    ///
    /// ```
    /// use illuminate_image::{AnyTransformationHandler, Transformation};
    /// use illuminate_image::image::DynamicImage;
    ///
    /// #[derive(Debug)]
    /// struct Invert;
    /// impl Transformation for Invert {}
    ///
    /// let handler = AnyTransformationHandler::new(|mut image: DynamicImage, _: &Invert| {
    ///     image.invert();
    ///     Ok(image)
    /// });
    ///
    /// assert!(handler.handles::<Invert>());
    /// assert!(handler.downcast::<DynamicImage>().is_ok());
    /// assert!(handler.downcast::<String>().is_err());
    /// ```
    pub fn new<T, I>(handler: impl Fn(I, &T) -> Result<I> + Send + Sync + 'static) -> Self
    where
        T: Transformation,
        I: Send + 'static,
    {
        Self {
            transformation: TypeId::of::<T>(),
            transformation_name: type_name::<T>(),
            image_type: type_name::<I>(),
            handler: Arc::new(TransformationHandler::<I>::new(handler)),
        }
    }

    /// The type of transformation this handler applies.
    pub fn transformation(&self) -> TypeId {
        self.transformation
    }

    /// The name of the transformation this handler applies.
    pub fn transformation_name(&self) -> &'static str {
        self.transformation_name
    }

    /// The name of the image type this handler works with.
    pub fn image_type(&self) -> &'static str {
        self.image_type
    }

    /// Determine if this handler applies transformations of type `T`.
    pub fn handles<T: Transformation>(&self) -> bool {
        self.transformation == TypeId::of::<T>()
    }

    /// Recover the handler for the image type `I`, or fail if it was
    /// written for a different image type.
    pub fn downcast<I: 'static>(&self) -> Result<TransformationHandler<I>, ImageException> {
        self.handler
            .downcast_ref::<TransformationHandler<I>>()
            .cloned()
            .ok_or_else(|| {
                ImageException::new(format!(
                    "The [{}] transformation handler works with [{}] images, but the driver works with [{}] images.",
                    self.transformation_name,
                    self.image_type,
                    type_name::<I>(),
                ))
            })
    }
}

/// A registry of transformation handlers for a driver whose native image
/// type is `I`. Custom drivers embed one to implement
/// [`ImageDriver::transform_using`](crate::ImageDriver::transform_using).
pub struct TransformationHandlers<I> {
    handlers: RwLock<HashMap<TypeId, TransformationHandler<I>>>,
    _image: PhantomData<fn() -> I>,
}

impl<I> Default for TransformationHandlers<I> {
    fn default() -> Self {
        Self {
            handlers: RwLock::new(HashMap::new()),
            _image: PhantomData,
        }
    }
}

impl<I> fmt::Debug for TransformationHandlers<I> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TransformationHandlers")
            .field("count", &self.handlers.read().unwrap().len())
            .finish()
    }
}

impl<I: 'static> TransformationHandlers<I> {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a handler, replacing any previous handler for the same transformation.
    pub fn register(&self, handler: &AnyTransformationHandler) -> Result<(), ImageException> {
        let typed = handler.downcast::<I>()?;
        self.handlers
            .write()
            .unwrap()
            .insert(handler.transformation(), typed);
        Ok(())
    }

    /// The handler registered for the given transformation, if any.
    pub fn handler_for(
        &self,
        transformation: &dyn Transformation,
    ) -> Option<TransformationHandler<I>> {
        let id = (transformation as &dyn Any).type_id();
        self.handlers.read().unwrap().get(&id).cloned()
    }

    /// Determine if a handler is registered for transformations of type `T`.
    pub fn has<T: Transformation>(&self) -> bool {
        self.handlers
            .read()
            .unwrap()
            .contains_key(&TypeId::of::<T>())
    }

    /// The number of registered handlers.
    pub fn len(&self) -> usize {
        self.handlers.read().unwrap().len()
    }

    /// Determine if no handlers are registered.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq)]
    struct Pixelate {
        size: u32,
    }

    impl Transformation for Pixelate {}

    #[test]
    fn amounts_are_clamped() {
        assert_eq!(Blur::new(-5).amount, 0);
        assert_eq!(Blur::new(500).amount, 100);
        assert_eq!(Sharpen::new(-1).amount, 0);
        assert_eq!(Sharpen::new(101).amount, 100);
        assert_eq!(Sharpen::new(42).amount, 42);
    }

    #[test]
    fn transformations_can_be_downcast() {
        let transformation: Arc<dyn Transformation> = Arc::new(Pixelate { size: 12 });

        assert!(transformation.is::<Pixelate>());
        assert!(!transformation.is::<Blur>());
        assert_eq!(
            transformation.downcast_ref::<Pixelate>(),
            Some(&Pixelate { size: 12 })
        );
        assert!(transformation.name().ends_with("Pixelate"));
        assert_eq!(
            Cover::new(1, 1).name(),
            "Illuminate\\Image\\Transformations\\Cover"
        );
    }

    #[test]
    fn handlers_are_registered_by_transformation_type() {
        let handlers = TransformationHandlers::<Vec<u32>>::new();
        let handler = AnyTransformationHandler::new(|mut image: Vec<u32>, pixelate: &Pixelate| {
            image.push(pixelate.size);
            Ok(image)
        });
        assert!(handler.handles::<Pixelate>());
        assert!(handler.transformation_name().ends_with("Pixelate"));
        assert!(handler.image_type().contains("Vec<u32>"));

        assert!(handlers.is_empty());
        handlers.register(&handler).unwrap();
        assert!(handlers.has::<Pixelate>());
        assert_eq!(handlers.len(), 1);

        let pixelate = Pixelate { size: 12 };
        let found = handlers.handler_for(&pixelate).unwrap();
        assert_eq!(found.handle(vec![1], &pixelate).unwrap(), vec![1, 12]);
        assert!(handlers.handler_for(&Grayscale).is_none());
        assert!(found.handle(vec![], &Grayscale).is_err());
    }

    #[test]
    fn handlers_for_other_image_types_are_rejected() {
        let handlers = TransformationHandlers::<String>::new();
        let handler = AnyTransformationHandler::new(|image: Vec<u32>, _: &Pixelate| Ok(image));

        let error = handlers.register(&handler).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("transformation handler works with")
        );
        assert!(handlers.is_empty());
    }
}
