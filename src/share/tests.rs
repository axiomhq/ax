use super::*;

#[test]
fn rewrites_api_host_to_app() {
    assert_eq!(app_host("https://api.axiom.co"), "https://app.axiom.co");
    assert_eq!(app_host("https://api.axiom.co/"), "https://app.axiom.co");
    assert_eq!(
        app_host("http://api.example.test"),
        "http://app.example.test"
    );
}

#[test]
fn leaves_non_api_host_unchanged() {
    assert_eq!(
        app_host("https://staging.example.com"),
        "https://staging.example.com"
    );
}

#[test]
fn json_escapes_quotes_and_newlines() {
    assert_eq!(json_escape(r#"he said "hi""#), r#"he said \"hi\""#);
    assert_eq!(json_escape("a\nb\tc"), "a\\nb\\tc");
    assert_eq!(json_escape("a\\b"), "a\\\\b");
}

#[test]
fn json_escapes_control_chars() {
    // 0x01 is below the printable range and not one of the named escapes.
    assert_eq!(json_escape("\x01"), "\\u0001");
}

#[test]
fn urlencodes_reserved_chars() {
    assert_eq!(urlencode("{\"apl\":\"x\"}"), "%7B%22apl%22%3A%22x%22%7D");
    assert_eq!(urlencode("a b c"), "a%20b%20c");
}

#[test]
fn builds_url_with_dataset() {
    let url = build_axiom_url(
        "https://api.axiom.co",
        "org-123",
        "`my-ds`:cpu_usage[1h..]",
        Some("my-ds"),
    );
    assert!(url.starts_with("https://app.axiom.co/org-123/query?initForm="));
    // Decoded payload should contain both fields.
    let payload = url.split("initForm=").nth(1).unwrap();
    let decoded = urldecode_for_test(payload);
    assert!(decoded.contains(r#""apl":""#));
    assert!(decoded.contains(r#""metricsDataset":"my-ds""#));
}

#[test]
fn builds_url_without_dataset() {
    let url = build_axiom_url("https://api.axiom.co", "org", "foo", None);
    let payload = url.split("initForm=").nth(1).unwrap();
    let decoded = urldecode_for_test(payload);
    assert_eq!(decoded, r#"{"apl":"foo"}"#);
}

#[test]
fn empty_dataset_is_treated_as_none() {
    let url = build_axiom_url("https://api.axiom.co", "org", "foo", Some(""));
    let payload = url.split("initForm=").nth(1).unwrap();
    let decoded = urldecode_for_test(payload);
    assert!(!decoded.contains("metricsDataset"));
}

fn urldecode_for_test(s: &str) -> String {
    let mut out = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16).unwrap() as u8;
            let lo = (bytes[i + 2] as char).to_digit(16).unwrap() as u8;
            out.push(hi * 16 + lo);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).unwrap()
}
