//! Bounded, deterministic input checks. These functions never read a browser's
//! permission assertions as evidence of server authority.
use serde_json::Value;

pub(crate) fn bounded_text(value: &str, max: usize) -> bool {
    !value.is_empty() && value.len() <= max && !value.chars().any(char::is_control)
}

pub(crate) fn safe_key(value: &str) -> bool {
    bounded_text(value, 80)
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
}

pub(crate) fn prohibited_key(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    lower.split(['_', '.', '-']).any(|part| {
        matches!(
            part,
            "password"
                | "passwd"
                | "secret"
                | "token"
                | "authorization"
                | "cookie"
                | "email"
                | "phone"
                | "keystroke"
                | "fingerprint"
                | "html"
                | "body"
                | "field"
                | "text"
        )
    })
}

/// Sanitized URLs never carry fragments, credentials or arbitrary query fields.
/// Campaign data is represented separately, not smuggled through URL fields.
pub(crate) fn safe_url(value: &str) -> bool {
    if !bounded_text(value, 2048) {
        return false;
    }
    let Ok(url) = url::Url::parse(value) else {
        return false;
    };
    matches!(url.scheme(), "https" | "http")
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
}

/// JSON scalars only: request bodies, DOM snapshots and nested journals cannot
/// pass even if an operator mistakenly adds their field name to a schema.
pub(crate) fn scalar(value: &Value, kind: &str) -> bool {
    match kind {
        "string" => value.as_str().is_some_and(|s| bounded_text(s, 512)),
        "integer" => value.is_i64() || value.is_u64(),
        "number" => value.is_number(),
        "boolean" => value.is_boolean(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn unsafe_urls_and_payloads_fail_closed() {
        assert!(safe_url("https://site.example/play"));
        for value in [
            "https://site.example/?email=a",
            "https://site.example/#token",
            "https://a:b@site.example/",
            "javascript:alert(1)",
        ] {
            assert!(!safe_url(value));
        }
        assert!(prohibited_key("access_token"));
        assert!(prohibited_key("form-field"));
        assert!(!prohibited_key("duration_ms"));
        assert!(!scalar(&json!({"nested":true}), "string"));
        assert!(!scalar(&json!("true"), "boolean"));
        assert!(scalar(&json!(true), "boolean"));
        assert!(!bounded_text("bad\nvalue", 512));
    }
}
