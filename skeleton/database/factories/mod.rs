//! Model factories: `User::factory().count(3).create().await?`.

pub mod user_factory;

pub use user_factory::{UserFactory, UserFactoryStates};
