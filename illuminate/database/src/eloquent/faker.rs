//! A small, dependency-free fake data generator for factories and seeders,
//! modelled on PHP's Faker (English locale).
//!
//! ```
//! use illuminate_database::eloquent::Faker;
//!
//! let mut faker = Faker::seeded(42);
//! let name = faker.name();
//! let email = faker.unique(|faker| faker.safe_email());
//! assert!(email.contains('@'));
//! assert!(!name.is_empty());
//! ```

use std::collections::{HashMap, HashSet};

use illuminate_support::{Carbon, Str};
use rand::rngs::StdRng;
use rand::seq::{IndexedRandom, SliceRandom};
use rand::{Rng, SeedableRng};

const FIRST_NAMES_MALE: &[&str] = &[
    "James",
    "John",
    "Robert",
    "Michael",
    "William",
    "David",
    "Richard",
    "Joseph",
    "Thomas",
    "Charles",
    "Christopher",
    "Daniel",
    "Matthew",
    "Anthony",
    "Mark",
    "Donald",
    "Steven",
    "Paul",
    "Andrew",
    "Joshua",
    "Kenneth",
    "Kevin",
    "Brian",
    "George",
    "Timothy",
    "Ronald",
    "Edward",
    "Jason",
    "Jeffrey",
    "Ryan",
    "Jacob",
    "Gary",
    "Nicholas",
    "Eric",
    "Jonathan",
    "Stephen",
    "Larry",
    "Justin",
    "Scott",
    "Brandon",
    "Benjamin",
    "Samuel",
    "Gregory",
    "Alexander",
    "Patrick",
    "Frank",
    "Raymond",
    "Jack",
    "Dennis",
    "Jerry",
    "Taylor",
    "Tyler",
    "Aaron",
    "Jose",
    "Adam",
    "Nathan",
    "Henry",
    "Zachary",
    "Douglas",
    "Peter",
];

const FIRST_NAMES_FEMALE: &[&str] = &[
    "Mary",
    "Patricia",
    "Jennifer",
    "Linda",
    "Elizabeth",
    "Barbara",
    "Susan",
    "Jessica",
    "Sarah",
    "Karen",
    "Lisa",
    "Nancy",
    "Betty",
    "Margaret",
    "Sandra",
    "Ashley",
    "Kimberly",
    "Emily",
    "Donna",
    "Michelle",
    "Carol",
    "Amanda",
    "Dorothy",
    "Melissa",
    "Deborah",
    "Stephanie",
    "Rebecca",
    "Sharon",
    "Laura",
    "Cynthia",
    "Kathleen",
    "Amy",
    "Angela",
    "Shirley",
    "Anna",
    "Brenda",
    "Pamela",
    "Emma",
    "Nicole",
    "Helen",
    "Samantha",
    "Katherine",
    "Christine",
    "Debra",
    "Rachel",
    "Carolyn",
    "Janet",
    "Catherine",
    "Maria",
    "Heather",
    "Abigail",
    "Olivia",
    "Sophia",
    "Isabella",
    "Ava",
    "Mia",
    "Charlotte",
    "Amelia",
    "Harper",
    "Evelyn",
];

const LAST_NAMES: &[&str] = &[
    "Smith",
    "Johnson",
    "Williams",
    "Brown",
    "Jones",
    "Garcia",
    "Miller",
    "Davis",
    "Rodriguez",
    "Martinez",
    "Hernandez",
    "Lopez",
    "Gonzalez",
    "Wilson",
    "Anderson",
    "Thomas",
    "Taylor",
    "Moore",
    "Jackson",
    "Martin",
    "Lee",
    "Perez",
    "Thompson",
    "White",
    "Harris",
    "Sanchez",
    "Clark",
    "Ramirez",
    "Lewis",
    "Robinson",
    "Walker",
    "Young",
    "Allen",
    "King",
    "Wright",
    "Scott",
    "Torres",
    "Nguyen",
    "Hill",
    "Flores",
    "Green",
    "Adams",
    "Nelson",
    "Baker",
    "Hall",
    "Rivera",
    "Campbell",
    "Mitchell",
    "Carter",
    "Roberts",
    "Gomez",
    "Phillips",
    "Evans",
    "Turner",
    "Diaz",
    "Parker",
    "Cruz",
    "Edwards",
    "Collins",
    "Reyes",
    "Stewart",
    "Morris",
    "Morales",
    "Murphy",
    "Cook",
    "Rogers",
    "Gutierrez",
    "Ortiz",
    "Morgan",
    "Cooper",
    "Peterson",
    "Bailey",
    "Reed",
    "Kelly",
    "Howard",
    "Ramos",
    "Kim",
    "Cox",
    "Ward",
    "Otwell",
];

const WORDS: &[&str] = &[
    "alias",
    "consequatur",
    "aut",
    "perferendis",
    "sit",
    "voluptatem",
    "accusantium",
    "doloremque",
    "aperiam",
    "eaque",
    "ipsa",
    "quae",
    "ab",
    "illo",
    "inventore",
    "veritatis",
    "et",
    "quasi",
    "architecto",
    "beatae",
    "vitae",
    "dicta",
    "sunt",
    "explicabo",
    "aspernatur",
    "odit",
    "fugit",
    "sed",
    "quia",
    "consequuntur",
    "magni",
    "dolores",
    "eos",
    "qui",
    "ratione",
    "sequi",
    "nesciunt",
    "neque",
    "dolorem",
    "ipsum",
    "dolor",
    "amet",
    "consectetur",
    "adipisci",
    "velit",
    "non",
    "numquam",
    "eius",
    "modi",
    "tempora",
    "incidunt",
    "ut",
    "labore",
    "dolore",
    "magnam",
    "aliquam",
    "quaerat",
    "enim",
    "ad",
    "minima",
    "veniam",
    "quis",
    "nostrum",
    "exercitationem",
    "ullam",
    "corporis",
    "nemo",
    "ipsam",
    "voluptas",
    "suscipit",
    "laboriosam",
    "nisi",
    "aliquid",
    "ex",
    "ea",
    "commodi",
    "autem",
    "vel",
    "eum",
    "iure",
    "reprehenderit",
    "in",
    "voluptate",
    "esse",
    "quam",
    "nihil",
    "molestiae",
    "illum",
    "fugiat",
    "quo",
    "voluptas",
    "nulla",
    "pariatur",
    "at",
    "vero",
    "accusamus",
    "officiis",
    "debitis",
    "rerum",
    "necessitatibus",
    "saepe",
    "eveniet",
    "voluptates",
    "repudiandae",
    "recusandae",
    "itaque",
    "earum",
    "hic",
    "tenetur",
    "a",
    "sapiente",
    "delectus",
    "reiciendis",
    "maiores",
    "doloribus",
    "asperiores",
    "repellat",
    "omnis",
    "iste",
    "natus",
    "error",
    "similique",
    "culpa",
    "officia",
    "deserunt",
    "mollitia",
    "animi",
    "id",
    "est",
    "laborum",
    "dolorum",
    "fuga",
    "harum",
    "quidem",
    "facilis",
    "expedita",
    "distinctio",
    "nam",
    "libero",
    "tempore",
    "cum",
    "soluta",
    "nobis",
    "eligendi",
    "optio",
    "cumque",
    "impedit",
    "minus",
    "maxime",
    "placeat",
    "facere",
    "possimus",
    "assumenda",
    "repellendus",
    "temporibus",
    "quibusdam",
    "blanditiis",
    "praesentium",
    "voluptatum",
    "deleniti",
    "atque",
    "corrupti",
    "quos",
    "quas",
    "molestias",
    "excepturi",
    "sint",
    "occaecati",
    "cupiditate",
    "provident",
    "perspiciatis",
    "unde",
    "totam",
    "rem",
    "porro",
    "quisquam",
    "ullamco",
    "iusto",
];

const FREE_EMAIL_DOMAINS: &[&str] = &[
    "gmail.com",
    "yahoo.com",
    "hotmail.com",
    "outlook.com",
    "icloud.com",
];
const SAFE_EMAIL_DOMAINS: &[&str] = &["example.com", "example.org", "example.net"];
const TLDS: &[&str] = &["com", "net", "org", "biz", "info", "io", "dev"];

const CITY_PREFIXES: &[&str] = &[
    "North", "East", "West", "South", "New", "Lake", "Port", "Fort", "Mount",
];
const CITY_SUFFIXES: &[&str] = &[
    "town", "ton", "land", "ville", "berg", "burgh", "borough", "bury", "view", "port", "mouth",
    "stad", "furt", "chester", "fort", "haven", "side", "shire",
];
const CITIES: &[&str] = &[
    "Springfield",
    "Riverside",
    "Franklin",
    "Greenville",
    "Bristol",
    "Clinton",
    "Fairview",
    "Salem",
    "Madison",
    "Georgetown",
    "Arlington",
    "Ashland",
    "Burlington",
    "Manchester",
    "Milton",
    "Newport",
    "Oxford",
    "Dayton",
    "Lexington",
    "Jackson",
    "Auburn",
    "Dover",
    "Hudson",
    "Kingston",
];
const STREET_SUFFIXES: &[&str] = &[
    "Street",
    "Avenue",
    "Road",
    "Lane",
    "Drive",
    "Court",
    "Place",
    "Boulevard",
    "Way",
    "Terrace",
    "Parkway",
    "Circle",
    "Trail",
    "Square",
    "Crossing",
    "Ridge",
    "Hollow",
    "Ville",
    "Plaza",
    "Pike",
];
const STATES: &[(&str, &str)] = &[
    ("Alabama", "AL"),
    ("Alaska", "AK"),
    ("Arizona", "AZ"),
    ("Arkansas", "AR"),
    ("California", "CA"),
    ("Colorado", "CO"),
    ("Connecticut", "CT"),
    ("Delaware", "DE"),
    ("Florida", "FL"),
    ("Georgia", "GA"),
    ("Hawaii", "HI"),
    ("Idaho", "ID"),
    ("Illinois", "IL"),
    ("Indiana", "IN"),
    ("Iowa", "IA"),
    ("Kansas", "KS"),
    ("Kentucky", "KY"),
    ("Louisiana", "LA"),
    ("Maine", "ME"),
    ("Maryland", "MD"),
    ("Massachusetts", "MA"),
    ("Michigan", "MI"),
    ("Minnesota", "MN"),
    ("Mississippi", "MS"),
    ("Missouri", "MO"),
    ("Montana", "MT"),
    ("Nebraska", "NE"),
    ("Nevada", "NV"),
    ("New Hampshire", "NH"),
    ("New Jersey", "NJ"),
    ("New Mexico", "NM"),
    ("New York", "NY"),
    ("North Carolina", "NC"),
    ("North Dakota", "ND"),
    ("Ohio", "OH"),
    ("Oklahoma", "OK"),
    ("Oregon", "OR"),
    ("Pennsylvania", "PA"),
    ("Rhode Island", "RI"),
    ("South Carolina", "SC"),
    ("South Dakota", "SD"),
    ("Tennessee", "TN"),
    ("Texas", "TX"),
    ("Utah", "UT"),
    ("Vermont", "VT"),
    ("Virginia", "VA"),
    ("Washington", "WA"),
    ("West Virginia", "WV"),
    ("Wisconsin", "WI"),
    ("Wyoming", "WY"),
];
const COUNTRIES: &[(&str, &str)] = &[
    ("United States of America", "US"),
    ("Canada", "CA"),
    ("Mexico", "MX"),
    ("Brazil", "BR"),
    ("Argentina", "AR"),
    ("United Kingdom", "GB"),
    ("Ireland", "IE"),
    ("France", "FR"),
    ("Germany", "DE"),
    ("Spain", "ES"),
    ("Portugal", "PT"),
    ("Italy", "IT"),
    ("Netherlands", "NL"),
    ("Belgium", "BE"),
    ("Switzerland", "CH"),
    ("Austria", "AT"),
    ("Sweden", "SE"),
    ("Norway", "NO"),
    ("Denmark", "DK"),
    ("Finland", "FI"),
    ("Poland", "PL"),
    ("Greece", "GR"),
    ("Turkey", "TR"),
    ("Egypt", "EG"),
    ("South Africa", "ZA"),
    ("Nigeria", "NG"),
    ("Kenya", "KE"),
    ("India", "IN"),
    ("China", "CN"),
    ("Japan", "JP"),
    ("South Korea", "KR"),
    ("Australia", "AU"),
    ("New Zealand", "NZ"),
    ("Singapore", "SG"),
    ("Indonesia", "ID"),
    ("Thailand", "TH"),
    ("Vietnam", "VN"),
];
const COMPANY_SUFFIXES: &[&str] = &["Inc", "LLC", "Ltd", "Group", "PLC", "and Sons", "Co"];
const JOB_TITLES: &[&str] = &[
    "Software Engineer",
    "Web Developer",
    "Product Manager",
    "Data Scientist",
    "Accountant",
    "Architect",
    "Graphic Designer",
    "Marketing Manager",
    "Sales Representative",
    "Nurse",
    "Teacher",
    "Electrician",
    "Mechanical Engineer",
    "Financial Analyst",
    "Human Resources Manager",
    "Chef",
    "Pharmacist",
    "Civil Engineer",
    "Customer Service Representative",
    "Operations Manager",
    "Lawyer",
    "Photographer",
    "Editor",
    "Dentist",
    "Veterinarian",
    "Systems Administrator",
    "Project Manager",
    "Copywriter",
];
const COLOR_NAMES: &[&str] = &[
    "AliceBlue",
    "AntiqueWhite",
    "Aqua",
    "Aquamarine",
    "Azure",
    "Beige",
    "Bisque",
    "Black",
    "Blue",
    "BlueViolet",
    "Brown",
    "BurlyWood",
    "CadetBlue",
    "Chartreuse",
    "Chocolate",
    "Coral",
    "CornflowerBlue",
    "Crimson",
    "Cyan",
    "DarkBlue",
    "DarkCyan",
    "DarkGreen",
    "DarkOrange",
    "DarkRed",
    "DeepPink",
    "DodgerBlue",
    "FireBrick",
    "ForestGreen",
    "Gold",
    "GoldenRod",
    "Gray",
    "Green",
    "HotPink",
    "Indigo",
    "Ivory",
    "Khaki",
    "Lavender",
    "LawnGreen",
    "LightBlue",
    "Lime",
    "Magenta",
    "Maroon",
    "Navy",
    "Olive",
    "Orange",
    "Orchid",
    "Peru",
    "Pink",
    "Plum",
    "Purple",
    "Red",
    "Salmon",
    "SeaGreen",
    "Sienna",
    "Silver",
    "SkyBlue",
    "SlateGray",
    "Tan",
    "Teal",
    "Tomato",
    "Turquoise",
    "Violet",
    "Wheat",
    "White",
    "Yellow",
];
const SAFE_COLOR_NAMES: &[&str] = &[
    "black", "maroon", "green", "navy", "olive", "purple", "teal", "lime", "blue", "silver",
    "gray", "yellow", "fuchsia", "aqua", "white",
];
const CURRENCY_CODES: &[&str] = &[
    "USD", "EUR", "GBP", "JPY", "CAD", "AUD", "CHF", "CNY", "SEK", "NZD", "MXN", "SGD", "HKD",
    "NOK", "KRW", "TRY", "INR", "BRL", "ZAR", "DKK", "PLN",
];
const TITLES_MALE: &[&str] = &["Mr.", "Dr.", "Prof."];
const TITLES_FEMALE: &[&str] = &["Mrs.", "Ms.", "Miss", "Dr.", "Prof."];

/// The maximum number of attempts `unique` makes before giving up.
const MAX_UNIQUE_RETRIES: usize = 10_000;

/// A fake data generator.
pub struct Faker {
    rng: StdRng,
    unique: HashMap<&'static str, HashSet<String>>,
}

impl Default for Faker {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for Faker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Faker").finish_non_exhaustive()
    }
}

impl Faker {
    /// A generator seeded from the operating system.
    pub fn new() -> Self {
        Self {
            rng: StdRng::from_os_rng(),
            unique: HashMap::new(),
        }
    }

    /// A deterministic generator: the same seed yields the same data.
    pub fn seeded(seed: u64) -> Self {
        Self {
            rng: StdRng::seed_from_u64(seed),
            unique: HashMap::new(),
        }
    }

    /// Re-seed the generator.
    pub fn seed(&mut self, seed: u64) {
        self.rng = StdRng::seed_from_u64(seed);
    }

    /// The underlying random number generator.
    pub fn rng(&mut self) -> &mut StdRng {
        &mut self.rng
    }

    // ------------------------------------------------------------------
    // Modifiers
    // ------------------------------------------------------------------

    /// Generate a value that this generator hasn't produced at the same call
    /// site before.
    ///
    /// # Panics
    ///
    /// Panics when no unique value is found after 10,000 attempts (see
    /// [`try_unique`](Faker::try_unique)).
    pub fn unique<T: ToString>(&mut self, generator: impl FnMut(&mut Faker) -> T) -> T {
        match self.try_unique(generator) {
            Ok(value) => value,
            Err(message) => panic!("{message}"),
        }
    }

    /// Generate a value that this generator hasn't produced at the same call
    /// site before, failing after 10,000 attempts.
    pub fn try_unique<T: ToString, G: FnMut(&mut Faker) -> T>(
        &mut self,
        mut generator: G,
    ) -> Result<T, String> {
        let key = std::any::type_name::<G>();
        for _ in 0..MAX_UNIQUE_RETRIES {
            let value = generator(self);
            let string = value.to_string();
            if self.unique.entry(key).or_default().insert(string) {
                return Ok(value);
            }
        }
        Err(format!(
            "Maximum retries of {MAX_UNIQUE_RETRIES} reached without finding a unique value"
        ))
    }

    /// Forget the values `unique` has generated.
    pub fn reset_unique(&mut self) {
        self.unique.clear();
    }

    /// Generate a value with the given probability (0.0 to 1.0), otherwise
    /// `None`.
    pub fn optional<T>(
        &mut self,
        weight: f64,
        generator: impl FnOnce(&mut Faker) -> T,
    ) -> Option<T> {
        if self.rng.random_bool(weight.clamp(0.0, 1.0)) {
            Some(generator(self))
        } else {
            None
        }
    }

    // ------------------------------------------------------------------
    // Numbers and elements
    // ------------------------------------------------------------------

    /// A random integer between `min` and `max` (inclusive).
    pub fn number_between(&mut self, min: i64, max: i64) -> i64 {
        let (min, max) = if min <= max { (min, max) } else { (max, min) };
        self.rng.random_range(min..=max)
    }

    /// A random digit (0-9).
    pub fn random_digit(&mut self) -> u8 {
        self.rng.random_range(0..=9)
    }

    /// A random non-zero digit (1-9).
    pub fn random_digit_not_null(&mut self) -> u8 {
        self.rng.random_range(1..=9)
    }

    /// A random number with up to `digits` digits.
    pub fn random_number(&mut self, digits: u32) -> u64 {
        let digits = digits.clamp(1, 18);
        self.rng.random_range(0..10u64.pow(digits))
    }

    /// A random float between `min` and `max`, rounded to `decimals` places.
    pub fn random_float(&mut self, decimals: u32, min: f64, max: f64) -> f64 {
        let (min, max) = if min <= max { (min, max) } else { (max, min) };
        let value = if min == max {
            min
        } else {
            self.rng.random_range(min..max)
        };
        let factor = 10f64.powi(decimals.min(10) as i32);
        (value * factor).round() / factor
    }

    /// A random boolean, `true` with the given percentage chance (0-100).
    pub fn boolean(&mut self, chance_of_true: u8) -> bool {
        self.rng
            .random_bool(f64::from(chance_of_true.min(100)) / 100.0)
    }

    /// A random element of the slice.
    ///
    /// # Panics
    ///
    /// Panics when the slice is empty.
    pub fn random_element<T: Clone>(&mut self, elements: &[T]) -> T {
        elements
            .choose(&mut self.rng)
            .cloned()
            .expect("random_element requires at least one element")
    }

    /// `count` distinct random elements of the slice (fewer when the slice is
    /// shorter).
    pub fn random_elements<T: Clone>(&mut self, elements: &[T], count: usize) -> Vec<T> {
        elements
            .choose_multiple(&mut self.rng, count.min(elements.len()))
            .cloned()
            .collect()
    }

    /// Shuffle a vector.
    pub fn shuffle<T>(&mut self, mut items: Vec<T>) -> Vec<T> {
        items.shuffle(&mut self.rng);
        items
    }

    /// Replace every `#` with a random digit (and `%` with a non-zero digit).
    pub fn numerify(&mut self, format: &str) -> String {
        format
            .chars()
            .map(|c| match c {
                '#' => char::from(b'0' + self.random_digit()),
                '%' => char::from(b'0' + self.random_digit_not_null()),
                other => other,
            })
            .collect()
    }

    /// Replace every `?` with a random lowercase letter.
    pub fn lexify(&mut self, format: &str) -> String {
        format
            .chars()
            .map(|c| match c {
                '?' => char::from(self.rng.random_range(b'a'..=b'z')),
                other => other,
            })
            .collect()
    }

    /// Replace `#` with digits and `?` with letters (`*` with either).
    pub fn bothify(&mut self, format: &str) -> String {
        let format: String = format
            .chars()
            .map(|c| match c {
                '*' => {
                    if self.rng.random_bool(0.5) {
                        '#'
                    } else {
                        '?'
                    }
                }
                other => other,
            })
            .collect();
        let numerified = self.numerify(&format);
        self.lexify(&numerified)
    }

    // ------------------------------------------------------------------
    // People
    // ------------------------------------------------------------------

    /// A first name.
    pub fn first_name(&mut self) -> String {
        if self.rng.random_bool(0.5) {
            self.first_name_male()
        } else {
            self.first_name_female()
        }
    }

    /// A male first name.
    pub fn first_name_male(&mut self) -> String {
        self.random_element(FIRST_NAMES_MALE).to_string()
    }

    /// A female first name.
    pub fn first_name_female(&mut self) -> String {
        self.random_element(FIRST_NAMES_FEMALE).to_string()
    }

    /// A last name.
    pub fn last_name(&mut self) -> String {
        self.random_element(LAST_NAMES).to_string()
    }

    /// A full name.
    pub fn name(&mut self) -> String {
        format!("{} {}", self.first_name(), self.last_name())
    }

    /// A personal title (`Mr.`, `Dr.`, ...).
    pub fn title(&mut self) -> String {
        if self.rng.random_bool(0.5) {
            self.random_element(TITLES_MALE).to_string()
        } else {
            self.random_element(TITLES_FEMALE).to_string()
        }
    }

    /// A user name (`taylor.otwell`, `abigail42`, ...).
    pub fn user_name(&mut self) -> String {
        let first = Str::lower(&self.first_name());
        let last = Str::lower(&self.last_name());
        match self.rng.random_range(0..4) {
            0 => format!("{first}.{last}"),
            1 => format!("{first}{}", self.number_between(1, 99)),
            2 => format!("{}{last}", &first[..1]),
            _ => format!("{last}.{first}{}", self.number_between(1, 999)),
        }
    }

    /// A random password of 8 to 20 characters.
    pub fn password(&mut self) -> String {
        const CHARS: &[u8] =
            b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789!@#$%^&*";
        let length = self.rng.random_range(8..=20);
        (0..length)
            .map(|_| char::from(CHARS[self.rng.random_range(0..CHARS.len())]))
            .collect()
    }

    /// An email address on a random domain.
    pub fn email(&mut self) -> String {
        if self.rng.random_bool(0.5) {
            self.safe_email()
        } else {
            self.free_email()
        }
    }

    /// An email address on an `example.*` domain (never delivered).
    pub fn safe_email(&mut self) -> String {
        let domain = self.random_element(SAFE_EMAIL_DOMAINS);
        format!("{}@{domain}", self.user_name())
    }

    /// An email address on a free email provider's domain.
    pub fn free_email(&mut self) -> String {
        let domain = self.random_element(FREE_EMAIL_DOMAINS);
        format!("{}@{domain}", self.user_name())
    }

    /// An email address on a company domain.
    pub fn company_email(&mut self) -> String {
        format!("{}@{}", self.user_name(), self.domain_name())
    }

    /// A job title.
    pub fn job_title(&mut self) -> String {
        self.random_element(JOB_TITLES).to_string()
    }

    /// A company name.
    pub fn company(&mut self) -> String {
        match self.rng.random_range(0..3) {
            0 => format!("{} {}", self.last_name(), self.company_suffix()),
            1 => format!("{}-{}", self.last_name(), self.last_name()),
            _ => format!(
                "{}, {} and {}",
                self.last_name(),
                self.last_name(),
                self.last_name()
            ),
        }
    }

    /// A company suffix (`Inc`, `LLC`, ...).
    pub fn company_suffix(&mut self) -> String {
        self.random_element(COMPANY_SUFFIXES).to_string()
    }

    // ------------------------------------------------------------------
    // Text
    // ------------------------------------------------------------------

    /// A random word.
    pub fn word(&mut self) -> String {
        self.random_element(WORDS).to_string()
    }

    /// Several random words.
    pub fn words(&mut self, count: usize) -> Vec<String> {
        (0..count).map(|_| self.word()).collect()
    }

    /// A sentence of about six words.
    pub fn sentence(&mut self) -> String {
        let count = self.rng.random_range(4..=9);
        self.sentence_of(count)
    }

    /// A sentence with exactly the given number of words.
    pub fn sentence_of(&mut self, words: usize) -> String {
        let words = self.words(words.max(1)).join(" ");
        format!("{}.", Str::ucfirst(&words))
    }

    /// Several sentences.
    pub fn sentences(&mut self, count: usize) -> Vec<String> {
        (0..count).map(|_| self.sentence()).collect()
    }

    /// A paragraph of about three sentences.
    pub fn paragraph(&mut self) -> String {
        let count = self.rng.random_range(2..=5);
        self.sentences(count).join(" ")
    }

    /// Several paragraphs.
    pub fn paragraphs(&mut self, count: usize) -> Vec<String> {
        (0..count).map(|_| self.paragraph()).collect()
    }

    /// Text of at most `max_chars` characters (at least 5).
    pub fn text(&mut self, max_chars: usize) -> String {
        let max_chars = max_chars.max(5);
        let mut text = String::new();
        loop {
            let sentence = self.sentence();
            let candidate = if text.is_empty() {
                sentence.clone()
            } else {
                format!("{text} {sentence}")
            };
            if candidate.chars().count() > max_chars {
                break;
            }
            text = candidate;
        }
        if text.is_empty() {
            let mut word = self.word();
            word.truncate(max_chars - 1);
            text = format!("{}.", Str::ucfirst(&word));
        }
        text
    }

    /// A URL slug of a few words.
    pub fn slug(&mut self) -> String {
        let count = self.rng.random_range(3..=6);
        self.words(count).join("-")
    }

    // ------------------------------------------------------------------
    // Internet
    // ------------------------------------------------------------------

    /// A domain word (`otwell`, `smith-jones`).
    pub fn domain_word(&mut self) -> String {
        Str::slug(&self.last_name())
    }

    /// A top-level domain.
    pub fn tld(&mut self) -> String {
        self.random_element(TLDS).to_string()
    }

    /// A domain name.
    pub fn domain_name(&mut self) -> String {
        format!("{}.{}", self.domain_word(), self.tld())
    }

    /// A URL.
    pub fn url(&mut self) -> String {
        if self.rng.random_bool(0.5) {
            format!("https://www.{}/", self.domain_name())
        } else {
            format!("https://{}/{}", self.domain_name(), self.slug())
        }
    }

    /// An IPv4 address.
    pub fn ipv4(&mut self) -> String {
        let octets: Vec<String> = (0..4)
            .map(|i| {
                self.rng
                    .random_range(if i == 0 { 1 } else { 0 }..=254u8)
                    .to_string()
            })
            .collect();
        octets.join(".")
    }

    /// An IP address (alias of [`ipv4`](Faker::ipv4)).
    pub fn ip(&mut self) -> String {
        self.ipv4()
    }

    /// An IPv6 address.
    pub fn ipv6(&mut self) -> String {
        let groups: Vec<String> = (0..8)
            .map(|_| format!("{:x}", self.rng.random_range(0..=0xffffu32)))
            .collect();
        groups.join(":")
    }

    /// A MAC address.
    pub fn mac_address(&mut self) -> String {
        let bytes: Vec<String> = (0..6)
            .map(|_| format!("{:02X}", self.rng.random_range(0..=255u8)))
            .collect();
        bytes.join(":")
    }

    /// A placeholder image URL.
    pub fn image_url(&mut self, width: u32, height: u32) -> String {
        format!(
            "https://via.placeholder.com/{width}x{height}.png/{}",
            self.hex_color().trim_start_matches('#')
        )
    }

    // ------------------------------------------------------------------
    // Addresses and phone numbers
    // ------------------------------------------------------------------

    /// A phone number.
    pub fn phone_number(&mut self) -> String {
        let format = self.random_element(&[
            "(%##) ###-####",
            "%##-###-####",
            "%##.###.####",
            "+1-%##-###-####",
            "1-%##-###-####",
        ]);
        self.numerify(format)
    }

    /// A phone number in E.164 format.
    pub fn e164_phone_number(&mut self) -> String {
        self.numerify("+1%#########")
    }

    /// A building number.
    pub fn building_number(&mut self) -> String {
        let format = self.random_element(&["%##", "%###", "%#", "%####"]);
        self.numerify(format)
    }

    /// A street name.
    pub fn street_name(&mut self) -> String {
        let name = if self.rng.random_bool(0.5) {
            self.first_name()
        } else {
            self.last_name()
        };
        format!("{name} {}", self.random_element(STREET_SUFFIXES))
    }

    /// A street address.
    pub fn street_address(&mut self) -> String {
        format!("{} {}", self.building_number(), self.street_name())
    }

    /// A city.
    pub fn city(&mut self) -> String {
        match self.rng.random_range(0..3) {
            0 => format!(
                "{} {}",
                self.random_element(CITY_PREFIXES),
                self.random_element(CITIES)
            ),
            1 => format!("{}{}", self.last_name(), self.random_element(CITY_SUFFIXES)),
            _ => self.random_element(CITIES).to_string(),
        }
    }

    /// A US state.
    pub fn state(&mut self) -> String {
        self.random_element(STATES).0.to_string()
    }

    /// A US state abbreviation.
    pub fn state_abbr(&mut self) -> String {
        self.random_element(STATES).1.to_string()
    }

    /// A postcode.
    pub fn postcode(&mut self) -> String {
        if self.rng.random_bool(0.7) {
            self.numerify("#####")
        } else {
            self.numerify("#####-####")
        }
    }

    /// A country.
    pub fn country(&mut self) -> String {
        self.random_element(COUNTRIES).0.to_string()
    }

    /// An ISO 3166-1 alpha-2 country code.
    pub fn country_code(&mut self) -> String {
        self.random_element(COUNTRIES).1.to_string()
    }

    /// A full address.
    pub fn address(&mut self) -> String {
        format!(
            "{}\n{}, {} {}",
            self.street_address(),
            self.city(),
            self.state_abbr(),
            self.postcode()
        )
    }

    /// A latitude.
    pub fn latitude(&mut self) -> f64 {
        self.random_float(6, -90.0, 90.0)
    }

    /// A longitude.
    pub fn longitude(&mut self) -> f64 {
        self.random_float(6, -180.0, 180.0)
    }

    // ------------------------------------------------------------------
    // Dates and times
    // ------------------------------------------------------------------

    /// A date between two dates, given in any format `Carbon::parse`
    /// understands (`"-30 years"`, `"now"`, `"2024-01-01"`).
    pub fn date_time_between(&mut self, start: &str, end: &str) -> Carbon {
        let start = Carbon::parse(start).unwrap_or_else(|_| Carbon::now().sub_years(30));
        let end = Carbon::parse(end).unwrap_or_else(|_| Carbon::now());
        self.date_time_between_dates(start, end)
    }

    /// A date between two dates.
    pub fn date_time_between_dates(&mut self, start: Carbon, end: Carbon) -> Carbon {
        let (start, end) = (start.timestamp(), end.timestamp());
        let (start, end) = if start <= end {
            (start, end)
        } else {
            (end, start)
        };
        Carbon::from_timestamp(self.rng.random_range(start..=end))
    }

    /// A date in the past (up to 30 years ago).
    pub fn date_time(&mut self) -> Carbon {
        self.date_time_between("-30 years", "now")
    }

    /// A date this year, up to now.
    pub fn date_time_this_year(&mut self) -> Carbon {
        let now = Carbon::now();
        self.date_time_between_dates(now.start_of_year(), now)
    }

    /// A date this month, up to now.
    pub fn date_time_this_month(&mut self) -> Carbon {
        let now = Carbon::now();
        self.date_time_between_dates(now.start_of_month(), now)
    }

    /// A date (`Y-m-d`).
    pub fn date(&mut self) -> String {
        self.date_time().to_date_string()
    }

    /// A time (`H:i:s`).
    pub fn time(&mut self) -> String {
        self.date_time().to_time_string()
    }

    /// An ISO 8601 date and time.
    pub fn iso8601(&mut self) -> String {
        self.date_time().to_iso8601_string()
    }

    /// A Unix timestamp.
    pub fn unix_time(&mut self) -> i64 {
        self.date_time().timestamp()
    }

    // ------------------------------------------------------------------
    // Miscellaneous
    // ------------------------------------------------------------------

    /// A version 4 UUID string.
    pub fn uuid(&mut self) -> String {
        let mut bytes: [u8; 16] = self.rng.random();
        bytes[6] = (bytes[6] & 0x0f) | 0x40;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        format!(
            "{}-{}-{}-{}-{}",
            &hex[..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..]
        )
    }

    /// A hex color (`#a1b2c3`).
    pub fn hex_color(&mut self) -> String {
        format!("#{:06x}", self.rng.random_range(0..=0xffffffu32))
    }

    /// An RGB color (`12,200,255`).
    pub fn rgb_color(&mut self) -> String {
        let [r, g, b]: [u8; 3] = self.rng.random();
        format!("{r},{g},{b}")
    }

    /// A color name.
    pub fn color_name(&mut self) -> String {
        self.random_element(COLOR_NAMES).to_string()
    }

    /// A CSS-safe color name.
    pub fn safe_color_name(&mut self) -> String {
        self.random_element(SAFE_COLOR_NAMES).to_string()
    }

    /// An ISO 4217 currency code.
    pub fn currency_code(&mut self) -> String {
        self.random_element(CURRENCY_CODES).to_string()
    }
}
