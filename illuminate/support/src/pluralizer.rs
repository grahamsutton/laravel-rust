//! English pluralization and singularization, ported from the inflection
//! rules Laravel relies on.

use std::sync::LazyLock;

use regex::Regex;

/// Words that are the same in singular and plural form.
const UNCOUNTABLE: &[&str] = &[
    "audio", "bison", "cattle", "chassis", "compensation", "coreopsis", "data", "deer",
    "education", "emoji", "equipment", "evidence", "feedback", "firmware", "fish", "furniture",
    "gold", "hardware", "information", "jedi", "kin", "knowledge", "love", "metadata", "money",
    "moose", "news", "nutrition", "offspring", "plankton", "pokemon", "police", "rain",
    "recommended", "related", "rice", "series", "sheep", "software", "species", "swine",
    "traffic", "wheat", "staff", "advice", "art", "baggage", "butter", "clothing", "coal",
    "cotton", "debris", "economics", "electricity", "flour", "garbage", "homework", "jewelry",
    "luggage", "management", "mail", "music", "oxygen", "permission", "research", "sand",
    "spam", "steam", "weather", "wood", "wool", "aircraft", "blood", "chess", "corps",
    "cod", "faqs", "fruit", "gallows", "graffiti", "headquarters", "innings", "means",
    "mews", "pliers", "proceedings", "salmon", "scissors", "sea-bass", "shorts", "trout",
    "tuna", "whiting", "wildebeest",
];

/// Irregular singular => plural pairs.
const IRREGULAR: &[(&str, &str)] = &[
    ("atlas", "atlases"),
    ("axe", "axes"),
    ("beef", "beefs"),
    ("blouse", "blouses"),
    ("brother", "brothers"),
    ("cafe", "cafes"),
    ("chateau", "chateaux"),
    ("child", "children"),
    ("cookie", "cookies"),
    ("corpus", "corpuses"),
    ("cow", "cows"),
    ("criterion", "criteria"),
    ("curriculum", "curricula"),
    ("demo", "demos"),
    ("domino", "dominoes"),
    ("echo", "echoes"),
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

struct Rule {
    pattern: Regex,
    replacement: &'static str,
}

fn rules(list: &[(&str, &'static str)]) -> Vec<Rule> {
    list.iter()
        .map(|(pattern, replacement)| Rule {
            pattern: Regex::new(&format!("(?i){pattern}")).expect("valid inflection rule"),
            replacement,
        })
        .collect()
}

static PLURAL_RULES: LazyLock<Vec<Rule>> = LazyLock::new(|| {
    rules(&[
        (r"(s)tatus$", "${1}tatuses"),
        (r"(quiz)$", "${1}zes"),
        (r"^(ox)$", "${1}en"),
        (r"([m|l])ouse$", "${1}ice"),
        (r"(matr|vert|ind)(ix|ex)$", "${1}ices"),
        (r"(x|ch|ss|sh)$", "${1}es"),
        (r"([^aeiouy]|qu)y$", "${1}ies"),
        (r"(hive|gulf)$", "${1}s"),
        (r"(?:([^f])fe|([lr])f)$", "${1}${2}ves"),
        (r"sis$", "ses"),
        (r"([ti])um$", "${1}a"),
        (r"(tax)on$", "${1}a"),
        (r"(c)riterion$", "${1}riteria"),
        (r"(p)erson$", "${1}eople"),
        (r"(m)an$", "${1}en"),
        (r"(c)hild$", "${1}hildren"),
        (r"(f)oot$", "${1}eet"),
        (r"(buffal|her|potat|tomat|volcan)o$", "${1}oes"),
        (r"(alumn|bacill|cact|foc|fung|nucle|radi|stimul|syllab|termin|vir)us$", "${1}i"),
        (r"us$", "uses"),
        (r"(alias)$", "${1}es"),
        (r"(analys|ax|cris|test|thes)is$", "${1}es"),
        (r"s$", "s"),
        (r"^$", ""),
        (r"$", "s"),
    ])
});

static SINGULAR_RULES: LazyLock<Vec<Rule>> = LazyLock::new(|| {
    rules(&[
        (r"(s)tatuses$", "${1}tatus"),
        (r"^(.*)(menu)s$", "${1}${2}"),
        (r"(quiz)zes$", "${1}"),
        (r"(matr)ices$", "${1}ix"),
        (r"(vert|ind)ices$", "${1}ex"),
        (r"^(ox)en", "${1}"),
        (r"(alias)(es)*$", "${1}"),
        (r"(buffal|her|potat|tomat|volcan)oes$", "${1}o"),
        (r"(alumn|bacill|cact|foc|fung|nucle|radi|stimul|syllab|termin|viri?)i$", "${1}us"),
        (r"([ftw]ax)es", "${1}"),
        (r"(analys|ax|cris|test|thes)es$", "${1}is"),
        (r"(shoe|slave)s$", "${1}"),
        (r"(o)es$", "${1}"),
        (r"ouses$", "ouse"),
        (r"([^a])uses$", "${1}us"),
        (r"([m|l])ice$", "${1}ouse"),
        (r"(x|ch|ss|sh)es$", "${1}"),
        (r"(m)ovies$", "${1}ovie"),
        (r"(s)eries$", "${1}eries"),
        (r"([^aeiouy]|qu)ies$", "${1}y"),
        (r"([lr])ves$", "${1}f"),
        (r"(tive)s$", "${1}"),
        (r"(hive)s$", "${1}"),
        (r"(drive)s$", "${1}"),
        (r"(dive)s$", "${1}"),
        (r"(olive)s$", "${1}"),
        (r"([^fo])ves$", "${1}fe"),
        (r"(^analy)ses$", "${1}sis"),
        (r"(analy|diagno|^ba|(p)arenthe|(p)rogno|(s)ynop|(t)he)ses$", "${1}${2}sis"),
        (r"(tax)a$", "${1}on"),
        (r"(c)riteria$", "${1}riterion"),
        (r"([ti])a$", "${1}um"),
        (r"(p)eople$", "${1}erson"),
        (r"(m)en$", "${1}an"),
        (r"(c)hildren$", "${1}hild"),
        (r"(f)eet$", "${1}oot"),
        (r"(n)ews$", "${1}ews"),
        (r"eaus$", "eau"),
        (r"^(.*us)$", "${1}"),
        (r"s$", ""),
    ])
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
        if count.abs() == 1 || Self::uncountable(value) || value.is_empty() {
            return value.to_string();
        }
        let plural = Self::inflect(value, true);
        Self::match_case(&plural, value)
    }

    /// Get the singular form of an English word.
    pub fn singular(value: &str) -> String {
        if Self::uncountable(value) || value.is_empty() {
            return value.to_string();
        }
        let singular = Self::inflect(value, false);
        Self::match_case(&singular, value)
    }

    /// Determine if the given word is uncountable.
    pub fn uncountable(value: &str) -> bool {
        let lower = value.to_lowercase();
        UNCOUNTABLE.contains(&lower.as_str())
    }

    fn inflect(value: &str, plural: bool) -> String {
        let lower = value.to_lowercase();

        for (singular_form, plural_form) in IRREGULAR {
            let (from, to) = if plural {
                (*singular_form, *plural_form)
            } else {
                (*plural_form, *singular_form)
            };
            if lower == from {
                return to.to_string();
            }
            // Compound words ending in an irregular word ("salesperson").
            if lower.ends_with(from) && lower.len() > from.len() {
                let prefix = &value[..value.len() - from.len()];
                if prefix.chars().count() >= 3 && from.len() >= 4 {
                    return format!("{}{}", prefix.to_lowercase(), to);
                }
            }
            if lower == to {
                return value.to_lowercase();
            }
        }

        let rules = if plural { &PLURAL_RULES } else { &SINGULAR_RULES };
        for rule in rules.iter() {
            if rule.pattern.is_match(value) {
                return rule.pattern.replace(value, rule.replacement).to_string();
            }
        }
        value.to_string()
    }

    /// Attempt to match the case of the original word.
    fn match_case(value: &str, comparison: &str) -> String {
        if comparison.chars().all(|c| !c.is_alphabetic() || c.is_lowercase()) {
            return value.to_lowercase();
        }
        if comparison.chars().all(|c| !c.is_alphabetic() || c.is_uppercase()) {
            return value.to_uppercase();
        }
        let mut chars = comparison.chars();
        if chars.next().is_some_and(|c| c.is_uppercase())
            && chars.all(|c| !c.is_alphabetic() || c.is_lowercase())
        {
            let mut out = value.to_lowercase();
            if let Some(first) = out.get(0..1) {
                out = first.to_uppercase() + &out[1..];
            }
            return out;
        }
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_pluralizes() {
        for (singular, plural) in [
            ("user", "users"),
            ("post", "posts"),
            ("category", "categories"),
            ("box", "boxes"),
            ("status", "statuses"),
            ("person", "people"),
            ("child", "children"),
            ("mouse", "mice"),
            ("knife", "knives"),
            ("leaf", "leaves"),
            ("analysis", "analyses"),
            ("sheep", "sheep"),
            ("salesperson", "salespeople"),
            ("photo", "photos"),
            ("hero", "heroes"),
            ("index", "indices"),
            ("quiz", "quizzes"),
            ("bus", "buses"),
            ("address", "addresses"),
            ("key", "keys"),
            ("tax", "taxes"),
        ] {
            assert_eq!(Pluralizer::plural(singular, 2), plural, "plural of {singular}");
        }
    }

    #[test]
    fn it_singularizes() {
        for (plural, singular) in [
            ("users", "user"),
            ("categories", "category"),
            ("boxes", "box"),
            ("statuses", "status"),
            ("people", "person"),
            ("children", "child"),
            ("mice", "mouse"),
            ("knives", "knife"),
            ("analyses", "analysis"),
            ("news", "news"),
            ("addresses", "address"),
            ("movies", "movie"),
            ("taxes", "tax"),
        ] {
            assert_eq!(Pluralizer::singular(plural), singular, "singular of {plural}");
        }
    }

    #[test]
    fn it_matches_case() {
        assert_eq!(Pluralizer::plural("User", 2), "Users");
        assert_eq!(Pluralizer::plural("USER", 2), "USERS");
        assert_eq!(Pluralizer::plural("Child", 2), "Children");
    }
}
