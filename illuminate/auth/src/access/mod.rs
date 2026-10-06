//! Authorization: gates and policies.
//!
//! Gates are closures that decide whether a user may perform an action;
//! policies group that logic around a model. Both are registered on the
//! [`AccessGate`] behind the `Gate` facade.

mod arguments;
mod callbacks;
mod facade;
mod gate;
mod policy;
mod response;

pub use arguments::{GateArgument, GateArguments, IntoAbilities, IntoGateArguments};
pub use callbacks::{
    Ability, AbilityFn, AfterCallback, AfterFn, BeforeCallback, BeforeFn, IntoGateResult,
};
pub use facade::{Authorizable, Gate, authorize, gate};
pub use gate::{AccessGate, UserGate};
pub use policy::Policy;
pub use response::{AuthResponse, AuthorizationException, IntoMessage};
