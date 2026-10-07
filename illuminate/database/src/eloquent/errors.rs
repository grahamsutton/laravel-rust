//! Eloquent's exceptions.

use illuminate_support::{Value, ValueExt};

/// No model matched the query (`find_or_fail`, `first_or_fail`, route
/// model binding, ...). The HTTP kernel renders it as a 404.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{}", self.message())]
pub struct ModelNotFoundException {
    /// The model's class name (`User`).
    pub model: String,
    /// The keys that were looked up, if any.
    pub ids: Vec<Value>,
}

impl ModelNotFoundException {
    /// A "not found" error for the model class and keys.
    pub fn new(model: impl Into<String>, ids: Vec<Value>) -> Self {
        Self {
            model: model.into(),
            ids,
        }
    }

    /// The model's class name.
    pub fn get_model(&self) -> &str {
        &self.model
    }

    /// The keys that were looked up.
    pub fn get_ids(&self) -> &[Value] {
        &self.ids
    }

    fn message(&self) -> String {
        if self.ids.is_empty() {
            format!("No query results for model [{}].", self.model)
        } else {
            let ids: Vec<String> = self.ids.iter().map(|id| id.to_string_lossy()).collect();
            format!(
                "No query results for model [{}] {}",
                self.model,
                ids.join(", ")
            )
        }
    }
}

/// An attribute was mass assigned on a model that guards it.
#[derive(Debug, Clone, thiserror::Error)]
#[error("Add [{key}] to fillable property to allow mass assignment on [{model}].")]
pub struct MassAssignmentException {
    /// The attribute that was assigned.
    pub key: String,
    /// The model's class name.
    pub model: String,
}

impl MassAssignmentException {
    /// A mass assignment error for the attribute on the model class.
    pub fn new(key: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            model: model.into(),
        }
    }
}

/// A relationship that the model doesn't define was requested.
#[derive(Debug, Clone, thiserror::Error)]
#[error("Call to undefined relationship [{relation}] on model [{model}].")]
pub struct RelationNotFoundException {
    /// The model's class name.
    pub model: String,
    /// The relationship that was requested.
    pub relation: String,
}

impl RelationNotFoundException {
    /// A "relationship not found" error.
    pub fn new(model: impl Into<String>, relation: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            relation: relation.into(),
        }
    }
}

/// A polymorphic type was written or queried for a model missing from the
/// required morph map.
#[derive(Debug, Clone, thiserror::Error)]
#[error("No morph map defined for model [{model}].")]
pub struct ClassMorphViolationException {
    /// The model's class name.
    pub model: String,
}

impl ClassMorphViolationException {
    /// A morph map violation for the model class.
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
        }
    }
}

/// An attribute that doesn't exist (or wasn't retrieved) was accessed while
/// `prevent_accessing_missing_attributes` is on.
#[derive(Debug, Clone, thiserror::Error)]
#[error("The attribute [{key}] either does not exist or was not retrieved for model [{model}].")]
pub struct MissingAttributeException {
    /// The model's class name.
    pub model: String,
    /// The attribute that was accessed.
    pub key: String,
}

impl MissingAttributeException {
    /// A missing attribute error for the model class and key.
    pub fn new(model: impl Into<String>, key: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            key: key.into(),
        }
    }
}
