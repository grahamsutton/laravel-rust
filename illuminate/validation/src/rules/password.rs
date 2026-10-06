//! The `Password` rule object.

use std::sync::{Arc, LazyLock};

use async_trait::async_trait;
use regex::Regex;

use illuminate_container::{Container, try_app};
use illuminate_support::{Conditionable, Value, ValueExt, json};

use super::{RuleItem, RuleSet, Rules};
use crate::rule::{FailCallback, UncompromisedVerifier, ValidationContext, ValidationRule};

/// The application's default password rule, stored in the container by
/// [`Password::set_defaults`].
#[derive(Clone)]
pub struct PasswordDefaults(pub Arc<dyn Fn() -> Password + Send + Sync>);

/// Ensure passwords have an adequate level of complexity.
///
/// ```
/// use illuminate_validation::Password;
///
/// let rule = Password::min(8).letters().mixed_case().numbers().symbols();
/// assert_eq!(
///     rule.to_password_rules_string(),
///     "minlength: 8; required: lower; required: upper; required: digit; required: special;"
/// );
/// ```
#[derive(Clone)]
pub struct Password {
    min: usize,
    max: Option<usize>,
    required: bool,
    sometimes: bool,
    mixed_case: bool,
    letters: bool,
    numbers: bool,
    symbols: bool,
    uncompromised: bool,
    threshold: usize,
    custom: RuleSet,
}

impl std::fmt::Debug for Password {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Password")
            .field("rules", &self.applied_rules())
            .finish()
    }
}

impl Password {
    /// Require at least `size` characters (and at least one).
    pub fn min(size: usize) -> Self {
        Self {
            min: size.max(1),
            max: None,
            required: false,
            sometimes: false,
            mixed_case: false,
            letters: false,
            numbers: false,
            symbols: false,
            uncompromised: false,
            threshold: 0,
            custom: RuleSet::new(),
        }
    }

    /// Set the application's default password rule (usually in a service
    /// provider's `boot` method).
    ///
    /// ```
    /// use std::sync::Arc;
    /// use illuminate_container::Container;
    /// use illuminate_validation::Password;
    ///
    /// let container = Arc::new(Container::new());
    /// let _guard = Container::set_local_instance(container);
    ///
    /// Password::set_defaults(|| Password::min(12).mixed_case());
    /// assert_eq!(Password::defaults().to_password_rules_string(), "minlength: 12; required: lower; required: upper;");
    /// ```
    pub fn set_defaults(callback: impl Fn() -> Password + Send + Sync + 'static) {
        Container::get_instance().instance(PasswordDefaults(Arc::new(callback)));
    }

    /// The application's default password rule (`Password::min(8)` unless
    /// configured with [`Password::set_defaults`]).
    pub fn defaults() -> Password {
        match try_app::<PasswordDefaults>() {
            Some(defaults) => (defaults.0)(),
            None => Password::min(8),
        }
    }

    /// The default rule, marked as required.
    pub fn required() -> Password {
        let mut password = Self::defaults();
        password.required = true;
        password
    }

    /// The default rule, applied only when the field is present.
    pub fn sometimes() -> Password {
        let mut password = Self::defaults();
        password.sometimes = true;
        password
    }

    /// Allow at most `size` characters.
    pub fn max(mut self, size: usize) -> Self {
        self.max = Some(size);
        self
    }

    /// Require at least one uppercase and one lowercase letter.
    pub fn mixed_case(mut self) -> Self {
        self.mixed_case = true;
        self
    }

    /// Require at least one letter.
    pub fn letters(mut self) -> Self {
        self.letters = true;
        self
    }

    /// Require at least one number.
    pub fn numbers(mut self) -> Self {
        self.numbers = true;
        self
    }

    /// Require at least one symbol.
    pub fn symbols(mut self) -> Self {
        self.symbols = true;
        self
    }

    /// Ensure the password hasn't appeared in a data leak, using the bound
    /// `dyn UncompromisedVerifier` (a no-op when none is bound).
    pub fn uncompromised(mut self) -> Self {
        self.uncompromised = true;
        self
    }

    /// Like [`Password::uncompromised`], allowing up to `threshold` appearances.
    pub fn uncompromised_threshold(mut self, threshold: usize) -> Self {
        self.uncompromised = true;
        self.threshold = threshold;
        self
    }

    /// Add additional rules.
    pub fn rules(mut self, rules: impl Into<RuleSet>) -> Self {
        self.custom = rules.into();
        self
    }

    /// The configured constraints.
    pub fn applied_rules(&self) -> Value {
        json!({
            "min": self.min,
            "max": self.max,
            "mixedCase": self.mixed_case,
            "letters": self.letters,
            "numbers": self.numbers,
            "symbols": self.symbols,
            "uncompromised": self.uncompromised,
            "compromisedThreshold": self.threshold,
        })
    }

    /// The rule as an HTML `passwordrules` attribute value.
    pub fn to_password_rules_string(&self) -> String {
        let mut rules = vec![format!("minlength: {}", self.min)];
        if let Some(max) = self.max {
            rules.push(format!("maxlength: {max}"));
        }
        if self.mixed_case {
            rules.push("required: lower".into());
            rules.push("required: upper".into());
        } else if self.letters {
            rules.push("required: lower".into());
        }
        if self.numbers {
            rules.push("required: digit".into());
        }
        if self.symbols {
            rules.push("required: special".into());
        }
        format!("{};", rules.join("; "))
    }

    fn rule_set(&self) -> RuleSet {
        let mut items = Vec::new();
        if self.required {
            items.push(RuleItem::Str("required".into()));
        }
        if self.sometimes {
            items.push(RuleItem::Str("sometimes".into()));
        }
        items.push(RuleItem::Str("string".into()));
        items.push(RuleItem::Str(format!("min:{}", self.min)));
        if let Some(max) = self.max {
            items.push(RuleItem::Str(format!("max:{max}")));
        }
        items.extend(self.custom.items.iter().cloned());
        RuleSet::from_items(items)
    }
}

impl Conditionable for Password {}

static MIXED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(\p{Ll}+.*\p{Lu})|(\p{Lu}+.*\p{Ll})").unwrap());
static LETTERS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\pL").unwrap());
static SYMBOLS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\p{Z}|\p{S}|\p{P}").unwrap());
static NUMBERS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\pN").unwrap());

#[async_trait]
impl ValidationRule for Password {
    async fn validate_with(
        &self,
        _attribute: &str,
        value: &Value,
        context: &ValidationContext<'_>,
        fail: &mut FailCallback<'_>,
    ) {
        let attribute = context.attribute().to_string();
        if !self.required && !self.sometimes && !context.has(&attribute) {
            return;
        }
        if value.is_blank() && !self.required && context.has_rule(&attribute, &["Nullable"]) {
            return;
        }

        let mut validator = context.validator().nested(
            context.data().clone(),
            Rules::new().rule(attribute.clone(), self.rule_set()),
        );

        let password = value.as_str().map(str::to_string);
        let (mixed, letters, symbols, numbers) =
            (self.mixed_case, self.letters, self.symbols, self.numbers);
        let failing = attribute.clone();
        validator = validator.after(move |validator| {
            let Some(password) = &password else { return };
            if mixed && !MIXED.is_match(password) {
                validator.add_failure(&failing, "password.mixed", &[]);
            }
            if letters && !LETTERS.is_match(password) {
                validator.add_failure(&failing, "password.letters", &[]);
            }
            if symbols && !SYMBOLS.is_match(password) {
                validator.add_failure(&failing, "password.symbols", &[]);
            }
            if numbers && !NUMBERS.is_match(password) {
                validator.add_failure(&failing, "password.numbers", &[]);
            }
        });

        match validator.try_passes().await {
            Ok(true) => {}
            Ok(false) => {
                for message in validator.errors().all() {
                    fail(message);
                }
                return;
            }
            Err(error) => {
                context.abort(error);
                return;
            }
        }

        if self.uncompromised
            && let (Some(verifier), Some(password)) =
                (try_app::<dyn UncompromisedVerifier>(), value.as_str())
            && !verifier.verify(password, self.threshold).await
        {
            validator.add_failure(&attribute, "password.uncompromised", &[]);
            for message in validator.errors().all() {
                fail(message);
            }
        }
    }

    fn implicit(&self) -> bool {
        true
    }

    fn name(&self) -> String {
        "Password".to_string()
    }
}
