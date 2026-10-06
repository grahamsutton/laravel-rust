//! English pluralization and singularization.
//!
//! This is a faithful port of the Doctrine Inflector English ruleset that
//! Laravel's `Pluralizer` is built on: uninflected words are checked first,
//! then irregular words (whole-word, case-insensitive), then the regular
//! transformation rules in order. The result's case is matched to the input
//! exactly like Laravel's `Pluralizer::matchCase`.

use std::collections::HashMap;
use std::sync::{LazyLock, RwLock};

use regex::Regex;

/// Laravel's own list of words that should never be pluralized.
static UNCOUNTABLE: LazyLock<RwLock<Vec<String>>> =
    LazyLock::new(|| RwLock::new(vec!["recommended".to_string(), "related".to_string()]));

/// Words that are the same in singular and plural form (Doctrine's defaults).
const UNINFLECTED_DEFAULT: &[&str] = &[
    r"\w+media", "advice", "aircraft", "amoyese", "art", "audio", "baggage", "bison", "borghese",
    "bream", "breeches", "britches", "buffalo", "butter", "cantus", "carp", "cattle", "chassis",
    "clippers", "clothing", "coal", "cod", "coitus", "compensation", "congoese", "contretemps",
    "coreopsis", "corps", "cotton", "data", "debris", "deer", "diabetes", "djinn", "education",
    "eland", "elk", "emoji", "equipment", "evidence", "faroese", "feedback", "fish", "flounder",
    "flour", "foochowese", "food", "furniture", "gallows", "genevese", "genoese", "gilbertese",
    "gold", "headquarters", "herpes", "hijinks", "homework", "hottentotese", "impatience",
    "information", "innings", "jackanapes", "jedi", "kin", "kiplingese", "knowledge", "kongoese",
    "leather", "love", "lucchese", "luggage", "mackerel", "maltese", "management", "metadata",
    "mews", "money", "moose", "mumps", "music", "nankingese", "news", "nexus", "niasese",
    "nutrition", "offspring", "oil", "patience", "pekingese", "piedmontese", "pincers",
    "pistoiese", "plankton", "pliers", "pokemon", "police", "polish", "portuguese",
    "proceedings", "rabies", "rain", "rhinoceros", "rice", "salmon", "sand", "sarawakese",
    "scissors", "sea[- ]bass", "series", "shavese", "shears", "sheep", "siemens", "silk", "sms",
    "soap", "social media", "spam", "species", "staff", "sugar", "swine", "talent", "toothpaste",
    "traffic", "travel", "trousers", "trout", "tuna", "us", "vermontese", "vinegar", "weather",
    "wenchowese", "wheat", "whiting", "wildebeest", "wood", "wool", "yengeese",
];

/// Additional words that are never pluralized.
const UNINFLECTED_PLURAL: &[&str] = &["people", "trivia", r"\w+ware$", "media"];

/// Additional words that are never singularized.
const UNINFLECTED_SINGULAR: &[&str] = &[
    ".*ss", "clothes", "data", "fascia", "fuchsia", "galleria", "mafia", "militia", "pants",
    "petunia", "sepia", "trivia", "utopia",
];

/// Irregular singular => plural pairs.
const IRREGULAR: &[(&str, &str)] = &[
    ("atlas", "atlases"),
    ("axis", "axes"),
    ("axe", "axes"),
    ("beef", "beefs"),
    ("blouse", "blouses"),
    ("brother", "brothers"),
    ("cafe", "cafes"),
    ("cave", "caves"),
    ("chateau", "chateaux"),
    ("niveau", "niveaux"),
    ("child", "children"),
    ("canvas", "canvases"),
    ("cookie", "cookies"),
    ("brownie", "brownies"),
    ("corpus", "corpuses"),
    ("cow", "cows"),
    ("criterion", "criteria"),
    ("curriculum", "curricula"),
    ("demo", "demos"),
    ("domino", "dominoes"),
    ("echo", "echoes"),
    ("epoch", "epochs"),
    ("foot", "feet"),
    ("fungus", "fungi"),
    ("ganglion", "ganglions"),
    ("gas", "gases"),
    ("genie", "genies"),
    ("genus", "genera"),
    ("goose", "geese"),
    ("graffito", "graffiti"),
    ("hippopotamus", "hippopotami"),
    ("hoof", "hoofs"),
    ("human", "humans"),
    ("iris", "irises"),
    ("larva", "larvae"),
    ("leaf", "leaves"),
    ("lens", "lenses"),
    ("loaf", "loaves"),
    ("man", "men"),
    ("medium", "media"),
    ("memorandum", "memoranda"),
    ("money", "monies"),
    ("mongoose", "mongooses"),
    ("motto", "mottoes"),
    ("move", "moves"),
    ("mythos", "mythoi"),
    ("niche", "niches"),
    ("nucleus", "nuclei"),
    ("numen", "numina"),
    ("occiput", "occiputs"),
    ("octopus", "octopuses"),
    ("opus", "opuses"),
    ("ox", "oxen"),
    ("passerby", "passersby"),
    ("penis", "penises"),
    ("person", "people"),
    ("plateau", "plateaux"),
    ("runner-up", "runners-up"),
    ("safe", "safes"),
    ("sex", "sexes"),
    ("sieve", "sieves"),
    ("soliloquy", "soliloquies"),
    ("son-in-law", "sons-in-law"),
    ("syllabus", "syllabi"),
    ("testis", "testes"),
    ("thief", "thieves"),
    ("tooth", "teeth"),
    ("tornado", "tornadoes"),
    ("trilby", "trilbys"),
    ("turf", "turfs"),
    ("valve", "valves"),
    ("wave", "waves"),
    ("zombie", "zombies"),
];

const PLURAL_RULES: &[(&str, &str)] = &[
    (r"(s)tatus$", r"\1\2tatuses"),
    (r"(quiz)$", r"\1zes"),
    (r"^(ox)$", r"\1\2en"),
    (r"([m|l])ouse$", r"\1ice"),
    (r"(matr|vert|ind)(ix|ex)$", r"\1ices"),
    (r"(x|ch|ss|sh)$", r"\1es"),
    (r"([^aeiouy]|qu)y$", r"\1ies"),
    (r"(hive|gulf)$", r"\1s"),
    (r"(?:([^f])fe|([lr])f)$", r"\1\2ves"),
    (r"sis$", "ses"),
    (r"([ti])um$", r"\1a"),
    (r"(tax)on$", r"\1a"),
    (r"(c)riterion$", r"\1riteria"),
    (r"(p)erson$", r"\1eople"),
    (r"(m)an$", r"\1en"),
    (r"(c)hild$", r"\1hildren"),
    (r"(f)oot$", r"\1eet"),
    (r"(buffal|her|potat|tomat|volcan)o$", r"\1\2oes"),
    (r"(alumn|bacill|cact|foc|fung|nucle|radi|stimul|syllab|termin|vir)us$", r"\1i"),
    (r"us$", "uses"),
    (r"(alias)$", r"\1es"),
    (r"(analys|ax|cris|test|thes)is$", r"\1es"),
    (r"s$", "s"),
    (r"^$", ""),
    (r"$", "s"),
];

const SINGULAR_RULES: &[(&str, &str)] = &[
    (r"(s)tatuses$", r"\1\2tatus"),
    (r"(s)tatus$", r"\1\2tatus"),
    (r"(c)ampus$", r"\1\2ampus"),
    (r"^(.*)(menu)s$", r"\1\2"),
    (r"(quiz)zes$", r"\1"),
    (r"(matr)ices$", r"\1ix"),
    (r"(vert|ind)ices$", r"\1ex"),
    (r"^(ox)en", r"\1"),
    (r"(alias)(es)*$", r"\1"),
    (r"(buffal|her|potat|tomat|volcan)oes$", r"\1o"),
    (r"(alumn|bacill|cact|foc|fung|nucle|radi|stimul|syllab|termin|viri?)i$", r"\1us"),
    (r"([ftw]ax)es", r"\1"),
    (r"(analys|ax|cris|test|thes)es$", r"\1is"),
    (r"(shoe|slave)s$", r"\1"),
    (r"(o)es$", r"\1"),
    (r"ouses$", "ouse"),
    (r"([^a])uses$", r"\1us"),
    (r"([m|l])ice$", r"\1ouse"),
    (r"(x|ch|ss|sh)es$", r"\1"),
    (r"(m)ovies$", r"\1\2ovie"),
    (r"(s)eries$", r"\1\2eries"),
    (r"([^aeiouy]|qu)ies$", r"\1y"),
    (r"([lr])ves$", r"\1f"),
    (r"(tive)s$", r"\1"),
    (r"(hive)s$", r"\1"),
    (r"(drive)s$", r"\1"),
    (r"(dive)s$", r"\1"),
    (r"(olive)s$", r"\1"),
    (r"([^fo])ves$", r"\1fe"),
    (r"(^analy)ses$", r"\1sis"),
    (r"(analy|diagno|^ba|(p)arenthe|(p)rogno|(s)ynop|(t)he)ses$", r"\1\2sis"),
    (r"(tax)a$", r"\1on"),
    (r"(c)riteria$", r"\1riterion"),
    // Doctrine's rule is `([ti])a(?<!regatta)$`; the look-behind is handled in code.
    (r"([ti])a$", r"\1um"),
    (r"(p)eople$", r"\1\2erson"),
    (r"(m)en$", r"\1an"),
    (r"(c)hildren$", r"\1\2hild"),
    (r"(f)eet$", r"\1oot"),
    (r"(n)ews$", r"\1\2ews"),
    (r"eaus$", "eau"),
    (r"^tights$", "tights"),
    (r"^shorts$", "shorts"),
    (r"s$", ""),
];

struct Rule {
    pattern: Regex,
    replacement: String,
    source: &'static str,
}

struct Ruleset {
    uninflected: Regex,
    irregular: HashMap<&'static str, &'static str>,
    rules: Vec<Rule>,
}

impl Ruleset {
    fn new(
        uninflected: &[&[&str]],
        irregular: HashMap<&'static str, &'static str>,
        rules: &[(&'static str, &'static str)],
    ) -> Self {
        let patterns: Vec<&str> = uninflected.iter().flat_map(|l| l.iter().copied()).collect();
        let uninflected = Regex::new(&format!("(?i)^(?:{})$", patterns.join("|")))
            .expect("valid uninflected patterns");
        let rules = rules
            .iter()
            .map(|(pattern, replacement)| Rule {
                pattern: Regex::new(&format!("(?i){pattern}")).expect("valid inflection rule"),
                replacement: crate::preg::translate_replacement(replacement),
                source: pattern,
            })
            .collect();
        Self {
            uninflected,
            irregular,
            rules,
        }
    }

    fn inflect(&self, word: &str) -> String {
        if word.is_empty() {
            return String::new();
        }
        if self.uninflected.is_match(word) {
            return word.to_string();
        }
        let lower = word.to_lowercase();
        if let Some(to) = self.irregular.get(lower.as_str()) {
            let first_upper = lower.chars().next() != word.chars().next();
            return if first_upper { ucfirst_ascii(to) } else { (*to).to_string() };
        }
        for rule in &self.rules {
            if !rule.pattern.is_match(word) {
                continue;
            }
            if rule.source == r"([ti])a$" && lower.ends_with("regatta") {
                continue;
            }
            return rule
                .pattern
                .replace_all(word, rule.replacement.as_str())
                .into_owned();
        }
        word.to_string()
    }
}

static PLURAL: LazyLock<Ruleset> = LazyLock::new(|| {
    Ruleset::new(
        &[UNINFLECTED_DEFAULT, UNINFLECTED_PLURAL],
        IRREGULAR.iter().copied().collect(),
        PLURAL_RULES,
    )
});

static SINGULAR: LazyLock<Ruleset> = LazyLock::new(|| {
    Ruleset::new(
        &[UNINFLECTED_DEFAULT, UNINFLECTED_SINGULAR],
        IRREGULAR.iter().map(|(singular, plural)| (*plural, *singular)).collect(),
        SINGULAR_RULES,
    )
});

/// The pluralizer.
pub struct Pluralizer;

impl Pluralizer {
    /// Get the plural form of an English word.
    ///
    /// ```
    /// use illuminate_support::Pluralizer;
    ///
    /// assert_eq!(Pluralizer::plural("user", 2), "users");
    /// assert_eq!(Pluralizer::plural("child", 2), "children");
    /// assert_eq!(Pluralizer::plural("Person", 2), "People");
    /// assert_eq!(Pluralizer::plural("category", 1), "category");
    /// ```
    pub fn plural(value: &str, count: i64) -> String {
        let ends_with_word_character = value.chars().last().is_some_and(|c| {
            c.is_ascii_alphanumeric() || ('\u{0080}'..='\u{FFFF}').contains(&c)
        });
        if count.unsigned_abs() == 1 || Self::uncountable(value) || !ends_with_word_character {
            return value.to_string();
        }
        Self::match_case(&PLURAL.inflect(value), value)
    }

    /// Get the singular form of an English word.
    ///
    /// ```
    /// use illuminate_support::Pluralizer;
    ///
    /// assert_eq!(Pluralizer::singular("children"), "child");
    /// assert_eq!(Pluralizer::singular("Categories"), "Category");
    /// ```
    pub fn singular(value: &str) -> String {
        Self::match_case(&SINGULAR.inflect(value), value)
    }

    /// Determine if the given word is one of Laravel's uncountable words.
    pub fn uncountable(value: &str) -> bool {
        let lower = value.to_lowercase();
        UNCOUNTABLE.read().unwrap().iter().any(|w| *w == lower)
    }

    /// Register an additional word that should never be pluralized.
    pub fn add_uncountable(word: impl Into<String>) {
        let word = word.into().to_lowercase();
        let mut list = UNCOUNTABLE.write().unwrap();
        if !list.contains(&word) {
            list.push(word);
        }
    }

    /// Attempt to match the case of the original word, like Laravel.
    fn match_case(value: &str, comparison: &str) -> String {
        if comparison.to_lowercase() == comparison {
            return value.to_lowercase();
        }
        if comparison.to_uppercase() == comparison {
            return value.to_uppercase();
        }
        if ucfirst_ascii(comparison) == comparison {
            return ucfirst_ascii(value);
        }
        if ucwords_ascii(comparison) == comparison {
            return ucwords_ascii(value);
        }
        value.to_string()
    }
}

/// PHP's byte-oriented `ucfirst`.
fn ucfirst_ascii(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
        None => String::new(),
    }
}

/// PHP's byte-oriented `ucwords`.
fn ucwords_ascii(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut capitalize = true;
    for c in value.chars() {
        out.push(if capitalize { c.to_ascii_uppercase() } else { c });
        capitalize = matches!(c, ' ' | '\t' | '\r' | '\n' | '\u{0B}' | '\u{0C}');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_pluralizes() {
        for (singular, plural) in [
            ("user", "users"),
            ("post", "posts"),
            ("comment", "comments"),
            ("category", "categories"),
            ("box", "boxes"),
            ("status", "statuses"),
            ("person", "people"),
            ("child", "children"),
            ("mouse", "mice"),
            ("house", "houses"),
            ("knife", "knives"),
            ("wife", "wives"),
            ("wolf", "wolves"),
            ("leaf", "leaves"),
            ("analysis", "analyses"),
            ("criterion", "criteria"),
            ("datum", "data"),
            ("data", "data"),
            ("medium", "media"),
            ("media", "media"),
            ("news", "news"),
            ("sheep", "sheep"),
            ("fish", "fish"),
            ("cod", "cod"),
            ("salesperson", "salespeople"),
            ("woman", "women"),
            ("photo", "photos"),
            ("piano", "pianos"),
            ("hero", "heroes"),
            ("potato", "potatoes"),
            ("index", "indices"),
            ("matrix", "matrices"),
            ("quiz", "quizzes"),
            ("bus", "buses"),
            ("campus", "campuses"),
            ("cactus", "cacti"),
            ("address", "addresses"),
            ("key", "keys"),
            ("tax", "taxes"),
            ("ox", "oxen"),
            ("tooth", "teeth"),
            ("goose", "geese"),
            ("software", "software"),
            ("equipment", "equipment"),
            ("The word", "The words"),
            ("Bouqueté", "Bouquetés"),
            ("User1", "User1s"),
            ("VortexField", "VortexFields"),
            ("MatrixField", "MatrixFields"),
            ("IndexField", "IndexFields"),
            ("RealHuman", "RealHumen"),
        ] {
            assert_eq!(Pluralizer::plural(singular, 2), plural, "plural of {singular}");
        }
    }

    #[test]
    fn it_singularizes() {
        for (plural, singular) in [
            ("users", "user"),
            ("posts", "post"),
            ("comments", "comment"),
            ("categories", "category"),
            ("boxes", "box"),
            ("statuses", "status"),
            ("status", "status"),
            ("people", "person"),
            ("children", "child"),
            ("mice", "mouse"),
            ("knives", "knife"),
            ("wives", "wife"),
            ("leaves", "leaf"),
            ("heroes", "hero"),
            ("potatoes", "potato"),
            ("photos", "photo"),
            ("analyses", "analysis"),
            ("criteria", "criterion"),
            ("data", "data"),
            ("media", "medium"),
            ("news", "news"),
            ("sheep", "sheep"),
            ("addresses", "address"),
            ("address", "address"),
            ("movies", "movie"),
            ("taxes", "tax"),
            ("quizzes", "quiz"),
            ("indices", "index"),
            ("matrices", "matrix"),
            ("buses", "bus"),
            ("houses", "house"),
            ("cookies", "cookie"),
            ("menus", "menu"),
            ("regattas", "regatta"),
            ("bacteria", "bacterium"),
        ] {
            assert_eq!(Pluralizer::singular(plural), singular, "singular of {plural}");
        }
    }

    #[test]
    fn it_matches_case() {
        assert_eq!(Pluralizer::plural("User", 2), "Users");
        assert_eq!(Pluralizer::plural("USER", 2), "USERS");
        assert_eq!(Pluralizer::plural("Child", 2), "Children");
        assert_eq!(Pluralizer::plural("CHILD", 2), "CHILDREN");
        assert_eq!(Pluralizer::plural("cHiLd", 2), "children");
        assert_eq!(Pluralizer::singular("Children"), "Child");
        assert_eq!(Pluralizer::singular("CHILDREN"), "CHILD");
        assert_eq!(Pluralizer::singular("Tests"), "Test");
    }

    #[test]
    fn it_respects_counts_and_trailing_characters() {
        assert_eq!(Pluralizer::plural("test", 1), "test");
        assert_eq!(Pluralizer::plural("test", -1), "test");
        assert_eq!(Pluralizer::plural("test", -2), "tests");
        assert_eq!(Pluralizer::plural("test", 0), "tests");
        assert_eq!(Pluralizer::plural("Alien.", 2), "Alien.");
        assert_eq!(Pluralizer::plural("Alien!", 2), "Alien!");
        assert_eq!(Pluralizer::plural("Alien ", 2), "Alien ");
        assert_eq!(Pluralizer::plural("50%", 2), "50%");
        assert_eq!(Pluralizer::plural("", 2), "");
        assert_eq!(Pluralizer::plural("related", 2), "related");
        Pluralizer::add_uncountable("laravel");
        assert_eq!(Pluralizer::plural("laravel", 2), "laravel");
    }
}
