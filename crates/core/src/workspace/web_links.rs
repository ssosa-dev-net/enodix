//! Web links: a host's named links to web pages about it (its own GUI, the
//! controller that manages it), opened in the system's default browser.
//!
//! A link is a name and a URL template, set in the `web_links` setting of a
//! host or a folder and merged by name down the tree (see
//! [`super::EffectiveSettings::web_links`]). Its variables are filled from
//! the host's fields as snippets are ([`crate::template`]); values go in as
//! they are, and the filled text must then be an `http` or `https` URL.
//! Anything else (`file:`, `javascript:`, a bare address) is refused, so a
//! workspace file can never make the app run a program or a script.
//!
//! The app only hands the URL to the browser: it shows no page itself and
//! keeps no web logins (a URL holding a password is refused).
//!
//! Values are trusted workspace data: a value holding `/`, `?`, `#` or `@`
//! changes the URL's structure (it is not percent-encoded), so that a
//! variable can fill a whole host, path or query.

use serde::Serialize;

use crate::template::{MissingVariables, Variables};

use super::{Origin, ResolvedHost, WebLink};

/// The URL schemes a link may open.
const WEB_SCHEMES: [&str; 2] = ["http", "https"];

/// Why a web link is not opened.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WebLinkError {
    /// The URL uses variables the host has no value for.
    #[error("the host has no value for {}", .0.names.join(", "))]
    Missing(MissingVariables),
    /// The filled URL is not a URL at all.
    #[error("\"{url}\" is not a web address: {reason}")]
    NotAUrl { url: String, reason: String },
    /// The filled URL is not an http or https one.
    #[error("only http and https links are opened, not {scheme}: {url}")]
    NotWeb { url: String, scheme: String },
    /// The filled URL carries a password (`user:password@`); it is not
    /// repeated in the message.
    #[error("a link may not hold a password: the app keeps no web logins")]
    HoldsPassword,
    /// No link of that name applies to the host.
    #[error("the host has no web link named {0}")]
    NoSuchLink(String),
}

/// `text` as the URL a browser is given, if it is an `http` or `https` one
/// with a host: blanks around it dropped, the scheme and host in lower case,
/// characters a URL cannot hold (such as spaces) percent-encoded.
pub fn web_url(text: &str) -> Result<String, WebLinkError> {
    let text = text.trim();
    let url = url::Url::parse(text).map_err(|error| WebLinkError::NotAUrl {
        url: text.to_string(),
        reason: error.to_string(),
    })?;
    if !WEB_SCHEMES.contains(&url.scheme()) {
        return Err(WebLinkError::NotWeb {
            url: text.to_string(),
            scheme: url.scheme().to_string(),
        });
    }
    if url.password().is_some() {
        return Err(WebLinkError::HoldsPassword);
    }
    // The URL parser already refuses an http(s) URL without a host.
    Ok(url.into())
}

impl WebLink {
    /// The URL this link opens with its variables filled from `values`, if
    /// they all have one and the result is a web address ([`web_url`]).
    pub fn open_url(&self, values: &Variables) -> Result<String, WebLinkError> {
        let filled = self.fill(values).map_err(WebLinkError::Missing)?;
        web_url(&filled)
    }
}

/// One of a host's web links, as the side panel offers it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostWebLink {
    pub name: String,
    /// The URL as written, with its variables.
    pub template: String,
    /// The host's own file, or the folder it is inherited from.
    pub origin: Origin,
    /// What clicking it opens, or why it is not opened.
    pub url: Result<String, WebLinkError>,
}

impl Serialize for HostWebLink {
    /// `url` is the URL opened, or `null` with `refused` saying why not.
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut link = serializer.serialize_struct("HostWebLink", 5)?;
        link.serialize_field("name", &self.name)?;
        link.serialize_field("template", &self.template)?;
        link.serialize_field("origin", &self.origin)?;
        link.serialize_field("url", &self.url.as_ref().ok())?;
        link.serialize_field("refused", &self.url.as_ref().err().map(ToString::to_string))?;
        link.end()
    }
}

impl ResolvedHost {
    /// The web links that apply to this host (its own and the inherited
    /// ones, merged by name), each filled from its fields and checked.
    pub fn web_links(&self) -> Vec<HostWebLink> {
        let values = self.variables();
        self.settings
            .web_links
            .iter()
            .map(|link| HostWebLink {
                name: link.value.name.clone(),
                template: link.value.url.clone(),
                origin: link.origin.clone(),
                url: link.value.open_url(&values),
            })
            .collect()
    }

    /// The URL the web link named `name` opens (names match as written, as
    /// they do when links are merged).
    pub fn web_link_url(&self, name: &str) -> Result<String, WebLinkError> {
        let link = self
            .settings
            .web_links
            .iter()
            .find(|link| link.value.name == name)
            .ok_or_else(|| WebLinkError::NoSuchLink(name.to_string()))?;
        link.value.open_url(&self.variables())
    }
}
