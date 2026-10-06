//! Number formatting helpers.

/// Static number helpers, mirroring `Illuminate\Support\Number`.
pub struct Number;

impl Number {
    /// Format a number with grouped thousands.
    ///
    /// ```
    /// use illuminate_support::Number;
    ///
    /// assert_eq!(Number::format(100000.0, None), "100,000");
    /// assert_eq!(Number::format(100000.5, Some(2)), "100,000.50");
    /// ```
    pub fn format(number: f64, precision: Option<usize>) -> String {
        let precision = precision.unwrap_or_else(|| {
            if number.fract() == 0.0 { 0 } else { 2.min(decimals(number)) }
        });
        let formatted = format!("{:.*}", precision, number.abs());
        let (int_part, frac_part) = match formatted.split_once('.') {
            Some((i, f)) => (i.to_string(), Some(f.to_string())),
            None => (formatted.clone(), None),
        };
        let mut grouped = String::new();
        for (i, c) in int_part.chars().enumerate() {
            if i > 0 && (int_part.len() - i) % 3 == 0 {
                grouped.push(',');
            }
            grouped.push(c);
        }
        let sign = if number < 0.0 { "-" } else { "" };
        match frac_part {
            Some(f) => format!("{sign}{grouped}.{f}"),
            None => format!("{sign}{grouped}"),
        }
    }

    /// Convert the given number to its percentage equivalent.
    pub fn percentage(number: f64, precision: usize) -> String {
        format!("{}%", Self::format(number, Some(precision)))
    }

    /// Convert the given number to its currency equivalent (USD by default).
    pub fn currency(number: f64, currency: &str) -> String {
        let symbol = match currency.to_uppercase().as_str() {
            "USD" | "" => "$",
            "EUR" => "€",
            "GBP" => "£",
            "JPY" => "¥",
            "INR" => "₹",
            other => return format!("{other} {}", Self::format(number, Some(2))),
        };
        let formatted = Self::format(number.abs(), Some(2));
        if number < 0.0 {
            format!("-{symbol}{formatted}")
        } else {
            format!("{symbol}{formatted}")
        }
    }

    /// Convert the given number to its file size equivalent.
    ///
    /// ```
    /// use illuminate_support::Number;
    ///
    /// assert_eq!(Number::file_size(1024.0, 0), "1 KB");
    /// assert_eq!(Number::file_size(1536.0, 1), "1.5 KB");
    /// ```
    pub fn file_size(bytes: f64, precision: usize) -> String {
        let units = ["B", "KB", "MB", "GB", "TB", "PB", "EB", "ZB", "YB"];
        let mut size = bytes;
        let mut unit = 0;
        while size.abs() >= 1024.0 && unit < units.len() - 1 {
            size /= 1024.0;
            unit += 1;
        }
        format!("{} {}", Self::format(size, Some(precision)), units[unit])
    }

    /// Convert the number to a human readable abbreviation (1K, 1.2M).
    pub fn abbreviate(number: f64, precision: usize) -> String {
        Self::summarize(number, precision, &[(3, "K"), (6, "M"), (9, "B"), (12, "T"), (15, "Q")], "")
    }

    /// Convert the number to a human readable string (1 thousand, 1.2 million).
    pub fn for_humans(number: f64, precision: usize) -> String {
        Self::summarize(
            number,
            precision,
            &[(3, "thousand"), (6, "million"), (9, "billion"), (12, "trillion"), (15, "quadrillion")],
            " ",
        )
    }

    fn summarize(number: f64, precision: usize, units: &[(i32, &str)], separator: &str) -> String {
        let abs = number.abs();
        for (exponent, suffix) in units.iter().rev() {
            let threshold = 10f64.powi(*exponent);
            if abs >= threshold {
                let value = number / threshold;
                let formatted = format!("{:.*}", precision, value);
                let formatted = if formatted.contains('.') {
                    formatted.trim_end_matches('0').trim_end_matches('.').to_string()
                } else {
                    formatted
                };
                return format!("{formatted}{separator}{suffix}");
            }
        }
        let formatted = format!("{:.*}", precision, number);
        if formatted.contains('.') {
            formatted.trim_end_matches('0').trim_end_matches('.').to_string()
        } else {
            formatted
        }
    }

    /// Convert the number to its ordinal form (1st, 2nd, 3rd).
    pub fn ordinal(number: i64) -> String {
        let suffix = match (number.abs() % 10, number.abs() % 100) {
            (_, 11..=13) => "th",
            (1, _) => "st",
            (2, _) => "nd",
            (3, _) => "rd",
            _ => "th",
        };
        format!("{number}{suffix}")
    }

    /// Clamp the number between a minimum and maximum.
    pub fn clamp(number: f64, min: f64, max: f64) -> f64 {
        number.max(min).min(max)
    }
}

fn decimals(number: f64) -> usize {
    let s = format!("{number}");
    s.split_once('.').map(|(_, f)| f.len()).unwrap_or(0)
}
