use std::{io::BufReader, sync::RwLock};

use cookie::Cookie as RawCookie;
use cookie_store::{CookieDomain, CookieExpiration, CookieStore as Store};
use reqwest::header::HeaderValue;
use url::Url;

/// A cookie jar the application can inspect, edit and persist.
///
/// `reqwest::cookie::Jar` is write-only from the outside: received cookies
/// could never be shown, managed or saved across restarts. This jar wraps
/// `cookie_store` (the same implementation reqwest uses internally) behind
/// the `reqwest::cookie::CookieStore` trait.
#[derive(Debug, Default)]
pub struct PersistentCookieJar {
    store: RwLock<Store>,
}

/// One cookie as listed by the management UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredCookie {
    pub domain: String,
    pub path: String,
    pub name: String,
    pub value: String,
    /// Unix seconds of the expiry; `None` for session cookies.
    pub expires_at: Option<i64>,
    pub secure: bool,
    pub http_only: bool,
}

impl PersistentCookieJar {
    /// Restore a jar from an earlier [`Self::to_json`] payload. A broken
    /// payload yields an empty jar instead of a startup failure.
    pub fn from_json(json: &str) -> Self {
        let store =
            cookie_store::serde::json::load(BufReader::new(json.as_bytes())).unwrap_or_default();
        Self {
            store: RwLock::new(store),
        }
    }

    /// Serialize the persistent (non-session) cookies.
    pub fn to_json(&self) -> Option<String> {
        let mut buffer = Vec::new();
        let store = self.store.read().ok()?;
        cookie_store::serde::json::save(&store, &mut buffer).ok()?;
        drop(store);
        String::from_utf8(buffer).ok()
    }

    /// Every unexpired cookie, ordered by domain, path and name.
    pub fn list(&self) -> Vec<StoredCookie> {
        let Ok(store) = self.store.read() else {
            return Vec::new();
        };
        let mut cookies = store
            .iter_unexpired()
            .map(|cookie| StoredCookie {
                domain: domain_label(&cookie.domain, cookie.domain().unwrap_or_default()),
                path: (*cookie.path).to_owned(),
                name: cookie.name().to_owned(),
                value: cookie.value().to_owned(),
                expires_at: match cookie.expires {
                    CookieExpiration::AtUtc(at) => Some(at.unix_timestamp()),
                    CookieExpiration::SessionEnd => None,
                },
                secure: cookie.secure().unwrap_or(false),
                http_only: cookie.http_only().unwrap_or(false),
            })
            .collect::<Vec<_>>();
        cookies.sort_by(|left, right| {
            (&left.domain, &left.path, &left.name).cmp(&(&right.domain, &right.path, &right.name))
        });
        cookies
    }

    /// Remove one cookie by the identity shown in [`Self::list`].
    pub fn remove(&self, domain: &str, path: &str, name: &str) {
        if let Ok(mut store) = self.store.write() {
            store.remove(domain, path, name);
        }
    }

    pub fn clear(&self) {
        if let Ok(mut store) = self.store.write() {
            store.clear();
        }
    }
}

fn domain_label(domain: &CookieDomain, raw: &str) -> String {
    match domain {
        CookieDomain::HostOnly(host) => host.clone(),
        CookieDomain::Suffix(suffix) => suffix.clone(),
        CookieDomain::NotPresent | CookieDomain::Empty => raw.to_owned(),
    }
}

impl reqwest::cookie::CookieStore for PersistentCookieJar {
    fn set_cookies(&self, cookie_headers: &mut dyn Iterator<Item = &HeaderValue>, url: &Url) {
        let Ok(mut store) = self.store.write() else {
            return;
        };
        let cookies = cookie_headers.filter_map(|value| {
            std::str::from_utf8(value.as_bytes())
                .ok()
                .and_then(|value| RawCookie::parse(value.to_owned()).ok())
        });
        store.store_response_cookies(cookies, url);
    }

    fn cookies(&self, url: &Url) -> Option<HeaderValue> {
        let store = self.store.read().ok()?;
        let header = store
            .get_request_values(url)
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join("; ");
        if header.is_empty() {
            return None;
        }
        HeaderValue::from_str(&header).ok()
    }
}

#[cfg(test)]
mod tests {
    use reqwest::cookie::CookieStore as _;

    use super::*;

    #[test]
    fn stores_lists_serializes_and_removes_cookies() {
        let jar = PersistentCookieJar::default();
        let url = Url::parse("https://api.example.com/login").expect("test URL should parse");
        let headers = [
            HeaderValue::from_static("session=abc123; Path=/; HttpOnly; Max-Age=3600; Secure"),
            HeaderValue::from_static("theme=dark; Path=/"),
        ];
        jar.set_cookies(&mut headers.iter(), &url);

        let sent = jar.cookies(&url).expect("cookies should be returned");
        let sent = sent.to_str().expect("header should be ASCII");
        assert!(sent.contains("session=abc123"));
        assert!(sent.contains("theme=dark"));

        let listed = jar.list();
        assert_eq!(listed.len(), 2);
        let session = listed
            .iter()
            .find(|cookie| cookie.name == "session")
            .expect("session cookie should be listed");
        assert_eq!(session.domain, "api.example.com");
        assert!(session.secure && session.http_only);
        assert!(session.expires_at.is_some());

        // Only the persistent cookie survives the JSON round-trip.
        let json = jar.to_json().expect("jar should serialize");
        let restored = PersistentCookieJar::from_json(&json);
        let restored = restored.list();
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].name, "session");

        jar.remove(&session.domain, &session.path, &session.name);
        assert_eq!(jar.list().len(), 1);
        jar.clear();
        assert!(jar.list().is_empty());
    }
}
