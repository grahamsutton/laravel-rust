use std::sync::OnceLock;

use laravel::eloquent::FactoryBuilder;
use laravel::prelude::*;

use crate::app::models::User;

/// The current password being used by the factory.
static PASSWORD: OnceLock<String> = OnceLock::new();

#[derive(Default)]
pub struct UserFactory;

impl Factory for UserFactory {
    type Model = User;

    /// Define the model's default state.
    fn definition(&self, faker: &mut Faker) -> Value {
        json!({
            "name": faker.name(),
            "email": faker.unique(|faker| faker.safe_email()),
            "email_verified_at": now(),
            "password": PASSWORD.get_or_init(|| Hash::make("password").expect("Unable to hash the password")),
            "remember_token": Str::random(10),
        })
    }
}

/// The factory's states.
pub trait UserFactoryStates {
    /// Indicate that the model's email address should be unverified.
    fn unverified(self) -> Self;
}

impl UserFactoryStates for FactoryBuilder<UserFactory> {
    fn unverified(self) -> Self {
        self.state(json!({ "email_verified_at": null }))
    }
}
