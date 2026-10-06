use std::sync::Arc;

use illuminate_encryption::{EncryptException, Encrypter, encrypter};
use illuminate_http::{Middleware, Next, Request, Response, async_trait};
use illuminate_support::Result;

use crate::prefix::CookieValuePrefix;

/// Encrypts every outgoing cookie and decrypts every incoming one, so
/// cookies can't be read or modified by the client.
///
/// Incoming cookies that fail to decrypt — or whose value prefix doesn't
/// match their name — are dropped from the request. Values are encrypted
/// exactly like Laravel (`encryptString` of `CookieValuePrefix . value`), so
/// cookies are interchangeable with a Laravel application sharing the key.
///
/// ```
/// use illuminate_cookie::EncryptCookies;
///
/// let middleware = EncryptCookies::new().except(["cookie_name"]);
/// assert!(middleware.is_disabled("cookie_name"));
/// assert!(!middleware.is_disabled("laravel_session"));
/// ```
#[derive(Clone, Debug, Default)]
pub struct EncryptCookies {
    except: Vec<String>,
    encrypter: Option<Arc<Encrypter>>,
}

impl EncryptCookies {
    /// Create the middleware, using the application's encrypter.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create the middleware with a specific encrypter.
    pub fn with_encrypter(encrypter: Arc<Encrypter>) -> Self {
        Self {
            except: Vec::new(),
            encrypter: Some(encrypter),
        }
    }

    /// The names of the cookies that should not be encrypted.
    pub fn except<I, S>(mut self, names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.except.extend(names.into_iter().map(Into::into));
        self
    }

    /// Disable encryption for the given cookie name.
    pub fn disable_for(&mut self, name: impl Into<String>) -> &mut Self {
        self.except.push(name.into());
        self
    }

    /// Determine whether encryption has been disabled for the given cookie.
    pub fn is_disabled(&self, name: &str) -> bool {
        self.except.iter().any(|except| except == name)
    }

    fn encrypter(&self) -> Result<Arc<Encrypter>> {
        match &self.encrypter {
            Some(encrypter) => Ok(encrypter.clone()),
            None => encrypter(),
        }
    }

    /// Decrypt the cookies on the request, dropping any that are invalid.
    pub fn decrypt(&self, request: &Request, encrypter: &Encrypter) {
        let keys = encrypter.get_all_keys();
        for (name, value) in request.cookies() {
            if self.is_disabled(&name) {
                continue;
            }
            let decrypted = encrypter
                .decrypt_string(&value)
                .ok()
                .and_then(|value| CookieValuePrefix::validate(&name, &value, &keys));
            match decrypted {
                Some(value) => request.set_cookie(&name, value),
                None => request.remove_cookie(&name),
            }
        }
    }

    /// Encrypt the cookies on an outgoing response.
    pub fn encrypt(
        &self,
        response: &mut Response,
        encrypter: &Encrypter,
    ) -> Result<(), EncryptException> {
        for cookie in response.cookies_mut() {
            if self.is_disabled(&cookie.name) {
                continue;
            }
            let prefixed = format!(
                "{}{}",
                CookieValuePrefix::create(&cookie.name, encrypter.get_key()),
                cookie.value
            );
            cookie.value = encrypter.encrypt_string(&prefixed)?;
        }
        Ok(())
    }
}

#[async_trait]
impl Middleware for EncryptCookies {
    async fn handle(&self, request: Request, next: Next) -> Result<Response> {
        let encrypter = self.encrypter()?;
        self.decrypt(&request, &encrypter);
        let mut response = next.run(request).await;
        self.encrypt(&mut response, &encrypter)?;
        Ok(response)
    }
}
