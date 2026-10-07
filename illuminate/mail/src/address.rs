//! Email addresses and the things that can be turned into them.

use std::fmt;

use serde::{Deserialize, Serialize};

/// An email address, with an optional display name.
///
/// ```
/// use illuminate_mail::Address;
///
/// let address = Address::new("jeffrey@example.com", "Jeffrey Way");
///
/// assert_eq!(address.to_string(), "Jeffrey Way <jeffrey@example.com>");
/// assert_eq!(Address::from("taylor@example.com").name, None);
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Address {
    /// The recipient's email address.
    pub address: String,
    /// The recipient's name.
    pub name: Option<String>,
}

impl Address {
    /// Create a new address with a display name. An empty name means "no name".
    pub fn new(address: impl Into<String>, name: impl Into<String>) -> Self {
        let name = name.into();
        Self {
            address: address.into(),
            name: (!name.is_empty()).then_some(name),
        }
    }

    /// Create an address without a display name.
    pub fn email(address: impl Into<String>) -> Self {
        Self {
            address: address.into(),
            name: None,
        }
    }

    /// The domain part of the address (`example.com`).
    pub fn domain(&self) -> &str {
        self.address
            .rsplit_once('@')
            .map_or("", |(_, domain)| domain)
    }

    /// Determine if this address matches the given address (case-insensitively),
    /// and the given name when one is provided.
    pub fn matches(&self, address: &str, name: Option<&str>) -> bool {
        self.address.eq_ignore_ascii_case(address)
            && name.is_none_or(|name| self.name.as_deref() == Some(name))
    }
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.name {
            Some(name) => write!(f, "{name} <{}>", self.address),
            None => f.write_str(&self.address),
        }
    }
}

impl From<&str> for Address {
    fn from(address: &str) -> Self {
        Address::email(address)
    }
}

impl From<String> for Address {
    fn from(address: String) -> Self {
        Address::email(address)
    }
}

impl From<&String> for Address {
    fn from(address: &String) -> Self {
        Address::email(address.clone())
    }
}

impl From<&Address> for Address {
    fn from(address: &Address) -> Self {
        address.clone()
    }
}

impl<A: Into<String>, N: Into<String>> From<(A, N)> for Address {
    fn from((address, name): (A, N)) -> Self {
        Address::new(address, name)
    }
}

/// Anything that may receive mail: your `User` model, for example.
///
/// Implement it to hand your models straight to `Mail::to(...)`, just like
/// passing a user in Laravel (which reads its `email` and `name`):
///
/// ```
/// use illuminate_mail::{IntoAddresses, MailRecipient};
///
/// struct User { name: String, email: String }
///
/// impl MailRecipient for User {
///     fn mail_address(&self) -> String { self.email.clone() }
///     fn mail_name(&self) -> Option<String> { Some(self.name.clone()) }
/// }
///
/// let user = User { name: "Taylor".into(), email: "taylor@laravel.com".into() };
/// let addresses = (&user).into_addresses();
///
/// assert_eq!(addresses[0].to_string(), "Taylor <taylor@laravel.com>");
/// ```
pub trait MailRecipient {
    /// The recipient's email address.
    fn mail_address(&self) -> String;

    /// The recipient's display name.
    fn mail_name(&self) -> Option<String> {
        None
    }

    /// The recipient's preferred locale (Laravel's `HasLocalePreference`).
    fn preferred_locale(&self) -> Option<String> {
        None
    }
}

impl MailRecipient for Address {
    fn mail_address(&self) -> String {
        self.address.clone()
    }

    fn mail_name(&self) -> Option<String> {
        self.name.clone()
    }
}

/// One or more recipients: strings, [`Address`]es, `(email, name)` tuples,
/// [`MailRecipient`]s, or lists of any of them.
pub trait IntoAddresses {
    /// Convert into a list of addresses.
    fn into_addresses(self) -> Vec<Address>;

    /// The preferred locale of the recipient, if it has one.
    fn preferred_locale(&self) -> Option<String> {
        None
    }
}

impl IntoAddresses for &str {
    fn into_addresses(self) -> Vec<Address> {
        vec![Address::email(self)]
    }
}

impl IntoAddresses for String {
    fn into_addresses(self) -> Vec<Address> {
        vec![Address::email(self)]
    }
}

impl IntoAddresses for &String {
    fn into_addresses(self) -> Vec<Address> {
        vec![Address::email(self.clone())]
    }
}

impl IntoAddresses for Address {
    fn into_addresses(self) -> Vec<Address> {
        vec![self]
    }
}

impl<T: MailRecipient + ?Sized> IntoAddresses for &T {
    fn into_addresses(self) -> Vec<Address> {
        vec![Address {
            address: self.mail_address(),
            name: self.mail_name().filter(|name| !name.is_empty()),
        }]
    }

    fn preferred_locale(&self) -> Option<String> {
        MailRecipient::preferred_locale(*self)
    }
}

impl<A: Into<String>, N: Into<String>> IntoAddresses for (A, N) {
    fn into_addresses(self) -> Vec<Address> {
        vec![Address::new(self.0, self.1)]
    }
}

impl<T: IntoAddresses> IntoAddresses for Vec<T> {
    fn into_addresses(self) -> Vec<Address> {
        self.into_iter()
            .flat_map(IntoAddresses::into_addresses)
            .collect()
    }

    fn preferred_locale(&self) -> Option<String> {
        match self.as_slice() {
            [single] => single.preferred_locale(),
            _ => None,
        }
    }
}

impl<T: IntoAddresses, const N: usize> IntoAddresses for [T; N] {
    fn into_addresses(self) -> Vec<Address> {
        self.into_iter()
            .flat_map(IntoAddresses::into_addresses)
            .collect()
    }

    fn preferred_locale(&self) -> Option<String> {
        match self.as_slice() {
            [single] => single.preferred_locale(),
            _ => None,
        }
    }
}

impl<T: IntoAddresses + Clone> IntoAddresses for &[T] {
    fn into_addresses(self) -> Vec<Address> {
        self.iter()
            .cloned()
            .flat_map(IntoAddresses::into_addresses)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct User {
        email: &'static str,
        locale: &'static str,
    }

    impl MailRecipient for User {
        fn mail_address(&self) -> String {
            self.email.into()
        }

        fn preferred_locale(&self) -> Option<String> {
            Some(self.locale.into())
        }
    }

    #[test]
    fn many_things_convert_into_addresses() {
        assert_eq!("a@b.com".into_addresses(), vec![Address::email("a@b.com")]);
        assert_eq!(
            ("a@b.com", "A").into_addresses(),
            vec![Address::new("a@b.com", "A")]
        );
        assert_eq!(vec!["a@b.com", "c@d.com"].into_addresses().len(), 2);
        let user = User {
            email: "taylor@laravel.com",
            locale: "es",
        };
        assert_eq!((&user).into_addresses()[0].address, "taylor@laravel.com");
        assert_eq!(
            IntoAddresses::preferred_locale(&&user).as_deref(),
            Some("es")
        );
        assert_eq!(vec![&user].preferred_locale().as_deref(), Some("es"));
        let list = [Address::email("x@y.com")];
        assert_eq!(list.as_slice().into_addresses().len(), 1);
    }

    #[test]
    fn addresses_format_and_match() {
        let address = Address::new("Taylor@Laravel.com", "Taylor");
        assert_eq!(address.to_string(), "Taylor <Taylor@Laravel.com>");
        assert_eq!(address.domain(), "Laravel.com");
        assert!(address.matches("taylor@laravel.com", None));
        assert!(address.matches("taylor@laravel.com", Some("Taylor")));
        assert!(!address.matches("taylor@laravel.com", Some("Abigail")));
        assert_eq!(Address::new("a@b.com", "").name, None);
    }
}
