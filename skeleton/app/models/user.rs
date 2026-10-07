use laravel::prelude::*;

use crate::database::factories::UserFactory;

#[derive(Debug, Clone, Default, Model, Authenticatable, Notifiable)]
#[use_factory(UserFactory)]
#[fillable(name, email, password)]
#[hidden(password, remember_token)]
pub struct User {
    pub id: u64,
    pub name: String,
    pub email: String,
    pub email_verified_at: Option<Carbon>,
    #[hashed]
    pub password: String,
    pub remember_token: Option<String>,
    pub created_at: Option<Carbon>,
    pub updated_at: Option<Carbon>,
    pub original: Original,
}
