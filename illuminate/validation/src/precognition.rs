//! Validating precognitive requests (Laravel Precognition).
//!
//! A precognitive request may list the only attributes it wants validated
//! in its `Precognition-Validate-Only` header — the fields a user has
//! touched so far. Those are the only rules that run, and once they pass,
//! the request is answered with `204 No Content` right away.

use illuminate_http::{Precognition, Request};
use illuminate_support::Result;

use crate::validator::Validator;

/// Prepare a validator for the request: when it is precognitive, only the
/// attributes listed in its `Precognition-Validate-Only` header are
/// validated.
pub(crate) fn prepare(validator: Validator, request: &Request) -> Validator {
    if !validates_only_some_attributes(request) {
        return validator;
    }
    let request = request.clone();
    validator
        .filter_rules(move |attribute| request.should_validate_precognitive_attribute(attribute))
}

/// Called once validation passed — Laravel's
/// `Precognition::afterValidationHook`: a precognitive request validating
/// only some attributes is done, so it ends with a successful `204`.
pub(crate) fn passed(request: &Request) -> Result<()> {
    if validates_only_some_attributes(request) {
        return Precognition::abort_successfully();
    }
    Ok(())
}

fn validates_only_some_attributes(request: &Request) -> bool {
    request.is_precognitive() && request.has_header(Precognition::VALIDATE_ONLY_HEADER)
}
