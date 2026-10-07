//! Streamed responses: Server-Sent Events and streamed JSON.

use bytes::Bytes;
use futures::stream::{self, BoxStream, Stream, StreamExt};
use serde::Serialize;

use illuminate_support::{Error, Js, Map, Value, to_value};

/// An event sent by an [event stream](crate::Response::event_stream) —
/// Laravel's `StreamedEvent`.
///
/// Plain messages are sent as `update` events; use a `StreamedEvent` to
/// name the event yourself.
///
/// ```
/// use illuminate_http::StreamedEvent;
/// use illuminate_support::json;
///
/// let event = StreamedEvent::new("price", json!({"symbol": "LARA", "price": 42}));
/// assert_eq!(event.render(), "event: price\ndata: {\"symbol\":\"LARA\",\"price\":42}\n\n");
///
/// let update = StreamedEvent::from("Hello");
/// assert_eq!(update.render(), "event: update\ndata: Hello\n\n");
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct StreamedEvent {
    /// The name of the event (`update` by default).
    pub event: String,
    /// The event's data: strings and numbers are sent as-is, anything
    /// else is JSON encoded.
    pub data: Value,
}

impl StreamedEvent {
    /// Create a named event.
    pub fn new(event: impl Into<String>, data: impl Serialize) -> Self {
        Self {
            event: event.into(),
            data: to_value(&data),
        }
    }

    /// Render the event in the `text/event-stream` format.
    pub fn render(&self) -> String {
        let message = match &self.data {
            Value::String(text) => text.clone(),
            Value::Number(number) => number.to_string(),
            other => Js::encode(other),
        };
        format!("event: {}\ndata: {}\n\n", self.event, message)
    }

    fn is_filled(&self) -> bool {
        match &self.data {
            Value::Null => false,
            Value::String(text) => !text.trim().is_empty(),
            _ => true,
        }
    }
}

impl From<&str> for StreamedEvent {
    fn from(data: &str) -> Self {
        Self::new("update", data)
    }
}

impl From<String> for StreamedEvent {
    fn from(data: String) -> Self {
        Self::new("update", data)
    }
}

impl From<Value> for StreamedEvent {
    fn from(data: Value) -> Self {
        Self {
            event: "update".to_string(),
            data,
        }
    }
}

/// Render a stream of events, followed by the closing event (if any).
pub(crate) fn event_stream_body<S, T>(
    events: S,
    end_stream_with: Option<StreamedEvent>,
) -> impl Stream<Item = Result<Bytes, Error>> + Send + 'static
where
    S: Stream<Item = T> + Send + 'static,
    T: Into<StreamedEvent>,
{
    let end = end_stream_with.filter(StreamedEvent::is_filled);
    events
        .map(|event| Ok(Bytes::from(event.into().render())))
        .chain(stream::iter(end.map(|end| Ok(Bytes::from(end.render())))))
}

/// JSON data streamed to the client piece by piece — Laravel's
/// `response()->streamJson()`.
///
/// Fixed data is sent as-is, while the streams you add are sent as JSON
/// arrays item by item, so large result sets never need to be held in
/// memory at once.
///
/// ```
/// use illuminate_http::{Response, StreamedJson};
/// use illuminate_support::json;
///
/// # tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
/// let users = futures::stream::iter(vec![json!({"id": 1}), json!({"id": 2})]);
///
/// let response = Response::stream_json(
///     StreamedJson::new(json!({"meta": {"page": 1}})).with_stream("users", users),
/// );
///
/// assert_eq!(response.header("content-type").unwrap(), "application/json");
/// assert_eq!(
///     response.into_content_string().await.unwrap(),
///     r#"{"meta":{"page":1},"users":[{"id":1},{"id":2}]}"#
/// );
/// # });
/// ```
pub struct StreamedJson {
    data: Value,
    streams: Vec<(String, BoxStream<'static, Value>)>,
}

impl std::fmt::Debug for StreamedJson {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamedJson")
            .field("data", &self.data)
            .field(
                "streams",
                &self.streams.iter().map(|(key, _)| key).collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl StreamedJson {
    /// Stream the given (fixed) data.
    pub fn new(data: impl Serialize) -> Self {
        Self {
            data: to_value(&data),
            streams: Vec::new(),
        }
    }

    /// Stream a top-level JSON array, item by item.
    pub fn items<S, T>(items: S) -> Self
    where
        S: Stream<Item = T> + Send + 'static,
        T: Serialize,
    {
        Self {
            data: Value::String(placeholder(0)),
            streams: vec![(String::new(), items.map(|item| to_value(&item)).boxed())],
        }
    }

    /// Stream the items of the given stream as the JSON array at `key`
    /// ("dot" notation reaches into nested objects).
    pub fn with_stream<S, T>(mut self, key: &str, items: S) -> Self
    where
        S: Stream<Item = T> + Send + 'static,
        T: Serialize,
    {
        let index = self.streams.len();
        set_dotted(&mut self.data, key, Value::String(placeholder(index)));
        self.streams
            .push((key.to_string(), items.map(|item| to_value(&item)).boxed()));
        self
    }

    /// Turn the data into a stream of body chunks.
    pub(crate) fn into_body(self) -> impl Stream<Item = Result<Bytes, Error>> + Send + 'static {
        let json = serde_json::to_string(&self.data).unwrap_or_else(|_| "null".into());
        let mut streams: Vec<Option<BoxStream<'static, Value>>> = self
            .streams
            .into_iter()
            .map(|(_, items)| Some(items))
            .collect();

        // Find where each placeholder landed in the encoded data.
        let mut positions: Vec<(usize, usize, usize)> = Vec::new();
        for index in 0..streams.len() {
            let needle = format!("\"{}\"", placeholder(index));
            if let Some(start) = json.find(&needle) {
                positions.push((start, start + needle.len(), index));
            }
        }
        positions.sort();

        let mut segments: Vec<BoxStream<'static, Result<Bytes, Error>>> = Vec::new();
        let mut cursor = 0;
        for (start, end, index) in positions {
            segments.push(text(json[cursor..start].to_string()));
            if let Some(items) = streams[index].take() {
                segments.push(array(items));
            }
            cursor = end;
        }
        segments.push(text(json[cursor..].to_string()));
        stream::iter(segments).flatten()
    }
}

impl From<Value> for StreamedJson {
    fn from(data: Value) -> Self {
        Self {
            data,
            streams: Vec::new(),
        }
    }
}

fn placeholder(index: usize) -> String {
    format!("__laravel_streamed_json_{index}__")
}

fn text(text: String) -> BoxStream<'static, Result<Bytes, Error>> {
    stream::iter((!text.is_empty()).then(|| Ok(Bytes::from(text)))).boxed()
}

fn array(items: BoxStream<'static, Value>) -> BoxStream<'static, Result<Bytes, Error>> {
    let items = items.enumerate().map(|(index, item)| {
        let json = serde_json::to_string(&item).unwrap_or_else(|_| "null".into());
        Ok(Bytes::from(if index == 0 {
            json
        } else {
            format!(",{json}")
        }))
    });
    stream::once(async { Ok(Bytes::from_static(b"[")) })
        .chain(items)
        .chain(stream::once(async { Ok(Bytes::from_static(b"]")) }))
        .boxed()
}

/// Set a value using "dot" notation, creating objects along the way.
fn set_dotted(target: &mut Value, key: &str, value: Value) {
    let mut current = target;
    let segments: Vec<&str> = key.split('.').collect();
    for (position, segment) in segments.iter().enumerate() {
        if !current.is_object() {
            *current = Value::Object(Map::new());
        }
        let Value::Object(map) = current else {
            unreachable!()
        };
        if position == segments.len() - 1 {
            map.insert(segment.to_string(), value);
            return;
        }
        current = map
            .entry(segment.to_string())
            .or_insert_with(|| Value::Object(Map::new()));
    }
}
