//! Pluralization: choosing the right segment of `apple|apples`.

use std::cmp::Ordering;
use std::sync::LazyLock;

use regex::Regex;

use illuminate_support::{Value, json};

static CONDITION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)^[\{\[]([-?0-9|*,.]*)[\}\]](.*)").expect("valid regex"));

static STRIP_CONDITION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[\{\[][-?0-9|*,.]*[\}\]]").expect("valid regex"));

static NUMERIC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[ \t\n\r\v\f]*[+-]?([0-9]+(\.[0-9]*)?|\.[0-9]+)([eE][+-]?[0-9]+)?[ \t\n\r\v\f]*$")
        .expect("valid regex")
});

/// The "count" handed to `choice`: any number, or the length of a list.
///
/// ```
/// use illuminate_translation::ChoiceCount;
///
/// assert_eq!(ChoiceCount::from(3).as_f64(), 3.0);
/// assert_eq!(ChoiceCount::from(&vec!["a", "b"][..]).as_f64(), 2.0);
/// assert_eq!(ChoiceCount::from(1.5).to_string(), "1.5");
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ChoiceCount {
    /// A whole number.
    Int(i64),
    /// A fractional number.
    Float(f64),
}

impl ChoiceCount {
    /// The count as a float.
    pub fn as_f64(self) -> f64 {
        match self {
            ChoiceCount::Int(n) => n as f64,
            ChoiceCount::Float(n) => n,
        }
    }

    /// The count as a JSON value (used for the `:count` replacement).
    pub fn to_value(self) -> Value {
        match self {
            ChoiceCount::Int(n) => json!(n),
            ChoiceCount::Float(n) => json!(n),
        }
    }
}

impl std::fmt::Display for ChoiceCount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&php_number(self.as_f64()))
    }
}

macro_rules! choice_count_from_int {
    ($($ty:ty),*) => {
        $(impl From<$ty> for ChoiceCount {
            fn from(n: $ty) -> Self {
                ChoiceCount::Int(i64::try_from(n).unwrap_or(i64::MAX))
            }
        })*
    };
}

choice_count_from_int!(i8, i16, i32, i64, isize, u8, u16, u32, u64, usize);

impl From<f64> for ChoiceCount {
    fn from(n: f64) -> Self {
        ChoiceCount::Float(n)
    }
}

impl From<f32> for ChoiceCount {
    fn from(n: f32) -> Self {
        ChoiceCount::Float(n as f64)
    }
}

impl<T> From<&[T]> for ChoiceCount {
    fn from(items: &[T]) -> Self {
        ChoiceCount::from(items.len())
    }
}

impl<T> From<&Vec<T>> for ChoiceCount {
    fn from(items: &Vec<T>) -> Self {
        ChoiceCount::from(items.len())
    }
}

impl<T> From<&illuminate_support::Collection<T>> for ChoiceCount {
    fn from(items: &illuminate_support::Collection<T>) -> Self {
        ChoiceCount::from(items.count())
    }
}

/// Selects the proper translation string based on a number, using
/// Laravel's syntax:
///
/// - `apple|apples`, chosen by the locale's plural rules;
/// - `{0} none|[1,19] some|[20,*] many`, chosen by explicit conditions.
///
/// ```
/// use illuminate_translation::MessageSelector;
///
/// let selector = MessageSelector;
/// assert_eq!(selector.choose("apple|apples", 1.0, "en"), "apple");
/// assert_eq!(selector.choose("apple|apples", 2.0, "en"), "apples");
/// assert_eq!(selector.choose("{0} none|[1,19] some|[20,*] many", 25.0, "en"), "many");
/// ```
#[derive(Debug, Clone, Copy, Default)]
pub struct MessageSelector;

impl MessageSelector {
    /// Select a proper translation string based on the given number.
    pub fn choose(&self, line: &str, number: f64, locale: &str) -> String {
        let segments: Vec<&str> = line.split('|').collect();

        if let Some(value) = self.extract(&segments, number) {
            return php_trim(value).to_string();
        }

        let segments: Vec<String> = segments
            .iter()
            .map(|part| STRIP_CONDITION.replace(part, "").into_owned())
            .collect();

        let index = self.get_plural_index(locale, number);

        if segments.len() == 1 || index >= segments.len() {
            return segments[0].clone();
        }

        segments[index].clone()
    }

    fn extract<'a>(&self, segments: &[&'a str], number: f64) -> Option<&'a str> {
        segments
            .iter()
            .find_map(|part| self.extract_from_string(part, number))
    }

    fn extract_from_string<'a>(&self, part: &'a str, number: f64) -> Option<&'a str> {
        let captures = CONDITION.captures(part)?;
        let condition = captures.get(1)?.as_str();
        let value = captures.get(2)?.as_str();

        if let Some((from, to)) = condition.split_once(',') {
            let from_matches = php_compare(number, from) != Ordering::Less;
            let to_matches = php_compare(number, to) != Ordering::Greater;
            if (to == "*" && from_matches)
                || (from == "*" && to_matches)
                || (from_matches && to_matches)
            {
                return Some(value);
            }
        }

        (php_compare(number, condition) == Ordering::Equal).then_some(value)
    }

    /// Get the index to use for pluralization in the given locale.
    ///
    /// The plural rules are derived from code of the Zend Framework
    /// (2010-09-25), which is subject to the new BSD license.
    pub fn get_plural_index(&self, locale: &str, number: f64) -> usize {
        let n = number.abs();
        let i = n as i64;

        match locale {
            "az" | "az_AZ" | "bo" | "bo_CN" | "bo_IN" | "dz" | "dz_BT" | "id" | "id_ID" | "ja"
            | "ja_JP" | "jv" | "ka" | "ka_GE" | "km" | "km_KH" | "kn" | "kn_IN" | "ko"
            | "ko_KR" | "ms" | "ms_MY" | "th" | "th_TH" | "tr" | "tr_CY" | "tr_TR" | "vi"
            | "vi_VN" | "zh" | "zh_CN" | "zh_HK" | "zh_SG" | "zh_TW" => 0,

            "af" | "af_ZA" | "bn" | "bn_BD" | "bn_IN" | "bg" | "bg_BG" | "ca" | "ca_AD"
            | "ca_ES" | "ca_FR" | "ca_IT" | "da" | "da_DK" | "de" | "de_AT" | "de_BE" | "de_CH"
            | "de_DE" | "de_LI" | "de_LU" | "el" | "el_CY" | "el_GR" | "en" | "en_AG" | "en_AU"
            | "en_BW" | "en_CA" | "en_DK" | "en_GB" | "en_HK" | "en_IE" | "en_IN" | "en_NG"
            | "en_NZ" | "en_PH" | "en_SG" | "en_US" | "en_ZA" | "en_ZM" | "en_ZW" | "eo"
            | "eo_US" | "es" | "es_AR" | "es_BO" | "es_CL" | "es_CO" | "es_CR" | "es_CU"
            | "es_DO" | "es_EC" | "es_ES" | "es_GT" | "es_HN" | "es_MX" | "es_NI" | "es_PA"
            | "es_PE" | "es_PR" | "es_PY" | "es_SV" | "es_US" | "es_UY" | "es_VE" | "et"
            | "et_EE" | "eu" | "eu_ES" | "eu_FR" | "fa" | "fa_IR" | "fi" | "fi_FI" | "fo"
            | "fo_FO" | "fur" | "fur_IT" | "fy" | "fy_DE" | "fy_NL" | "gl" | "gl_ES" | "gu"
            | "gu_IN" | "ha" | "ha_NG" | "he" | "he_IL" | "hu" | "hu_HU" | "is" | "is_IS"
            | "it" | "it_CH" | "it_IT" | "ku" | "ku_TR" | "lb" | "lb_LU" | "ml" | "ml_IN"
            | "mn" | "mn_MN" | "mr" | "mr_IN" | "nah" | "nb" | "nb_NO" | "ne" | "ne_NP" | "nl"
            | "nl_AW" | "nl_BE" | "nl_NL" | "nn" | "nn_NO" | "no" | "om" | "om_ET" | "om_KE"
            | "or" | "or_IN" | "pa" | "pa_IN" | "pa_PK" | "pap" | "pap_AN" | "pap_AW"
            | "pap_CW" | "ps" | "ps_AF" | "pt" | "pt_BR" | "pt_PT" | "so" | "so_DJ" | "so_ET"
            | "so_KE" | "so_SO" | "sq" | "sq_AL" | "sq_MK" | "sv" | "sv_FI" | "sv_SE" | "sw"
            | "sw_KE" | "sw_TZ" | "ta" | "ta_IN" | "ta_LK" | "te" | "te_IN" | "tk" | "tk_TM"
            | "ur" | "ur_IN" | "ur_PK" | "zu" | "zu_ZA" => usize::from(n != 1.0),

            "am" | "am_ET" | "bh" | "fil" | "fil_PH" | "fr" | "fr_BE" | "fr_CA" | "fr_CH"
            | "fr_FR" | "fr_LU" | "gun" | "hi" | "hi_IN" | "hy" | "hy_AM" | "ln" | "ln_CD"
            | "mg" | "mg_MG" | "nso" | "nso_ZA" | "ti" | "ti_ER" | "ti_ET" | "wa" | "wa_BE"
            | "xbr" => usize::from(!(n == 0.0 || n == 1.0)),

            "be" | "be_BY" | "bs" | "bs_BA" | "hr" | "hr_HR" | "ru" | "ru_RU" | "ru_UA" | "sr"
            | "sr_ME" | "sr_RS" | "uk" | "uk_UA" => {
                if i % 10 == 1 && i % 100 != 11 {
                    0
                } else if (2..=4).contains(&(i % 10)) && (i % 100 < 10 || i % 100 >= 20) {
                    1
                } else {
                    2
                }
            }

            "cs" | "cs_CZ" | "sk" | "sk_SK" => {
                if n == 1.0 {
                    0
                } else if (2.0..=4.0).contains(&n) {
                    1
                } else {
                    2
                }
            }

            "ga" | "ga_IE" => {
                if n == 1.0 {
                    0
                } else if n == 2.0 {
                    1
                } else {
                    2
                }
            }

            "lt" | "lt_LT" => {
                if i % 10 == 1 && i % 100 != 11 {
                    0
                } else if i % 10 >= 2 && (i % 100 < 10 || i % 100 >= 20) {
                    1
                } else {
                    2
                }
            }

            "sl" | "sl_SI" => match i % 100 {
                1 => 0,
                2 => 1,
                3 | 4 => 2,
                _ => 3,
            },

            "mk" | "mk_MK" => usize::from(i % 10 != 1),

            "mt" | "mt_MT" => {
                if n == 1.0 {
                    0
                } else if n == 0.0 || (i % 100 > 1 && i % 100 < 11) {
                    1
                } else if i % 100 > 10 && i % 100 < 20 {
                    2
                } else {
                    3
                }
            }

            "lv" | "lv_LV" => {
                if n == 0.0 {
                    0
                } else if i % 10 == 1 && i % 100 != 11 {
                    1
                } else {
                    2
                }
            }

            "pl" | "pl_PL" => {
                if n == 1.0 {
                    0
                } else if (2..=4).contains(&(i % 10)) && (i % 100 < 12 || i % 100 > 14) {
                    1
                } else {
                    2
                }
            }

            "cy" | "cy_GB" => {
                if n == 1.0 {
                    0
                } else if n == 2.0 {
                    1
                } else if n == 8.0 || n == 11.0 {
                    2
                } else {
                    3
                }
            }

            "ro" | "ro_RO" => {
                if n == 1.0 {
                    0
                } else if n == 0.0 || (i % 100 > 0 && i % 100 < 20) {
                    1
                } else {
                    2
                }
            }

            "ar" | "ar_AE" | "ar_BH" | "ar_DZ" | "ar_EG" | "ar_IN" | "ar_IQ" | "ar_JO"
            | "ar_KW" | "ar_LB" | "ar_LY" | "ar_MA" | "ar_OM" | "ar_QA" | "ar_SA" | "ar_SD"
            | "ar_SS" | "ar_SY" | "ar_TN" | "ar_YE" => {
                if n == 0.0 {
                    0
                } else if n == 1.0 {
                    1
                } else if n == 2.0 {
                    2
                } else if (3..=10).contains(&(i % 100)) {
                    3
                } else if (11..=99).contains(&(i % 100)) {
                    4
                } else {
                    5
                }
            }

            _ => 0,
        }
    }
}

/// PHP's `trim` (which strips a narrower set than `str::trim`).
fn php_trim(value: &str) -> &str {
    value.trim_matches([' ', '\t', '\n', '\r', '\0', '\x0B'])
}

/// Print a number the way PHP casts it to a string.
pub(crate) fn php_number(number: f64) -> String {
    if number.fract() == 0.0 && number.abs() < 1e15 {
        format!("{}", number as i64)
    } else {
        format!("{number}")
    }
}

/// Compare a number with a string the way PHP 8 does: numerically when the
/// string is numeric, otherwise as strings.
fn php_compare(number: f64, value: &str) -> Ordering {
    if NUMERIC.is_match(value)
        && let Ok(parsed) = value.trim().parse::<f64>()
    {
        return number.partial_cmp(&parsed).unwrap_or(Ordering::Less);
    }
    php_number(number).as_str().cmp(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn choose(line: &str, number: f64) -> String {
        MessageSelector.choose(line, number, "en")
    }

    /// Laravel's own `MessageSelectorTest::chooseTestData`.
    #[test]
    fn it_matches_laravels_choose_test_data() {
        let cases: &[(&str, &str, f64)] = &[
            ("first", "first", 1.0),
            ("first", "first", 10.0),
            ("first", "first|second", 1.0),
            ("second", "first|second", 10.0),
            ("second", "first|second", 0.0),
            ("first", "{0}  first|{1}second", 0.0),
            ("first", "{1}first|{2}second", 1.0),
            ("second", "{1}first|{2}second", 2.0),
            ("first", "{2}first|{1}second", 2.0),
            ("second", "{9}first|{10}second", 0.0),
            ("first", "{9}first|{10}second", 1.0),
            ("", "{0}|{1}second", 0.0),
            ("", "{0}first|{1}", 1.0),
            ("first", "{1.3}first|{2.3}second", 1.3),
            ("second", "{1.3}first|{2.3}second", 2.3),
            ("first\nline", "{1}first\nline|{2}second", 1.0),
            ("first \n\nline", "{1}first \n\nline|{2}second", 1.0),
            ("first", "{0}  first|[1,9]second", 0.0),
            ("second", "{0}first|[1,9]  second", 1.0),
            ("second", "{0}first|[1,9]second", 10.0),
            ("first", "{0}first|[2,9]second", 1.0),
            ("second", "[4,*]first|[1,3]second", 1.0),
            ("first", "[4,*]first|[1,3]second", 100.0),
            ("second", "[1,5]first|[6,10]second", 7.0),
            ("first", "[*,4]first|[5,*]second", 1.0),
            ("second", "[5,*]first|[*,4]second", 1.0),
            ("second", "[5,*]first|[*,4]second", 0.0),
            ("first", "{0}first|[1,3]second|[4,*]third", 0.0),
            ("second", "{0}first|[1,3]second|[4,*]third", 1.0),
            ("third", "{0}first|[1,3]second|[4,*]third", 9.0),
            ("first", "first|second|third", 1.0),
            ("second", "first|second|third", 9.0),
            ("second", "first|second|third", 0.0),
            ("first", "{0}  first | { 1 } second", 0.0),
            ("first", "[4,*]first | [1,3]second", 100.0),
        ];
        for (expected, line, number) in cases {
            assert_eq!(
                choose(line, *number),
                *expected,
                "choose({line:?}, {number})"
            );
        }
    }

    #[test]
    fn conditions_are_stripped_when_falling_back_to_plural_rules() {
        assert_eq!(choose("{0}first|{1}second|third", 5.0), "second");
        assert_eq!(choose("{1} one|{2} two", 5.0), " two");
        assert_eq!(choose("{-1}negative|{0}zero", -1.0), "negative");
        assert_eq!(
            MessageSelector.choose("{0} none|[1,*] :count apples", 0.0, "ru"),
            "none"
        );
    }

    #[test]
    fn plural_rules_cover_many_locales() {
        let s = MessageSelector;
        // Languages without plural forms...
        assert_eq!(s.get_plural_index("ja", 5.0), 0);
        assert_eq!(s.get_plural_index("zh_TW", 1.0), 0);
        // One and other...
        assert_eq!(s.get_plural_index("de", 1.0), 0);
        assert_eq!(s.get_plural_index("de", 0.0), 1);
        assert_eq!(s.get_plural_index("en_GB", -1.0), 0);
        // French treats zero as singular...
        assert_eq!(s.get_plural_index("fr", 0.0), 0);
        assert_eq!(s.get_plural_index("fr", 1.0), 0);
        assert_eq!(s.get_plural_index("fr", 2.0), 1);
        // Slavic languages...
        assert_eq!(s.get_plural_index("ru", 1.0), 0);
        assert_eq!(s.get_plural_index("ru", 21.0), 0);
        assert_eq!(s.get_plural_index("ru", 11.0), 2);
        assert_eq!(s.get_plural_index("ru", 3.0), 1);
        assert_eq!(s.get_plural_index("ru", 24.0), 1);
        assert_eq!(s.get_plural_index("ru", 14.0), 2);
        assert_eq!(s.get_plural_index("ru", 5.0), 2);
        assert_eq!(s.get_plural_index("pl", 1.0), 0);
        assert_eq!(s.get_plural_index("pl", 22.0), 1);
        assert_eq!(s.get_plural_index("pl", 12.0), 2);
        assert_eq!(s.get_plural_index("pl", 25.0), 2);
        assert_eq!(s.get_plural_index("cs", 3.0), 1);
        assert_eq!(s.get_plural_index("cs", 5.0), 2);
        assert_eq!(s.get_plural_index("lt", 21.0), 0);
        assert_eq!(s.get_plural_index("lt", 12.0), 2);
        assert_eq!(s.get_plural_index("lt", 22.0), 1);
        assert_eq!(s.get_plural_index("sl", 101.0), 0);
        assert_eq!(s.get_plural_index("sl", 102.0), 1);
        assert_eq!(s.get_plural_index("sl", 104.0), 2);
        assert_eq!(s.get_plural_index("sl", 5.0), 3);
        assert_eq!(s.get_plural_index("mk", 11.0), 0);
        assert_eq!(s.get_plural_index("mk", 2.0), 1);
        // Others...
        assert_eq!(s.get_plural_index("ga", 2.0), 1);
        assert_eq!(s.get_plural_index("ga", 3.0), 2);
        assert_eq!(s.get_plural_index("mt", 0.0), 1);
        assert_eq!(s.get_plural_index("mt", 15.0), 2);
        assert_eq!(s.get_plural_index("mt", 20.0), 3);
        assert_eq!(s.get_plural_index("lv", 0.0), 0);
        assert_eq!(s.get_plural_index("lv", 21.0), 1);
        assert_eq!(s.get_plural_index("lv", 11.0), 2);
        assert_eq!(s.get_plural_index("cy", 8.0), 2);
        assert_eq!(s.get_plural_index("cy", 3.0), 3);
        assert_eq!(s.get_plural_index("ro", 0.0), 1);
        assert_eq!(s.get_plural_index("ro", 119.0), 1);
        assert_eq!(s.get_plural_index("ro", 20.0), 2);
        assert_eq!(s.get_plural_index("ar", 0.0), 0);
        assert_eq!(s.get_plural_index("ar", 1.0), 1);
        assert_eq!(s.get_plural_index("ar", 2.0), 2);
        assert_eq!(s.get_plural_index("ar", 105.0), 3);
        assert_eq!(s.get_plural_index("ar", 111.0), 4);
        assert_eq!(s.get_plural_index("ar", 100.0), 5);
        // Unknown locales (and hyphenated ones) always use the first form.
        assert_eq!(s.get_plural_index("xx", 5.0), 0);
        assert_eq!(s.get_plural_index("en-US", 5.0), 0);
    }

    #[test]
    fn php_comparisons() {
        assert_eq!(php_compare(5.0, "5"), Ordering::Equal);
        assert_eq!(php_compare(5.0, " 5"), Ordering::Equal);
        assert_eq!(php_compare(5.0, "10"), Ordering::Less);
        assert_eq!(php_compare(5.0, "*"), Ordering::Greater);
        assert_eq!(php_compare(5.0, ""), Ordering::Greater);
        assert_eq!(php_number(2.0), "2");
        assert_eq!(php_number(2.5), "2.5");
    }
}
