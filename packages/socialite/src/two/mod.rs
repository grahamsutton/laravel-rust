//! OAuth 2 (Laravel's `Laravel\Socialite\Two` namespace): the [`Provider`],
//! the [`OAuth2Provider`] drivers, and the [`User`] and [`Token`] they
//! return.

mod bitbucket;
mod facebook;
mod github;
mod gitlab;
mod google;
mod linkedin_openid;
mod provider;
mod slack;
mod slack_openid;
mod token;
mod user;
mod x;

pub use bitbucket::BitbucketProvider;
pub use facebook::FacebookProvider;
pub use github::GithubProvider;
pub use gitlab::GitlabProvider;
pub use google::GoogleProvider;
pub use linkedin_openid::LinkedInOpenIdProvider;
pub use provider::{OAuth2Provider, Provider};
pub use slack::SlackProvider;
pub use slack_openid::SlackOpenIdProvider;
pub use token::Token;
pub use user::User;
pub use x::{TwitterProvider, XProvider};

pub use crate::exceptions::InvalidStateException;
