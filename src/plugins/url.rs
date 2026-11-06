//! A URL typed in the regular search, like `github.com/foo` or
//! `localhost:3000`, opens in the browser from the top result.
//!
//! Only what's clearly a URL: a scheme, `www.`, localhost, an IP address, or a
//! domain whose ending is a common one. Endings that are also file extensions,
//! like `.rs` or `.md`, need a path after them: `docs.rs/serde` is a URL, but
//! `main.rs` isn't. Typing `https://` in front always works.

use crate::plugin::Action;
use crate::plugin::Effect;
use crate::plugin::Icon;
use crate::plugin::Item;
use crate::plugin::Plugin;
use crate::plugin::Query;

const ICON: &str = "internet-web-browser";

/// Domain endings that are taken as a URL on their own.
const COMMON: &[&str] = &[
    "ai", "app", "be", "blog", "ca", "ch", "cloud", "co", "com", "de", "dev", "edu", "es", "eu", "fm", "fr", "gg",
    "gov", "info", "io", "it", "jp", "me", "net", "news", "nl", "org", "page", "site", "tech", "to", "tv", "uk", "us",
    "wiki", "xyz",
];

/// Domain endings that are also file extensions: a URL only with a path after.
const AMBIGUOUS: &[&str] = &["am", "cc", "in", "md", "mk", "ml", "mov", "pl", "pm", "ps", "py", "rs", "sh", "so", "tf", "zip"];

pub struct Url;

/// What to open for what's typed, if it's clearly a URL.
fn detect(text: &str) -> Option<String> {
    if text.is_empty() || text.chars().any(char::is_whitespace) {
        return None;
    }
    let lower = text.to_ascii_lowercase();
    for scheme in ["http://", "https://"] {
        if let Some(rest) = lower.strip_prefix(scheme) {
            let host = rest.split(['/', '?', '#']).next().unwrap_or("");
            return (!host.is_empty()).then(|| text.to_string());
        }
    }
    let end = lower.find(['/', '?', '#']).unwrap_or(lower.len());
    let (authority, rest) = lower.split_at(end);
    let (host, port) = match authority.split_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (authority, None),
    };
    if port.is_some_and(|p| p.is_empty() || !p.chars().all(|c| c.is_ascii_digit())) {
        return None;
    }
    // Local servers are rarely on https.
    if host == "localhost" || is_ipv4(host) {
        return Some(format!("http://{text}"));
    }
    let labels: Vec<&str> = host.split('.').collect();
    if labels.len() < 2 || !labels.iter().all(|l| is_label(l)) {
        return None;
    }
    let tld = labels[labels.len() - 1];
    let has_path = rest.len() > 1;
    let known = COMMON.contains(&tld) || (has_path && AMBIGUOUS.contains(&tld));
    (known || labels[0] == "www").then(|| format!("https://{text}"))
}

fn is_label(label: &str) -> bool {
    !label.is_empty()
        && !label.starts_with('-')
        && !label.ends_with('-')
        && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

fn is_ipv4(host: &str) -> bool {
    let parts: Vec<&str> = host.split('.').collect();
    parts.len() == 4 && parts.iter().all(|p| !p.is_empty() && p.len() <= 3 && p.parse::<u8>().is_ok())
}

impl Plugin for Url {
    fn id(&self) -> &'static str {
        "url"
    }

    fn regular(&self) -> bool {
        true
    }

    fn query(&self, query: &Query) {
        if query.command.is_some() {
            return;
        }
        let Some(url) = detect(query.text.trim()) else { return };
        let item = Item::new("url", &url, "Open in the browser", Icon::Theme(ICON.into()))
            .action(Action::new("run", "Open", Effect::Open(url.clone())))
            .action(Action::new("copy", "Copy the link", Effect::Copy(url)))
            .score(2.0);
        query.send(vec![item]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domains_get_https() {
        assert_eq!(detect("github.com").as_deref(), Some("https://github.com"));
        assert_eq!(detect("github.com/Sigma-studios/genie").as_deref(), Some("https://github.com/Sigma-studios/genie"));
        assert_eq!(detect("news.ycombinator.com").as_deref(), Some("https://news.ycombinator.com"));
        assert_eq!(detect("lemonde.fr?x=1").as_deref(), Some("https://lemonde.fr?x=1"));
        assert_eq!(detect("www.example.lol").as_deref(), Some("https://www.example.lol"));
        assert_eq!(detect("GitHub.com").as_deref(), Some("https://GitHub.com"));
    }

    #[test]
    fn a_scheme_is_kept_as_is() {
        assert_eq!(detect("https://main.rs").as_deref(), Some("https://main.rs"));
        assert_eq!(detect("http://example.lol/a").as_deref(), Some("http://example.lol/a"));
        assert_eq!(detect("https://"), None);
    }

    #[test]
    fn local_servers_get_http() {
        assert_eq!(detect("localhost:3000").as_deref(), Some("http://localhost:3000"));
        assert_eq!(detect("localhost:8080/api").as_deref(), Some("http://localhost:8080/api"));
        assert_eq!(detect("192.168.1.1").as_deref(), Some("http://192.168.1.1"));
        assert_eq!(detect("127.0.0.1:5173").as_deref(), Some("http://127.0.0.1:5173"));
    }

    #[test]
    fn file_extensions_need_a_path() {
        assert_eq!(detect("docs.rs/serde").as_deref(), Some("https://docs.rs/serde"));
        for text in ["main.rs", "README.md", "install.sh", "setup.py", "docs.rs", "docs.rs/"] {
            assert_eq!(detect(text), None, "{text}");
        }
    }

    #[test]
    fn what_isnt_a_url_is_refused() {
        let refused = [
            "", "firefox", "config.toml", "Cargo.lock", "src/main.rs", "~/notes.txt", "github.c", "github .com",
            "hello world.com", "me@example.com", "localhost:", "localhost:abc", "1.2.3", "256.1.1.1", "0.1+0.2",
            "-a.com", ".com", "a..com",
        ];
        for text in refused {
            assert_eq!(detect(text), None, "{text}");
        }
    }
}
