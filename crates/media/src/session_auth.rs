//! Explicit, in-memory credentials. These types deliberately do not serialize.
use std::{fmt, time::SystemTime};

use reqwest::{RequestBuilder, header::COOKIE};
use url::Url;

use crate::{MediaError, network_url};

#[derive(Clone)]
pub enum MediaAuthorization {
    Basic { username: String, password: String },
    Bearer(String),
}

/// A cookie can only reach its enclosing exact origin; subdomain grants are
/// intentionally unsupported. Path and Secure impose additional restrictions.
#[derive(Clone)]
pub struct ScopedCookie {
    pub name: String,
    pub value: String,
    pub path: String,
    pub secure: bool,
    pub expires_at: Option<SystemTime>,
}

#[derive(Clone)]
pub struct OriginCredentials {
    origin: Url,
    authorization: Option<MediaAuthorization>,
    cookies: Vec<ScopedCookie>,
}

#[derive(Clone, Default)]
pub struct MediaSessionAuth {
    origins: Vec<OriginCredentials>,
    scope: String,
}

macro_rules! redacted_debug {
    ($($ty:ty),+) => {$(
        impl fmt::Debug for $ty {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(concat!(stringify!($ty), "(<redacted>)"))
            }
        }
    )+};
}
redacted_debug!(
    MediaAuthorization,
    ScopedCookie,
    OriginCredentials,
    MediaSessionAuth
);

fn safe_value(value: &str) -> bool {
    value.len() <= 8 * 1024 && value.bytes().all(|b| (0x20..0x7f).contains(&b))
}

impl OriginCredentials {
    pub fn new(
        origin: &str,
        authorization: Option<MediaAuthorization>,
        cookies: Vec<ScopedCookie>,
    ) -> Result<Self, MediaError> {
        let origin = network_url(origin).map_err(|_| MediaError::InvalidAuth)?;
        if origin.path() != "/"
            || origin.query().is_some()
            || origin.fragment().is_some()
            || cookies.len() > 64
        {
            return Err(MediaError::InvalidAuth);
        }
        match &authorization {
            Some(MediaAuthorization::Basic { username, password })
                if !safe_value(username) || username.contains(':') || !safe_value(password) =>
            {
                return Err(MediaError::InvalidAuth);
            }
            Some(MediaAuthorization::Bearer(token))
                if token.is_empty() || !safe_value(token) || token.contains(' ') =>
            {
                return Err(MediaError::InvalidAuth);
            }
            _ => {}
        }
        if cookies
            .iter()
            .map(|cookie| cookie.name.len() + cookie.value.len() + 3)
            .sum::<usize>()
            > 32 * 1024
        {
            return Err(MediaError::InvalidAuth);
        }
        for cookie in &cookies {
            if cookie.name.is_empty()
                || cookie.name.len() > 256
                || !cookie
                    .name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
                || !safe_value(&cookie.value)
                || cookie
                    .value
                    .bytes()
                    .any(|b| b <= 0x20 || b"\",;\\".contains(&b))
                || !cookie.path.starts_with('/')
                || !safe_value(&cookie.path)
            {
                return Err(MediaError::InvalidAuth);
            }
        }
        Ok(Self {
            origin,
            authorization,
            cookies,
        })
    }

    fn apply(&self, mut request: RequestBuilder, url: &Url) -> RequestBuilder {
        if self.origin.origin() != url.origin() {
            return request;
        }
        request = match &self.authorization {
            Some(MediaAuthorization::Basic { username, password }) => {
                request.basic_auth(username, Some(password))
            }
            Some(MediaAuthorization::Bearer(token)) => request.bearer_auth(token),
            None => request,
        };
        let mut cookies: Vec<_> = self
            .cookies
            .iter()
            .filter(|cookie| {
                (!cookie.secure || url.scheme() == "https")
                    && cookie
                        .expires_at
                        .is_none_or(|expires| expires > SystemTime::now())
                    && (url.path() == cookie.path
                        || (url.path().starts_with(&cookie.path)
                            && (cookie.path.ends_with('/')
                                || url.path().as_bytes().get(cookie.path.len()) == Some(&b'/'))))
            })
            .collect();
        cookies.sort_by_key(|cookie| std::cmp::Reverse(cookie.path.len()));
        if !cookies.is_empty() {
            let value = cookies
                .iter()
                .map(|cookie| format!("{}={}", cookie.name, cookie.value))
                .collect::<Vec<_>>()
                .join("; ");
            request = request.header(COOKIE, value);
        }
        request
    }
}

impl MediaSessionAuth {
    pub fn new(origins: Vec<OriginCredentials>) -> Result<Self, MediaError> {
        if origins.len() > 32
            || origins.iter().enumerate().any(|(index, item)| {
                origins[..index]
                    .iter()
                    .any(|previous| previous.origin.origin() == item.origin.origin())
            })
        {
            return Err(MediaError::InvalidAuth);
        }
        Ok(Self { origins, scope: uuid::Uuid::new_v4().to_string() })
    }

    /// Random per-session cache partition; never derives a key from credentials.
    pub(crate) fn cache_scope(&self) -> &str {
        if self.is_empty() { "public" } else { &self.scope }
    }

    pub fn is_empty(&self) -> bool {
        self.origins.is_empty()
    }

    pub(crate) fn apply(&self, request: RequestBuilder, url: &Url) -> RequestBuilder {
        match self
            .origins
            .iter()
            .find(|entry| entry.origin.origin() == url.origin())
        {
            Some(entry) => entry.apply(request, url),
            None => request,
        }
    }
}
