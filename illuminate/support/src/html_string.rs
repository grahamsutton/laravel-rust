use std::fmt;

use serde::{Deserialize, Serialize};

/// A string of HTML that should not be escaped when rendered by Blade.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct HtmlString(pub String);

impl HtmlString {
    pub fn new(html: impl Into<String>) -> Self {
        Self(html.into())
    }

    /// Get the HTML string.
    pub fn to_html(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn is_not_empty(&self) -> bool {
        !self.0.is_empty()
    }
}

impl fmt::Display for HtmlString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for HtmlString {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for HtmlString {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

/// Encode HTML special characters in a string, exactly like PHP's
/// `htmlspecialchars($value, ENT_QUOTES, 'UTF-8', true)`.
///
/// ```
/// use illuminate_support::e;
///
/// assert_eq!(e("<b>\"Taylor\" & 'Otwell'</b>"), "&lt;b&gt;&quot;Taylor&quot; &amp; &#039;Otwell&#039;&lt;/b&gt;");
/// ```
pub fn e(value: impl AsRef<str>) -> String {
    let value = value.as_ref();
    let mut out = String::with_capacity(value.len() + value.len() / 8);
    for c in value.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#039;"),
            other => out.push(other),
        }
    }
    out
}
