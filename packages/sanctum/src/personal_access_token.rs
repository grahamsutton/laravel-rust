//! The `PersonalAccessToken` model: one row of the `personal_access_tokens`
//! table.

use illuminate_auth::AuthUser;
use illuminate_database::eloquent::{KeyType, Model, MorphTo, Original};
use illuminate_support::{Carbon, Result, Value};

use crate::access_token::HasAbilities;
use crate::facade::state;
use crate::support::{hash_equals, sha256};

/// A personal access token (an API token) issued to a user.
///
/// Tokens are stored hashed (SHA-256); the plain-text value is only known
/// when the token is created (see
/// [`HasApiTokens::create_token`](crate::HasApiTokens::create_token)). The
/// `token` column is hidden when the model is serialized.
///
/// ```
/// use laravel_sanctum::PersonalAccessToken;
///
/// let token = PersonalAccessToken {
///     abilities: vec!["check-status".into(), "place-orders".into()],
///     ..Default::default()
/// };
///
/// assert!(token.can("check-status"));
/// assert!(token.cant("cancel-orders"));
///
/// let admin = PersonalAccessToken { abilities: vec!["*".into()], ..Default::default() };
/// assert!(admin.can("cancel-orders"));
/// ```
#[derive(Debug, Clone, Default, Model)]
#[table("personal_access_tokens")]
#[fillable(name, token, abilities, expires_at)]
#[hidden(token)]
pub struct PersonalAccessToken {
    /// The token's ID: the part before the `|` of a plain-text token.
    pub id: u64,
    /// The morph class of the model the token belongs to (`User`).
    pub tokenable_type: String,
    /// The key of the model the token belongs to.
    pub tokenable_id: Value,
    /// The token's name (`"token-name"`, `"Nuno's iPhone"`, ...).
    pub name: String,
    /// The SHA-256 hash of the token.
    pub token: String,
    /// The abilities granted to the token (`["*"]` grants everything).
    pub abilities: Vec<String>,
    /// When the token last authenticated a request.
    pub last_used_at: Option<Carbon>,
    /// When the token expires (`None` for never).
    pub expires_at: Option<Carbon>,
    /// When the token was created.
    pub created_at: Option<Carbon>,
    /// When the token was last updated.
    pub updated_at: Option<Carbon>,
    /// Dirty tracking, so updates only write what changed.
    pub original: Original,
}

impl PersonalAccessToken {
    /// Find the token instance matching the given plain-text token.
    ///
    /// Tokens of the form `{id}|{token}` are looked up by ID, and their
    /// hashes compared in constant time; bare tokens are looked up by hash.
    ///
    /// ```ignore
    /// let token = PersonalAccessToken::find_token(&request.bearer_token().unwrap()).await?;
    /// ```
    pub async fn find_token(token: &str) -> Result<Option<Self>> {
        let Some((id, token)) = token.split_once('|') else {
            return Self::where_("token", sha256(token)).first().await;
        };
        let Some(instance) = Self::find(id).await? else {
            return Ok(None);
        };
        Ok(hash_equals(&instance.token, &sha256(token)).then_some(instance))
    }

    /// Determine if the token has a given ability.
    pub fn can(&self, ability: &str) -> bool {
        self.abilities
            .iter()
            .any(|granted| granted == "*" || granted == ability)
    }

    /// Determine if the token is missing a given ability.
    pub fn cant(&self, ability: &str) -> bool {
        !self.can(ability)
    }

    /// Determine if the token has expired by its own `expires_at`.
    pub fn is_expired(&self) -> bool {
        self.expires_at
            .is_some_and(|expires_at| expires_at.is_past())
    }

    /// The `tokenable` relationship, for a known tokenable type.
    ///
    /// ```ignore
    /// let user: Option<User> = token.tokenable::<User>().get().await?;
    /// ```
    pub fn tokenable<M: Model>(&self) -> MorphTo<Self, M> {
        self.morph_to("tokenable")
    }

    /// The model the token belongs to, as an authenticated user — whatever
    /// its type. Types are resolved through the models Sanctum knows about
    /// (see [`Sanctum::use_tokenable_model`](crate::Sanctum::use_tokenable_model))
    /// and the application's resolver
    /// ([`Sanctum::resolve_tokenables_using`](crate::Sanctum::resolve_tokenables_using)).
    pub async fn tokenable_user(&self) -> Result<Option<AuthUser>> {
        Ok(self
            .resolve_tokenable()
            .await?
            .map(|tokenable| tokenable.user))
    }

    /// Resolve the token's owner along with its class name.
    pub(crate) async fn resolve_tokenable(&self) -> Result<Option<ResolvedTokenable>> {
        let state = state();
        if let Some(tokenable) = state.tokenable(&self.tokenable_type) {
            let user = tokenable.find(self.tokenable_id.clone()).await?;
            return Ok(user.map(|user| ResolvedTokenable {
                user,
                class_name: tokenable.class_name().to_string(),
            }));
        }
        if let Some(resolver) = state.tokenable_resolver() {
            let user = resolver(self.tokenable_type.clone(), self.tokenable_id.clone()).await?;
            return Ok(user.map(|user| ResolvedTokenable {
                user,
                class_name: self.tokenable_type.clone(),
            }));
        }
        // Otherwise, find the owner through the user provider configured
        // for its model (`auth.providers.*.model`).
        if let Some(user) = Self::resolve_through_user_providers(&self.tokenable_type, &self.tokenable_id).await? {
            return Ok(Some(ResolvedTokenable {
                user,
                class_name: self.tokenable_type.clone(),
            }));
        }
        Ok(None)
    }

    /// Retrieve the owner with the user provider whose `model` is the
    /// token's `tokenable_type` (compared by class basename).
    async fn resolve_through_user_providers(tokenable_type: &str, id: &Value) -> Result<Option<AuthUser>> {
        let basename = |name: &str| name.rsplit(['\\', ':']).next().unwrap_or(name).to_string();
        let wanted = basename(tokenable_type);
        let providers = match illuminate_config::config("auth.providers") {
            Value::Object(providers) => providers,
            _ => return Ok(None),
        };
        for (name, config) in providers {
            let model = config.get("model").and_then(Value::as_str).map(basename);
            if model.as_deref() != Some(wanted.as_str()) {
                continue;
            }
            if let Some(provider) = illuminate_auth::manager().create_user_provider(Some(&name))? {
                return provider.retrieve_by_id(id).await;
            }
        }
        Ok(None)
    }

    /// Whether the model's keys are integers (Laravel's `getKeyType() === 'int'`).
    pub(crate) fn has_integer_keys() -> bool {
        Self::key_type() == KeyType::Int
    }
}

impl HasAbilities for PersonalAccessToken {
    fn can(&self, ability: &str) -> bool {
        PersonalAccessToken::can(self, ability)
    }
}

/// The owner of a token, as an authenticated user.
pub(crate) struct ResolvedTokenable {
    pub(crate) user: AuthUser,
    pub(crate) class_name: String,
}
