//! The `$message` variable available in mail views, used to embed images:
//! `<img src="{{ $message->embed($pathToImage) }}">`.

use std::sync::{Arc, Mutex};

use illuminate_support::{Result, Value};
use illuminate_view::{ViewObject, ViewValue};

use crate::attachment::{MessageAttachment, guess_mime};
use crate::mime::random_hex;

/// Collects the files embedded while a message's views render.
#[derive(Clone, Default)]
pub(crate) struct Embeds(Arc<Mutex<Vec<MessageAttachment>>>);

impl Embeds {
    /// The view object for the HTML views.
    pub(crate) fn html_object(&self) -> ViewValue {
        ViewValue::object(ViewMessage {
            embeds: self.clone(),
            text: false,
        })
    }

    /// The view object for plain-text views (embedding outputs nothing).
    pub(crate) fn text_object(&self) -> ViewValue {
        ViewValue::object(ViewMessage {
            embeds: self.clone(),
            text: true,
        })
    }

    /// Take everything embedded so far.
    pub(crate) fn take(&self) -> Vec<MessageAttachment> {
        std::mem::take(&mut *self.0.lock().unwrap())
    }

    fn push(&self, data: Vec<u8>, name: String, content_type: Option<String>) -> String {
        let id = format!("{}@laravel", random_hex(16));
        self.0.lock().unwrap().push(MessageAttachment {
            content_type: content_type.unwrap_or_else(|| guess_mime(&name)),
            filename: name,
            data,
            content_id: Some(id.clone()),
        });
        format!("cid:{id}")
    }
}

/// Laravel's `Message` (and `TextMessage`) as seen from a view.
struct ViewMessage {
    embeds: Embeds,
    text: bool,
}

impl ViewObject for ViewMessage {
    fn class_name(&self) -> &str {
        if self.text {
            "Illuminate\\Mail\\TextMessage"
        } else {
            "Illuminate\\Mail\\Message"
        }
    }

    fn call(&self, method: &str, args: &[ViewValue]) -> Option<Result<ViewValue>> {
        let arg = |index: usize| args.get(index).map(ViewValue::to_string_lossy);
        match method {
            "embed" => Some((|| {
                if self.text {
                    return Ok(ViewValue::from(""));
                }
                let path = arg(0).unwrap_or_default();
                let data = std::fs::read(&path).map_err(|e| {
                    illuminate_support::error::RuntimeException::new(format!(
                        "Unable to embed file \"{path}\": {e}"
                    ))
                })?;
                let name = std::path::Path::new(&path)
                    .file_name()
                    .map_or_else(|| path.clone(), |n| n.to_string_lossy().into_owned());
                Ok(ViewValue::from(self.embeds.push(data, name, None)))
            })()),
            "embedData" => Some(Ok(if self.text {
                ViewValue::from("")
            } else {
                let data = arg(0).unwrap_or_default().into_bytes();
                let name = arg(1).unwrap_or_default();
                ViewValue::from(
                    self.embeds
                        .push(data, name, arg(2).filter(|t| !t.is_empty())),
                )
            })),
            _ => None,
        }
    }

    fn to_json(&self) -> Value {
        Value::Null
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use illuminate_view::{Factory, data};

    #[test]
    fn views_can_embed_files_and_data() {
        let dir = tempfile::tempdir().unwrap();
        let logo = dir.path().join("logo.png");
        std::fs::write(&logo, [1, 2, 3]).unwrap();

        let embeds = Embeds::default();
        let factory = Factory::new(Vec::<String>::new());
        let html = factory
            .render_inline(
                "<img src=\"{{ $message->embed($path) }}\"><img src=\"{{ $message->embedData('<svg/>', 'a.svg', 'image/svg+xml') }}\">",
                data([
                    ("message", embeds.html_object()),
                    ("path", ViewValue::from(logo.display().to_string())),
                ]),
            )
            .unwrap();
        assert_eq!(html.matches("src=\"cid:").count(), 2);
        let embedded = embeds.take();
        assert_eq!(embedded.len(), 2);
        assert_eq!(embedded[0].filename, "logo.png");
        assert_eq!(embedded[0].content_type, "image/png");
        assert_eq!(embedded[1].content_type, "image/svg+xml");

        let text = factory
            .render_inline(
                "[{{ $message->embed('/missing') }}]",
                data([("message", embeds.text_object())]),
            )
            .unwrap();
        assert_eq!(text, "[]");
        assert!(embeds.take().is_empty());

        let error = factory
            .render_inline(
                "{{ $message->embed('/missing/file.png') }}",
                data([("message", embeds.html_object())]),
            )
            .unwrap_err();
        assert!(error.to_string().contains("Unable to embed file"));
    }
}
