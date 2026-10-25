//! Kit version resolution against the zot registry API:
//! GET /v2/<repo>/tags/list → newest `YYYYMMDD.HHMMSS.mmm` tag.
//! Fixed-width timestamp tags sort lexicographically == chronologically.
//!
//! The repo is configurable (config.json `zot_repo`, e.g.
//! "zot.byte.art/blackbox/pi-kit"); host+path derive from it.
//!
//! Direct connection, no proxy: the corporate proxy would MITM/stall the
//! registry (NO_PROXY discipline from the pi-build README).

/// Registry API URL for a configured repo ref (host + path, no tag).
/// `tls` picks the scheme: zot.byte.art only serves https; a plain-http
/// scheme stays reachable via the retry fallback (local registries).
fn tags_list_url(repo: &str, tls: bool) -> String {
    let host = repo.split('/').next().unwrap_or(repo);
    let path = repo.get(host.len() + 1..).unwrap_or("").trim_matches('/');
    let scheme = if tls { "https" } else { "http" };
    format!("{scheme}://{host}/v2/{path}/tags/list")
}

/// Returns the newest kit reference, or falls back to `fallback_tag` when
/// zot is unreachable ("" fallback = hard error).
pub fn latest_kit(repo: &str, fallback_tag: &str) -> Result<String, String> {
    match fetch_newest_tag(repo) {
        Ok(tag) => Ok(format!("{repo}:{tag}")),
        Err(e) if !fallback_tag.is_empty() => {
            eprintln!("[zot] tags/list failed ({e}); using fallback tag");
            Ok(format!("{repo}:{fallback_tag}"))
        }
        Err(e) => Err(e),
    }
}

fn fetch_newest_tag(repo: &str) -> Result<String, String> {
    // https first (the zot deployment refuses plain http), then one http
    // retry so http-only local registries still work.
    let mut errs = Vec::new();
    for tls in [true, false] {
        match fetch_tags(&tags_list_url(repo, tls)) {
            Ok(newest) => return Ok(newest),
            Err(e) => errs.push(e),
        }
    }
    Err(format!("zot unreachable: {}", errs.join(" | ")))
}

fn fetch_tags(url: &str) -> Result<String, String> {
    let resp = ureq::get(url)
        .timeout(std::time::Duration::from_secs(10))
        .call()
        .map_err(|e| format!("{url}: {e}"))?;
    let body: serde_json::Value = resp
        .into_json()
        .map_err(|e| format!("zot bad JSON: {e}"))?;
    let tags = body
        .get("tags")
        .and_then(|t| t.as_array())
        .ok_or_else(|| "zot response has no tags[]".to_string())?;
    let newest = tags
        .iter()
        .filter_map(|t| t.as_str())
        .filter(|t| is_stamp_tag(t))
        .max()
        .ok_or_else(|| "no timestamp tags on zot".to_string())?;
    Ok(newest.to_string())
}

/// `YYYYMMDD.HHMMSS.mmm` (all digits + fixed dots).
pub fn is_stamp_tag(t: &str) -> bool {
    let b = t.as_bytes();
    b.len() == 8 + 1 + 6 + 1 + 3
        && b[8] == b'.'
        && b[15] == b'.'
        && b.iter().enumerate().all(|(i, c)| match i {
            8 | 15 => true,
            _ => c.is_ascii_digit(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamp_tag_shape() {
        assert!(is_stamp_tag("20261025.150332.857"));
        assert!(!is_stamp_tag("latest"));
        assert!(!is_stamp_tag("2026-10-25.150332.857"));
        assert!(!is_stamp_tag("20261025.15033.857"));
        assert!(!is_stamp_tag("20261025.150332.8570"));
    }

    #[test]
    fn tags_list_url_derives_from_repo() {
        assert_eq!(
            tags_list_url("zot.byte.art/blackbox/pi-kit", true),
            "https://zot.byte.art/v2/blackbox/pi-kit/tags/list"
        );
        assert_eq!(
            tags_list_url("registry.local:5000/team/kit", false),
            "http://registry.local:5000/v2/team/kit/tags/list"
        );
    }

    #[test]
    fn lexicographic_max_is_chronological() {
        let mut tags = ["20260101.000000.001", "20251231.235959.999", "20261025.150332.857"];
        tags.sort();
        assert_eq!(tags.last().unwrap(), &"20261025.150332.857");
    }
}
