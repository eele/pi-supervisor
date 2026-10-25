//! Kit version resolution against the zot registry API:
//! GET /v2/blackbox/pi-kit/tags/list → newest `YYYYMMDD.HHMMSS.mmm` tag.
//! Fixed-width timestamp tags sort lexicographically == chronologically.
//!
//! Direct connection, no proxy: the corporate proxy would MITM/stall the
//! registry (NO_PROXY discipline from the pi-build README).

const ZOT_URL: &str = "http://zot.byte.art/v2/blackbox/pi-kit/tags/list";
const REPO_PREFIX: &str = "zot.byte.art/blackbox/pi-kit:";

/// Returns the newest kit reference, or falls back to `fallback_tag` when
/// zot is unreachable ("" fallback = hard error).
pub fn latest_kit(fallback_tag: &str) -> Result<String, String> {
    match fetch_newest_tag() {
        Ok(tag) => Ok(format!("{REPO_PREFIX}{tag}")),
        Err(e) if !fallback_tag.is_empty() => {
            eprintln!("[zot] tags/list failed ({e}); using fallback tag");
            Ok(format!("{REPO_PREFIX}{fallback_tag}"))
        }
        Err(e) => Err(e),
    }
}

fn fetch_newest_tag() -> Result<String, String> {
    let resp = ureq::get(ZOT_URL)
        .timeout(std::time::Duration::from_secs(10))
        .call()
        .map_err(|e| format!("zot unreachable: {e}"))?;
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
    fn lexicographic_max_is_chronological() {
        let mut tags = ["20260101.000000.001", "20251231.235959.999", "20261025.150332.857"];
        tags.sort();
        assert_eq!(tags.last().unwrap(), &"20261025.150332.857");
    }
}
