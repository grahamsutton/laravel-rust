//! Building `multipart/form-data` bodies — what a browser sends when a
//! form uploads files. The test client uses it to make upload requests.

use bytes::Bytes;
use indexmap::IndexMap;

use illuminate_support::{Str, Value};

use crate::uploaded_file::UploadedFile;

/// Generate a random multipart boundary.
pub fn boundary() -> String {
    format!("----LaravelFormBoundary{}", Str::random(16))
}

/// The `Content-Type` header for a multipart body with the given boundary.
pub fn content_type(boundary: &str) -> String {
    format!("multipart/form-data; boundary={boundary}")
}

/// Encode form fields (using PHP's bracket syntax for nested values) and
/// files as a `multipart/form-data` body.
///
/// ```
/// use illuminate_http::{multipart, UploadedFile};
/// use illuminate_support::json;
/// use indexmap::IndexMap;
///
/// let mut files = IndexMap::new();
/// files.insert("avatar".to_string(), vec![UploadedFile::fake().create_with_content("me.txt", "hi")]);
///
/// let body = multipart::encode(&json!({"user": {"name": "Taylor"}}), &files, "XYZ");
/// let body = String::from_utf8(body.to_vec()).unwrap();
///
/// assert!(body.contains("Content-Disposition: form-data; name=\"user[name]\"\r\n\r\nTaylor\r\n"));
/// assert!(body.contains("name=\"avatar\"; filename=\"me.txt\"\r\nContent-Type: text/plain\r\n\r\nhi\r\n"));
/// assert!(body.ends_with("--XYZ--\r\n"));
/// ```
pub fn encode(input: &Value, files: &IndexMap<String, Vec<UploadedFile>>, boundary: &str) -> Bytes {
    let mut fields = Vec::new();
    flatten(None, input, &mut fields);

    let mut body = Vec::new();
    for (name, value) in fields {
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(
            format!(
                "Content-Disposition: form-data; name=\"{}\"\r\n\r\n",
                escape(&name)
            )
            .as_bytes(),
        );
        body.extend_from_slice(value.as_bytes());
        body.extend_from_slice(b"\r\n");
    }
    for (name, list) in files {
        let name = if list.len() > 1 && !name.ends_with("[]") {
            format!("{name}[]")
        } else {
            name.clone()
        };
        for file in list {
            body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
            body.extend_from_slice(
                format!(
                    "Content-Disposition: form-data; name=\"{}\"; filename=\"{}\"\r\nContent-Type: {}\r\n\r\n",
                    escape(&name),
                    escape(file.client_original_name()),
                    file.client_mime_type(),
                )
                .as_bytes(),
            );
            body.extend_from_slice(file.bytes());
            body.extend_from_slice(b"\r\n");
        }
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    Bytes::from(body)
}

/// Flatten a value into `(name, value)` pairs: `user[name]`, `tags[0]`, ...
fn flatten(prefix: Option<&str>, value: &Value, fields: &mut Vec<(String, String)>) {
    let key = |name: &str| match prefix {
        Some(prefix) => format!("{prefix}[{name}]"),
        None => name.to_string(),
    };
    match value {
        Value::Object(map) => {
            for (name, value) in map {
                flatten(Some(&key(name)), value, fields);
            }
        }
        Value::Array(items) => {
            for (index, value) in items.iter().enumerate() {
                flatten(Some(&key(&index.to_string())), value, fields);
            }
        }
        Value::Null => {}
        Value::Bool(flag) => {
            if let Some(prefix) = prefix {
                fields.push((
                    prefix.to_string(),
                    if *flag { "1" } else { "0" }.to_string(),
                ));
            }
        }
        Value::Number(number) => {
            if let Some(prefix) = prefix {
                fields.push((prefix.to_string(), number.to_string()));
            }
        }
        Value::String(text) => {
            if let Some(prefix) = prefix {
                fields.push((prefix.to_string(), text.clone()));
            }
        }
    }
}

fn escape(value: &str) -> String {
    value
        .replace('"', "%22")
        .replace('\r', "%0D")
        .replace('\n', "%0A")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::parse_multipart;
    use illuminate_support::json;

    #[tokio::test]
    async fn encoded_bodies_parse_back_into_input_and_files() {
        let mut files = IndexMap::new();
        files.insert(
            "photos".to_string(),
            vec![
                UploadedFile::fake().create_with_content("a.txt", "A"),
                UploadedFile::fake().create_with_content("b.txt", "B"),
            ],
        );
        files.insert(
            "avatar".to_string(),
            vec![UploadedFile::fake().image("me.png", 2, 2)],
        );
        let input = json!({"name": "Taylor", "admin": true, "age": 30, "tags": ["php", "rust"], "skip": null});

        let boundary = boundary();
        let body = encode(&input, &files, &boundary);
        let (parsed, parsed_files) = parse_multipart(&content_type(&boundary), body).await;

        assert_eq!(
            parsed,
            json!({"name": "Taylor", "admin": "1", "age": "30", "tags": ["php", "rust"]})
        );
        assert_eq!(parsed_files["photos"].len(), 2);
        assert_eq!(parsed_files["photos"][1].get(), "B");
        assert_eq!(parsed_files["avatar"][0].mime_type(), "image/png");
        assert_eq!(parsed_files["avatar"][0].dimensions(), Some((2, 2)));
    }
}
