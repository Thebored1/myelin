//! Web search for the agent's `web_search` tool. Privacy-first and key-free:
//! a user-configured **SearXNG** instance when set (`searxng_url`), otherwise a
//! no-key **DuckDuckGo** HTML fallback. Returns ranked {title, url, snippet}
//! that the model then opens with `fetch_web_page` — search finds pages, fetch
//! reads one.

use serde::Deserialize;
use std::net::{IpAddr, SocketAddr};

/// True for addresses that the web tools must never fetch: loopback, private,
/// link-local, CGNAT, unspecified, broadcast, multicast, and IPv4 0/8. Blocks
/// SSRF into the user's own machine or LAN (127.0.0.1, ::1, 10/8, 172.16/12,
/// 192.168/16, 169.254/16, 100.64/10, fc00::/7, fe80::/10, ...).
pub fn is_private_or_link_local(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let octets = v4.octets();
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_multicast()
                || (octets[0] == 100 && octets[1] >= 64 && octets[1] <= 127)
                || octets[0] == 0
        }
        IpAddr::V6(v6) => {
            v6.to_ipv4().is_some_and(|v4| is_private_or_link_local(IpAddr::V4(v4)))
                || v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (v6.segments()[0] & 0xfe00) == 0xfc00 // fc00::/7 unique local
                || (v6.segments()[0] & 0xffc0) == 0xfe80 // fe80::/10 link-local
        }
    }
}

fn unsafe_hostname(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    host == "localhost"
        || host.ends_with(".localhost")
        || host.ends_with(".local")
        || host.ends_with(".internal")
}

/// Synchronous SSRF surface check for a URL: rejects private/loopback IP
/// literals and loopback-style hostnames. Safe to run inside a reqwest redirect
/// policy (no DNS) — each redirect hop is re-checked before it is followed.
pub fn assert_url_surface(url: &reqwest::Url) -> Result<(), String> {
    let host = url.host_str().unwrap_or("");
    if host.is_empty() {
        return Err(format!("Invalid web URL: {url}"));
    }
    if unsafe_hostname(host) {
        return Err(format!(
            "Refused to fetch {url}: {host} is not a public web host."
        ));
    }
    // IPv6 hosts come back bracketed ("[::1]"); strip them for the literal check.
    let host = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(host);
    if let Ok(ip) = host.parse::<IpAddr>() {
        if is_private_or_link_local(ip) {
            return Err(format!(
                "Refused to fetch {url}: {host} is a private/local address."
            ));
        }
    }
    Ok(())
}

/// Resolve a public hostname once and return an address that the HTTP client
/// can pin for the request. Checking a hostname and then letting the client
/// resolve it again leaves a DNS-rebinding window between validation and
/// connection establishment.
pub async fn resolve_public_url(raw_url: &str) -> Result<Option<SocketAddr>, String> {
    let url = reqwest::Url::parse(raw_url).map_err(|_| format!("Invalid web URL: {raw_url}"))?;
    assert_url_surface(&url)?;
    let host = url.host_str().unwrap_or("").to_string();
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Ok(Some(SocketAddr::new(ip, url.port_or_known_default().unwrap_or(443))));
    }
    let port = url.port_or_known_default().unwrap_or(443);
    let mut addrs = tokio::net::lookup_host((host.as_str(), port))
        .await
        .map_err(|e| format!("DNS lookup failed for {host}: {e}"))?;
    addrs
        .find(|addr| !is_private_or_link_local(addr.ip()))
        .map(Some)
        .ok_or_else(|| format!("Refused to fetch {raw_url}: {host} has no public address"))
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

const UA: &str = "Mozilla/5.0 (compatible; Myelin/0.1; local notes web search)";

/// Shared HTTP client for outbound web calls (search + fetch). Two safeguards for
/// flaky/slow networks: bounded timeouts so a stalled connection can't hang a chat
/// turn, and binding the socket to an IPv4 local address so a broken IPv6 path
/// (common on mobile/hotspot links — exactly what made `fetch_web_page` fail with
/// "error sending request") can't sink a request. Falls back to a default client
/// if the builder ever fails.
pub fn web_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(6))
        .timeout(std::time::Duration::from_secs(20))
        .local_address(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

/// Run a web search. Uses SearXNG (`searxng_url`, JSON API) when provided,
/// else the no-key DuckDuckGo HTML endpoint. Caps to `count` results.
pub async fn web_search(
    query: &str,
    count: usize,
    searxng_url: Option<&str>,
) -> Result<Vec<SearchResult>, String> {
    let q = query.trim();
    if q.is_empty() {
        return Err("empty search query".into());
    }
    let results = match searxng_url.map(str::trim).filter(|s| !s.is_empty()) {
        Some(base) => searxng_search(base, q).await?,
        None => duckduckgo_search(q).await?,
    };
    Ok(results.into_iter().take(count.clamp(1, 10)).collect())
}

/// Format results as a compact numbered list for the model to read and then
/// pick a URL to `fetch_web_page`.
pub fn format_results(query: &str, results: &[SearchResult]) -> String {
    if results.is_empty() {
        return format!("No web results found for \"{query}\".");
    }
    let mut out = format!("Web results for \"{query}\":\n\n");
    for (i, r) in results.iter().enumerate() {
        out.push_str(&format!("{}. {}\n   {}\n", i + 1, r.title.trim(), r.url.trim()));
        let snip = r.snippet.trim();
        if !snip.is_empty() {
            out.push_str(&format!("   {snip}\n"));
        }
        out.push('\n');
    }
    out.push_str("To read a result in full, call fetch_web_page with its URL.");
    out
}

#[derive(Deserialize)]
struct SearxResponse {
    #[serde(default)]
    results: Vec<SearxResult>,
}
#[derive(Deserialize)]
struct SearxResult {
    #[serde(default)]
    title: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    content: String,
}

async fn searxng_search(base: &str, query: &str) -> Result<Vec<SearchResult>, String> {
    let url = format!("{}/search", base.trim_end_matches('/'));
    let resp = web_client()
        .get(&url)
        .query(&[("q", query), ("format", "json"), ("safesearch", "0")])
        .header(reqwest::header::USER_AGENT, UA)
        .send()
        .await
        .map_err(|e| format!("SearXNG request failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!(
            "SearXNG returned HTTP {} — make sure the instance has the JSON format enabled.",
            resp.status()
        ));
    }
    let parsed: SearxResponse = resp
        .json()
        .await
        .map_err(|e| format!("SearXNG JSON parse failed: {e}"))?;
    Ok(parsed
        .results
        .into_iter()
        .filter(|r| !r.url.trim().is_empty())
        .map(|r| SearchResult {
            title: r.title,
            url: r.url,
            snippet: r.content,
        })
        .collect())
}

async fn duckduckgo_search(query: &str) -> Result<Vec<SearchResult>, String> {
    let resp = web_client()
        .get("https://html.duckduckgo.com/html/")
        .query(&[("q", query)])
        .header(reqwest::header::USER_AGENT, UA)
        .send()
        .await
        .map_err(|e| format!("DuckDuckGo request failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("DuckDuckGo returned HTTP {}", resp.status()));
    }
    let html = resp
        .text()
        .await
        .map_err(|e| format!("DuckDuckGo read failed: {e}"))?;
    Ok(parse_ddg_html(&html))
}

/// Parse the DuckDuckGo HTML endpoint: each result has a `result__a` anchor
/// (title + a `/l/?uddg=<encoded-real-url>` redirect href) and a
/// `result__snippet` block. Zips them positionally. Fragile by nature (it's a
/// scrape) — that's the trade-off for a no-key fallback.
fn parse_ddg_html(html: &str) -> Vec<SearchResult> {
    let anchor_re =
        regex::Regex::new(r#"(?s)class="result__a"[^>]*href="([^"]*)"[^>]*>(.*?)</a>"#).ok();
    let snippet_re =
        regex::Regex::new(r#"(?s)class="result__snippet"[^>]*>(.*?)</a>"#).ok();

    let titles: Vec<(String, String)> = anchor_re
        .map(|re| {
            re.captures_iter(html)
                .map(|c| (decode_ddg_href(&c[1]), strip_html(&c[2])))
                .filter(|(url, _)| !url.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let snippets: Vec<String> = snippet_re
        .map(|re| re.captures_iter(html).map(|c| strip_html(&c[1])).collect())
        .unwrap_or_default();

    titles
        .into_iter()
        .enumerate()
        .map(|(i, (url, title))| SearchResult {
            title,
            url,
            snippet: snippets.get(i).cloned().unwrap_or_default(),
        })
        .collect()
}

/// A DDG result href is `//duckduckgo.com/l/?uddg=<percent-encoded real url>&...`.
/// Pull out and decode `uddg`; if it isn't a redirect, normalise a bare `//host`.
fn decode_ddg_href(href: &str) -> String {
    if let Some(idx) = href.find("uddg=") {
        let rest = &href[idx + 5..];
        let enc = rest.split('&').next().unwrap_or("");
        return percent_decode(enc);
    }
    if let Some(stripped) = href.strip_prefix("//") {
        return format!("https://{stripped}");
    }
    href.to_string()
}

/// Minimal percent-decoder (`%XX` and `+`), enough for a `uddg` URL param. Avoids
/// pulling in a urlencoding crate for one call site.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hi = (bytes[i + 1] as char).to_digit(16);
                let lo = (bytes[i + 2] as char).to_digit(16);
                match (hi, lo) {
                    (Some(h), Some(l)) => {
                        out.push((h * 16 + l) as u8);
                        i += 3;
                    }
                    _ => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Strip HTML tags and decode the few entities that show up in result text.
fn strip_html(s: &str) -> String {
    let no_tags = regex::Regex::new(r"(?s)<[^>]+>")
        .map(|re| re.replace_all(s, "").into_owned())
        .unwrap_or_else(|_| s.to_string());
    no_tags
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_uddg_redirect() {
        let href = "//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fpage%3Fa%3D1&rut=abc";
        assert_eq!(decode_ddg_href(href), "https://example.com/page?a=1");
    }

    #[test]
    fn strip_tags_and_entities() {
        assert_eq!(
            strip_html("<b>Rust</b> &amp; <i>llama.cpp</i>  guide"),
            "Rust & llama.cpp guide"
        );
    }

    #[test]
    fn parse_minimal_ddg_block() {
        let html = r#"
            <a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Frust-lang.org%2F">The <b>Rust</b> Language</a>
            <a class="result__snippet" href="x">A language empowering everyone.</a>
        "#;
        let r = parse_ddg_html(html);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].url, "https://rust-lang.org/");
        assert_eq!(r[0].title, "The Rust Language");
        assert_eq!(r[0].snippet, "A language empowering everyone.");
    }

    #[test]
    fn format_is_readable() {
        let r = vec![SearchResult {
            title: "T".into(),
            url: "https://x.com".into(),
            snippet: "S".into(),
        }];
        let out = format_results("q", &r);
        assert!(out.contains("1. T"));
        assert!(out.contains("https://x.com"));
        assert!(out.contains("fetch_web_page"));
    }

    #[test]
    fn private_and_link_local_ranges_are_rejected() {
        for ip in [
            "127.0.0.1", "127.8.8.8", "10.0.0.5", "172.16.0.1", "172.31.255.254",
            "192.168.1.1", "169.254.1.1", "100.64.0.1", "100.127.255.255", "0.0.0.0",
            "0.1.2.3", "255.255.255.255", "224.0.0.1", "::1", "::", "fc00::1", "fd12::7",
            "fe80::1",
        ] {
            assert!(is_private_or_link_local(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["8.8.8.8", "1.1.1.1", "93.184.216.34", "2606:2800:220:1::1"] {
            assert!(!is_private_or_link_local(ip.parse().unwrap()), "{ip}");
        }
    }

    #[test]
    fn url_surface_blocks_loopback_and_private_literals() {
        for bad in [
            "https://127.0.0.1/secret",
            "http://localhost/admin",
            "http://[::1]/x",
            "http://[::ffff:127.0.0.1]/x",
            "http://192.168.1.10:8080/",
            "https://10.0.0.2/",
            "http://mybox.local/",
        ] {
            assert!(assert_url_surface(&reqwest::Url::parse(bad).unwrap()).is_err(), "{bad}");
        }
        for good in ["https://example.com/page", "https://93.184.216.34/"] {
            assert!(assert_url_surface(&reqwest::Url::parse(good).unwrap()).is_ok(), "{good}");
        }
    }

    #[tokio::test]
    async fn resolved_public_url_rejects_private_literals() {
        for url in ["http://127.0.0.1/", "http://[::ffff:127.0.0.1]/"] {
            assert!(resolve_public_url(url).await.is_err(), "{url}");
        }
    }
}
