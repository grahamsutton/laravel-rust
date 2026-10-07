//! Parsing the background colors given to `contain` and `rotate`.

use ::image::Rgba;

use crate::exception::ImageException;

/// Parse a color: hex (`#fff`, `#ffffff`, `#ffffff80`, with or without the
/// `#`), `rgb(255, 255, 255)`, `rgba(255, 255, 255, 0.5)`, `transparent`,
/// or one of the basic CSS color names.
pub(crate) fn parse_color(input: &str) -> Result<Rgba<u8>, ImageException> {
    let invalid = || ImageException::new(format!("Unable to parse the color [{input}]."));
    let color = input.trim().to_ascii_lowercase();

    if let Some(named) = named_color(&color) {
        return Ok(named);
    }

    if let Some(arguments) = color
        .strip_prefix("rgba(")
        .or_else(|| color.strip_prefix("rgb("))
        .and_then(|rest| rest.strip_suffix(')'))
    {
        let parts: Vec<&str> = arguments.split(',').map(str::trim).collect();
        if parts.len() != 3 && parts.len() != 4 {
            return Err(invalid());
        }
        let mut channels = [0u8, 0, 0, 255];
        for (index, part) in parts.iter().take(3).enumerate() {
            channels[index] = part.parse::<u8>().map_err(|_| invalid())?;
        }
        if let Some(alpha) = parts.get(3) {
            let alpha: f64 = alpha.parse().map_err(|_| invalid())?;
            if !(0.0..=1.0).contains(&alpha) {
                return Err(invalid());
            }
            channels[3] = (alpha * 255.0).round() as u8;
        }
        return Ok(Rgba(channels));
    }

    let hex = color.strip_prefix('#').unwrap_or(&color);
    if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(invalid());
    }
    let digit = |index: usize| u8::from_str_radix(&hex[index..=index], 16).map(|d| d * 17);
    let pair = |index: usize| u8::from_str_radix(&hex[index..index + 2], 16);

    let channels = match hex.len() {
        3 => [digit(0), digit(1), digit(2), Ok(255)],
        4 => [digit(0), digit(1), digit(2), digit(3)],
        6 => [pair(0), pair(2), pair(4), Ok(255)],
        8 => [pair(0), pair(2), pair(4), pair(6)],
        _ => return Err(invalid()),
    };

    let mut rgba = [0u8; 4];
    for (slot, channel) in rgba.iter_mut().zip(channels) {
        *slot = channel.map_err(|_| invalid())?;
    }
    Ok(Rgba(rgba))
}

/// Format a color as a lowercase `#rrggbb` hex string.
pub(crate) fn to_hex(red: u8, green: u8, blue: u8) -> String {
    format!("#{red:02x}{green:02x}{blue:02x}")
}

fn named_color(name: &str) -> Option<Rgba<u8>> {
    let [r, g, b, a] = match name {
        "transparent" => [0, 0, 0, 0],
        "black" => [0, 0, 0, 255],
        "white" => [255, 255, 255, 255],
        "red" => [255, 0, 0, 255],
        "lime" => [0, 255, 0, 255],
        "green" => [0, 128, 0, 255],
        "blue" => [0, 0, 255, 255],
        "yellow" => [255, 255, 0, 255],
        "cyan" | "aqua" => [0, 255, 255, 255],
        "magenta" | "fuchsia" => [255, 0, 255, 255],
        "gray" | "grey" => [128, 128, 128, 255],
        "silver" => [192, 192, 192, 255],
        "maroon" => [128, 0, 0, 255],
        "olive" => [128, 128, 0, 255],
        "navy" => [0, 0, 128, 255],
        "purple" => [128, 0, 128, 255],
        "teal" => [0, 128, 128, 255],
        "orange" => [255, 165, 0, 255],
        _ => return None,
    };
    Some(Rgba([r, g, b, a]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_parses_hex_colors() {
        assert_eq!(parse_color("#ffffff").unwrap(), Rgba([255, 255, 255, 255]));
        assert_eq!(parse_color("ff0000").unwrap(), Rgba([255, 0, 0, 255]));
        assert_eq!(parse_color("#0080FF").unwrap(), Rgba([0, 128, 255, 255]));
        assert_eq!(parse_color("#fff").unwrap(), Rgba([255, 255, 255, 255]));
        assert_eq!(parse_color("#f008").unwrap(), Rgba([255, 0, 0, 136]));
        assert_eq!(parse_color("#00000080").unwrap(), Rgba([0, 0, 0, 128]));
    }

    #[test]
    fn it_parses_functional_and_named_colors() {
        assert_eq!(parse_color("rgb(1, 2, 3)").unwrap(), Rgba([1, 2, 3, 255]));
        assert_eq!(
            parse_color("rgba(1,2,3,0.5)").unwrap(),
            Rgba([1, 2, 3, 128])
        );
        assert_eq!(parse_color("transparent").unwrap(), Rgba([0, 0, 0, 0]));
        assert_eq!(parse_color("White").unwrap(), Rgba([255, 255, 255, 255]));
        assert_eq!(parse_color("orange").unwrap(), Rgba([255, 165, 0, 255]));
    }

    #[test]
    fn it_rejects_invalid_colors() {
        for color in [
            "",
            "#ff",
            "#ggg",
            "rgb(1,2)",
            "rgba(1,2,3,2)",
            "rgb(300,0,0)",
            "blurple",
            "#ffffffff00",
        ] {
            assert_eq!(
                parse_color(color).unwrap_err().to_string(),
                format!("Unable to parse the color [{color}].")
            );
        }
    }

    #[test]
    fn it_formats_hex_colors() {
        assert_eq!(to_hex(0, 128, 255), "#0080ff");
        assert_eq!(to_hex(255, 255, 255), "#ffffff");
    }
}
