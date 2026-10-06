//! Character transliteration tables used by `Str::ascii`, `Str::slug` and
//! `Str::transliterate`.

use std::collections::HashMap;
use std::sync::LazyLock;

/// Groups of characters sharing an ASCII replacement.
const GROUPS: &[(&str, &str)] = &[
    // Latin
    ("a", "àáâãäåāăąǎǟǡǻȁȃȧạảấầẩẫậắằẳẵặⱥɐａⓐ"),
    ("A", "ÀÁÂÃÄÅĀĂĄǍǞǠǺȀȂȦẠẢẤẦẨẪẬẮẰẲẴẶȺＡⒶ"),
    ("ae", "æǣǽ"),
    ("AE", "ÆǢǼ"),
    ("b", "ƀɓḃḅḇｂⓑ"),
    ("B", "ƁɃḂḄḆＢⒷ"),
    ("c", "çćĉċčƈȼḉｃⓒ"),
    ("C", "ÇĆĈĊČƇȻḈＣⒸ"),
    ("d", "ďđɖɗḋḍḏḑḓðｄⓓ"),
    ("D", "ĎĐƉƊḊḌḎḐḒÐＤⒹ"),
    ("dz", "ǆǳ"),
    ("Dz", "ǅǲ"),
    ("DZ", "ǄǱ"),
    ("e", "èéêëēĕėęěȅȇȩẹẻẽếềểễệḕḗḙḛḝɇｅⓔ"),
    ("E", "ÈÉÊËĒĔĖĘĚȄȆȨẸẺẼẾỀỂỄỆḔḖḘḚḜɆＥⒺ"),
    ("f", "ƒḟｆⓕ"),
    ("F", "ƑḞＦⒻ"),
    ("g", "ĝğġģǥǧǵɠḡｇⓖ"),
    ("G", "ĜĞĠĢǤǦǴƓḠＧⒼ"),
    ("h", "ĥħȟḣḥḧḩḫẖｈⓗ"),
    ("H", "ĤĦȞḢḤḦḨḪＨⒽ"),
    ("i", "ìíîïĩīĭįıǐȉȋỉịḭḯｉⓘ"),
    ("I", "ÌÍÎÏĨĪĬĮİǏȈȊỈỊḬḮＩⒾ"),
    ("ij", "ĳ"),
    ("IJ", "Ĳ"),
    ("j", "ĵǰɉｊⓙ"),
    ("J", "ĴɈＪⒿ"),
    ("k", "ķĸƙǩḱḳḵｋⓚ"),
    ("K", "ĶƘǨḰḲḴＫⓀ"),
    ("l", "ĺļľŀłƚḷḹḻḽｌⓛ"),
    ("L", "ĹĻĽĿŁȽḶḸḺḼＬⓁ"),
    ("lj", "ǉ"),
    ("Lj", "ǈ"),
    ("LJ", "Ǉ"),
    ("m", "ḿṁṃｍⓜ"),
    ("M", "ḾṀṂＭⓂ"),
    ("n", "ñńņňŉŋǹṅṇṉṋｎⓝ"),
    ("N", "ÑŃŅŇŊǸṄṆṈṊＮⓃ"),
    ("nj", "ǌ"),
    ("Nj", "ǋ"),
    ("NJ", "Ǌ"),
    ("o", "òóôõöøōŏőơǒǫǭǿȍȏȫȭȯȱọỏốồổỗộớờởỡợṍṏṑṓｏⓞ"),
    ("O", "ÒÓÔÕÖØŌŎŐƠǑǪǬǾȌȎȪȬȮȰỌỎỐỒỔỖỘỚỜỞỠỢṌṎṐṒＯⓄ"),
    ("oe", "œ"),
    ("OE", "Œ"),
    ("p", "ƥṕṗｐⓟ"),
    ("P", "ƤṔṖＰⓅ"),
    ("q", "ɋｑⓠ"),
    ("Q", "ɊＱⓆ"),
    ("r", "ŕŗřȑȓɍṙṛṝṟｒⓡ"),
    ("R", "ŔŖŘȐȒɌṘṚṜṞＲⓇ"),
    ("s", "śŝşšșſṡṣṥṧṩｓⓢ"),
    ("S", "ŚŜŞŠȘṠṢṤṦṨＳⓈ"),
    ("ss", "ß"),
    ("SS", "ẞ"),
    ("t", "ţťŧƫƭțṫṭṯṱẗｔⓣ"),
    ("T", "ŢŤŦƬƮȚṪṬṮṰＴⓉ"),
    ("th", "þ"),
    ("TH", "Þ"),
    ("u", "ùúûüũūŭůűųưǔǖǘǚǜȕȗụủứừửữựṳṵṷṹṻｕⓤ"),
    ("U", "ÙÚÛÜŨŪŬŮŰŲƯǓǕǗǙǛȔȖỤỦỨỪỬỮỰṲṴṶṸṺＵⓊ"),
    ("v", "ʋṽṿｖⓥ"),
    ("V", "ƲṼṾＶⓋ"),
    ("w", "ŵẁẃẅẇẉẘｗⓦ"),
    ("W", "ŴẀẂẄẆẈＷⓌ"),
    ("x", "ẋẍｘⓧ"),
    ("X", "ẊẌＸⓍ"),
    ("y", "ýÿŷƴȳɏẏẙỳỵỷỹｙⓨ"),
    ("Y", "ÝŸŶƳȲɎẎỲỴỶỸＹⓎ"),
    ("z", "źżžƶȥɀẑẓẕｚⓩ"),
    ("Z", "ŹŻŽƵȤẐẒẔＺⓏ"),
    // Greek
    ("a", "αάἀἁἂἃἄἅἆἇὰάᾀᾁᾂᾃᾄᾅᾆᾇᾰᾱᾲᾳᾴᾶᾷ"),
    ("A", "ΑΆἈἉἊἋἌἍἎἏᾈᾉᾊᾋᾌᾍᾎᾏᾸᾹᾺΆᾼ"),
    ("b", "β"),
    ("B", "Β"),
    ("g", "γ"),
    ("G", "Γ"),
    ("d", "δ"),
    ("D", "Δ"),
    ("e", "εέἐἑἒἓἔἕὲέ"),
    ("E", "ΕΈἘἙἚἛἜἝῈΈ"),
    ("z", "ζ"),
    ("Z", "Ζ"),
    ("i", "ηήἠἡἢἣἤἥἦἧὴήᾐᾑᾒᾓᾔᾕᾖᾗῂῃῄῆῇιίϊΐἰἱἲἳἴἵἶἷὶίῐῑῒΐῖῗ"),
    ("I", "ΗΉἨἩἪἫἬἭἮἯῊΉᾘᾙᾚᾛᾜᾝᾞᾟῌΙΊΪἸἹἺἻἼἽἾἿῘῙῚΊ"),
    ("th", "θ"),
    ("TH", "Θ"),
    ("k", "κ"),
    ("K", "Κ"),
    ("l", "λ"),
    ("L", "Λ"),
    ("m", "μµ"),
    ("M", "Μ"),
    ("n", "ν"),
    ("N", "Ν"),
    ("x", "ξ"),
    ("X", "Ξ"),
    ("o", "οόὀὁὂὃὄὅὸόωώὠὡὢὣὤὥὦὧὼώᾠᾡᾢᾣᾤᾥᾦᾧῲῳῴῶῷ"),
    ("O", "ΟΌὈὉὊὋὌὍῸΌΩΏὨὩὪὫὬὭὮὯῺΏᾨᾩᾪᾫᾬᾭᾮᾯῼ"),
    ("p", "π"),
    ("P", "Π"),
    ("r", "ρῤῥ"),
    ("R", "ΡῬ"),
    ("s", "σς"),
    ("S", "Σ"),
    ("t", "τ"),
    ("T", "Τ"),
    ("y", "υύϋΰὐὑὒὓὔὕὖὗὺύῠῡῢΰῦῧ"),
    ("Y", "ΥΎΫὙὛὝὟῨῩῪΎ"),
    ("f", "φ"),
    ("F", "Φ"),
    ("ch", "χ"),
    ("CH", "Χ"),
    ("ps", "ψ"),
    ("PS", "Ψ"),
    // Cyrillic
    ("a", "а"),
    ("A", "А"),
    ("b", "б"),
    ("B", "Б"),
    ("v", "в"),
    ("V", "В"),
    ("g", "гґѓ"),
    ("G", "ГҐЃ"),
    ("d", "д"),
    ("D", "Д"),
    ("e", "еэ"),
    ("E", "ЕЭ"),
    ("yo", "ё"),
    ("Yo", "Ё"),
    ("ye", "є"),
    ("Ye", "Є"),
    ("zh", "ж"),
    ("Zh", "Ж"),
    ("z", "з"),
    ("Z", "З"),
    ("i", "иіѝ"),
    ("I", "ИІЍ"),
    ("yi", "ї"),
    ("Yi", "Ї"),
    ("y", "йы"),
    ("Y", "ЙЫ"),
    ("k", "кќ"),
    ("K", "КЌ"),
    ("l", "л"),
    ("L", "Л"),
    ("m", "м"),
    ("M", "М"),
    ("n", "н"),
    ("N", "Н"),
    ("o", "о"),
    ("O", "О"),
    ("p", "п"),
    ("P", "П"),
    ("r", "р"),
    ("R", "Р"),
    ("s", "с"),
    ("S", "С"),
    ("t", "т"),
    ("T", "Т"),
    ("u", "уў"),
    ("U", "УЎ"),
    ("f", "ф"),
    ("F", "Ф"),
    ("h", "х"),
    ("H", "Х"),
    ("ts", "ц"),
    ("Ts", "Ц"),
    ("ch", "ч"),
    ("Ch", "Ч"),
    ("sh", "ш"),
    ("Sh", "Ш"),
    ("shch", "щ"),
    ("Shch", "Щ"),
    ("", "ъьЪЬ"),
    ("yu", "ю"),
    ("Yu", "Ю"),
    ("ya", "я"),
    ("Ya", "Я"),
    ("dj", "ђ"),
    ("Dj", "Ђ"),
    ("j", "ј"),
    ("J", "Ј"),
    ("lj", "љ"),
    ("Lj", "Љ"),
    ("nj", "њ"),
    ("Nj", "Њ"),
    ("c", "ћ"),
    ("C", "Ћ"),
    ("dz", "џѕ"),
    ("Dz", "ЏЅ"),
    // Ligatures, numerals and symbols
    ("ff", "ﬀ"),
    ("fi", "ﬁ"),
    ("fl", "ﬂ"),
    ("ffi", "ﬃ"),
    ("ffl", "ﬄ"),
    ("st", "ﬅﬆ"),
    ("0", "⁰₀⓪０"),
    ("1", "¹₁①⑴⒈１"),
    ("2", "²₂②⑵⒉２"),
    ("3", "³₃③⑶⒊３"),
    ("4", "⁴₄④⑷⒋４"),
    ("5", "⁵₅⑤⑸⒌５"),
    ("6", "⁶₆⑥⑹⒍６"),
    ("7", "⁷₇⑦⑺⒎７"),
    ("8", "⁸₈⑧⑻⒏８"),
    ("9", "⁹₉⑨⑼⒐９"),
    ("1/4", "¼"),
    ("1/2", "½"),
    ("3/4", "¾"),
    ("'", "‘’‚‛′´`ʹʼ"),
    ("\"", "“”„‟″«»"),
    ("-", "‐‑‒–—―−"),
    ("...", "…"),
    (" ", "\u{00A0}\u{2000}\u{2001}\u{2002}\u{2003}\u{2004}\u{2005}\u{2006}\u{2007}\u{2008}\u{2009}\u{200A}\u{202F}\u{205F}\u{3000}"),
    ("", "\u{00AD}\u{200B}\u{200C}\u{200D}\u{2060}\u{FEFF}"),
    ("*", "•·"),
    ("x", "×"),
    ("/", "÷⁄"),
    ("(c)", "©"),
    ("(r)", "®"),
    ("TM", "™"),
    ("<", "‹"),
    (">", "›"),
    ("EUR", "€"),
    ("!", "¡"),
    ("?", "¿"),
];

static TABLE: LazyLock<HashMap<char, &'static str>> = LazyLock::new(|| {
    let mut table = HashMap::new();
    for (replacement, characters) in GROUPS {
        for c in characters.chars() {
            table.entry(c).or_insert(*replacement);
        }
    }
    table
});

/// Look up the ASCII replacement for a single character.
pub(crate) fn lookup(c: char, language: Option<&str>) -> Option<&'static str> {
    if c.is_ascii() {
        return None;
    }
    if let Some(language) = language {
        let special = match (language, c) {
            ("de", 'ä') => Some("ae"),
            ("de", 'ö') => Some("oe"),
            ("de", 'ü') => Some("ue"),
            ("de", 'Ä') => Some("Ae"),
            ("de", 'Ö') => Some("Oe"),
            ("de", 'Ü') => Some("Ue"),
            ("bg", 'щ') => Some("sht"),
            ("bg", 'Щ') => Some("Sht"),
            ("bg", 'ъ') => Some("a"),
            ("bg", 'Ъ') => Some("A"),
            ("bg", 'ь') => Some("y"),
            ("bg", 'Ь') => Some("Y"),
            ("bg", 'ю') => Some("yu"),
            ("bg", 'я') => Some("ya"),
            _ => None,
        };
        if special.is_some() {
            return special;
        }
    }
    if let Some(found) = TABLE.get(&c) {
        return Some(found);
    }
    // Remaining fullwidth ASCII variants map by offset.
    if ('\u{FF01}'..='\u{FF5E}').contains(&c) {
        let index = c as usize - 0xFF01;
        return PRINTABLE_ASCII.get(index..index + 1);
    }
    None
}

const PRINTABLE_ASCII: &str = "!\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~";

/// Transliterate a string to ASCII, replacing unknown characters with `unknown`.
pub(crate) fn to_ascii(value: &str, language: Option<&str>, unknown: Option<&str>) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        if c.is_ascii() {
            out.push(c);
        } else if let Some(replacement) = lookup(c, language) {
            out.push_str(replacement);
        } else if is_combining_mark(c) {
            // Combining accents are dropped, leaving their base letter.
        } else if let Some(unknown) = unknown {
            out.push_str(unknown);
        }
    }
    out
}

fn is_combining_mark(c: char) -> bool {
    matches!(c,
        '\u{0300}'..='\u{036F}'
            | '\u{1AB0}'..='\u{1AFF}'
            | '\u{1DC0}'..='\u{1DFF}'
            | '\u{20D0}'..='\u{20FF}'
            | '\u{FE20}'..='\u{FE2F}')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_transliterates() {
        assert_eq!(to_ascii("Crème Brûlée", None, None), "Creme Brulee");
        assert_eq!(to_ascii("ⓣⓔⓢⓣ@ⓛⓐⓡⓐⓥⓔⓛ.ⓒⓞⓜ", None, Some("?")), "test@laravel.com");
        assert_eq!(to_ascii("Привет", None, None), "Privet");
        assert_eq!(to_ascii("ä ö ü Ä Ö Ü", Some("de"), None), "ae oe ue Ae Oe Ue");
        assert_eq!(to_ascii("ＬＡＲＡＶＥＬ！", None, None), "LARAVEL!");
        assert_eq!(to_ascii("日本", None, Some("?")), "??");
        assert_eq!(to_ascii("e\u{301}", None, None), "e");
    }
}
