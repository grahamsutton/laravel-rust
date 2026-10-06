//! Rendering tables, exactly like Symfony's table helper.
//!
//! ```
//! use illuminate_console::{Output, Table};
//!
//! let output = Output::buffered();
//!
//! Table::new()
//!     .headers(["ID", "Email"])
//!     .rows([["1", "taylor@example.com"], ["2", "abigail@example.com"]])
//!     .render(&output);
//!
//! assert_eq!(output.fetch(), "\
//! +----+---------------------+
//! | ID | Email               |
//! +----+---------------------+
//! | 1  | taylor@example.com  |
//! | 2  | abigail@example.com |
//! +----+---------------------+
//! ");
//! ```

use illuminate_support::Value;

use crate::formatter::OutputFormatter;
use crate::output::Output;

/// The characters and formats used to draw a table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableStyle {
    horizontal_outside: String,
    horizontal_inside: String,
    vertical_outside: String,
    vertical_inside: String,
    /// cross, top-left, top-mid, top-right, mid-right, bottom-right,
    /// bottom-mid, bottom-left, mid-left, top-left-bottom, top-mid-bottom,
    /// top-right-bottom
    crossings: [String; 12],
    cell_header_format: String,
    cell_row_content_format: String,
}

impl TableStyle {
    fn uniform(horizontal: &str, vertical: &str, crossing: &str) -> Self {
        Self {
            horizontal_outside: horizontal.into(),
            horizontal_inside: horizontal.into(),
            vertical_outside: vertical.into(),
            vertical_inside: vertical.into(),
            crossings: std::array::from_fn(|_| crossing.to_string()),
            cell_header_format: "<info>%s</info>".into(),
            cell_row_content_format: " %s ".into(),
        }
    }

    /// Resolve one of Symfony's named table styles: `default`, `borderless`,
    /// `compact`, `symfony-style-guide`, `box` and `box-double`.
    pub fn named(name: &str) -> Option<Self> {
        let style = match name {
            "default" => Self::uniform("-", "|", "+"),
            "borderless" => Self::uniform("=", " ", " "),
            "compact" => Self {
                cell_row_content_format: "%s".into(),
                ..Self::uniform("", " ", "")
            },
            "symfony-style-guide" => Self {
                cell_header_format: "%s".into(),
                ..Self::uniform("-", " ", " ")
            },
            "box" => {
                let mut style = Self::uniform("─", "│", "┼");
                style.crossings = ["┼", "┌", "┬", "┐", "┤", "┘", "┴", "└", "├", "├", "┼", "┤"].map(String::from);
                style
            }
            "box-double" => {
                let mut style = Self::uniform("═", "║", "┼");
                style.horizontal_inside = "─".into();
                style.vertical_inside = "│".into();
                style.crossings = ["┼", "╔", "╤", "╗", "╢", "╝", "╧", "╚", "╟", "╠", "╪", "╣"].map(String::from);
                style
            }
            _ => return None,
        };

        Some(style)
    }
}

impl Default for TableStyle {
    fn default() -> Self {
        Self::uniform("-", "|", "+")
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Separator {
    Top,
    TopBottom,
    Mid,
    Bottom,
}

/// A table of rows and columns.
#[derive(Clone, Debug, Default)]
pub struct Table {
    headers: Vec<String>,
    rows: Vec<Vec<String>>,
    style: TableStyle,
}

impl Table {
    /// Create an empty table.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the header cells.
    pub fn headers<I, S>(mut self, headers: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: ToString,
    {
        self.headers = headers.into_iter().map(|h| h.to_string()).collect();
        self
    }

    /// Set the rows.
    pub fn rows<R, C, S>(mut self, rows: R) -> Self
    where
        R: IntoIterator<Item = C>,
        C: IntoIterator<Item = S>,
        S: ToString,
    {
        self.rows = rows
            .into_iter()
            .map(|row| row.into_iter().map(|cell| cell.to_string()).collect())
            .collect();
        self
    }

    /// Set the rows from a [`Value`]: an array of arrays or of objects
    /// (like a Laravel collection's `toArray()`).
    pub fn rows_from_value(mut self, rows: &Value) -> Self {
        self.rows = value_rows(rows);
        self
    }

    /// Use one of the named styles (`default`, `compact`, `borderless`, `box`...).
    pub fn style(mut self, name: &str) -> Self {
        if let Some(style) = TableStyle::named(name) {
            self.style = style;
        }
        self
    }

    /// Use a custom style.
    pub fn with_style(mut self, style: TableStyle) -> Self {
        self.style = style;
        self
    }

    /// Render the table to the given output.
    pub fn render(&self, output: &Output) {
        for line in self.lines() {
            output.writeln(line);
        }
    }

    /// Render the table into a string (undecorated).
    pub fn render_to_string(&self) -> String {
        self.lines()
            .iter()
            .map(|line| format!("{}\n", OutputFormatter::format(line, false)))
            .collect()
    }

    /// The table's lines, still containing style tags.
    pub fn lines(&self) -> Vec<String> {
        let columns = self
            .rows
            .iter()
            .map(Vec::len)
            .chain(std::iter::once(self.headers.len()))
            .max()
            .unwrap_or(0);

        if columns == 0 {
            return Vec::new();
        }

        let padding = OutputFormatter::width(&self.style.cell_row_content_format.replace("%s", ""));
        let widths: Vec<usize> = (0..columns)
            .map(|column| {
                std::iter::once(&self.headers)
                    .chain(self.rows.iter())
                    .filter_map(|row| row.get(column))
                    .map(|cell| OutputFormatter::width(cell))
                    .max()
                    .unwrap_or(0)
                    + padding
            })
            .collect();

        let mut lines = Vec::new();
        let has_headers = !self.headers.is_empty();

        if has_headers {
            self.push_separator(&mut lines, Separator::Top, &widths);
            lines.push(self.render_row(&self.headers, &widths, &self.style.cell_header_format));
        }

        if !self.rows.is_empty() {
            self.push_separator(
                &mut lines,
                if has_headers { Separator::TopBottom } else { Separator::Top },
                &widths,
            );
            for row in &self.rows {
                lines.push(self.render_row(row, &widths, "%s"));
            }
        }

        self.push_separator(&mut lines, Separator::Bottom, &widths);

        lines
    }

    fn push_separator(&self, lines: &mut Vec<String>, kind: Separator, widths: &[usize]) {
        let style = &self.style;
        let c = &style.crossings;

        if style.horizontal_outside.is_empty() && style.horizontal_inside.is_empty() && c[0].is_empty() {
            return;
        }

        let (horizontal, left, mid, right) = match kind {
            Separator::Mid => (&style.horizontal_inside, &c[8], &c[0], &c[4]),
            Separator::Top => (&style.horizontal_outside, &c[1], &c[2], &c[3]),
            Separator::TopBottom => (&style.horizontal_outside, &c[9], &c[10], &c[11]),
            Separator::Bottom => (&style.horizontal_outside, &c[7], &c[6], &c[5]),
        };

        let mut markup = left.clone();
        for (index, width) in widths.iter().enumerate() {
            markup.push_str(&horizontal.repeat(*width));
            markup.push_str(if index == widths.len() - 1 { right } else { mid });
        }

        lines.push(markup);
    }

    fn render_row(&self, row: &[String], widths: &[usize], cell_format: &str) -> String {
        let mut content = self.style.vertical_outside.clone();

        for (index, width) in widths.iter().enumerate() {
            let cell = row.get(index).map(String::as_str).unwrap_or("");
            let cell = self.style.cell_row_content_format.replace("%s", cell);
            let pad = width.saturating_sub(OutputFormatter::width(&cell));
            let padded = format!("{cell}{}", " ".repeat(pad));
            content.push_str(&cell_format.replace("%s", &padded));
            content.push_str(if index == widths.len() - 1 {
                &self.style.vertical_outside
            } else {
                &self.style.vertical_inside
            });
        }

        content
    }
}

fn value_rows(rows: &Value) -> Vec<Vec<String>> {
    let cell = |value: &Value| match value {
        Value::String(value) => value.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    };

    match rows {
        Value::Array(rows) => rows
            .iter()
            .map(|row| match row {
                Value::Array(cells) => cells.iter().map(cell).collect(),
                Value::Object(map) => map.values().map(cell).collect(),
                other => vec![cell(other)],
            })
            .collect(),
        Value::Object(map) => map.values().map(|row| value_rows(&Value::Array(vec![row.clone()])).remove(0)).collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_renders_the_default_style() {
        let table = Table::new()
            .headers(["Name", "Email"])
            .rows([vec!["Taylor", "taylor@laravel.com"], vec!["Jess"]]);

        assert_eq!(
            table.render_to_string(),
            "+--------+--------------------+\n\
             | Name   | Email              |\n\
             +--------+--------------------+\n\
             | Taylor | taylor@laravel.com |\n\
             | Jess   |                    |\n\
             +--------+--------------------+\n"
        );
    }

    #[test]
    fn it_renders_headers_only() {
        let table = Table::new().headers(["A"]);
        assert_eq!(table.render_to_string(), "+---+\n| A |\n+---+\n");
    }

    #[test]
    fn it_renders_rows_without_headers() {
        let table = Table::new().rows([["a", "b"]]);
        assert_eq!(table.render_to_string(), "+---+---+\n| a | b |\n+---+---+\n");
    }

    #[test]
    fn it_renders_the_box_style() {
        let table = Table::new().headers(["A"]).rows([["b"]]).style("box");
        assert_eq!(table.render_to_string(), "┌───┐\n│ A │\n├───┤\n│ b │\n└───┘\n");
    }

    #[test]
    fn it_renders_the_compact_style() {
        let table = Table::new().headers(["ID", "Name"]).rows([["1", "Taylor"]]).style("compact");
        assert_eq!(table.render_to_string(), " ID Name   \n 1  Taylor \n");
    }

    #[test]
    fn it_renders_the_borderless_style() {
        let table = Table::new().headers(["A"]).rows([["b"]]).style("borderless");
        assert_eq!(table.render_to_string(), " === \n  A  \n === \n  b  \n === \n");
    }

    #[test]
    fn headers_are_green_when_decorated() {
        let output = Output::buffered().with_decoration(true);
        Table::new().headers(["A"]).render(&output);
        assert!(output.fetch().contains("\x1b[32m A \x1b[39m"));
    }

    #[test]
    fn it_accepts_value_rows() {
        let rows = illuminate_support::json!([
            {"id": 1, "email": "taylor@example.com"},
            [2, null],
        ]);
        let table = Table::new().headers(["ID", "Email"]).rows_from_value(&rows);
        assert!(table.render_to_string().contains("| 1  | taylor@example.com |"));
        assert!(table.render_to_string().contains("| 2  |                    |"));
    }
}
