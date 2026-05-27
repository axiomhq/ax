use super::*;

#[test]
fn parses_single_deployment() {
    let text = r#"
        [deployments.prod]
        url = "https://api.axiom.co"
        token = "xaat-..."
        org_id = "heinz"
    "#;
    let cfg = Config::parse(text).unwrap();
    let (name, dep) = cfg.select(None).unwrap();
    assert_eq!(name, "prod");
    assert_eq!(dep.url, "https://api.axiom.co");
    assert_eq!(dep.org_id, "heinz");
}

#[test]
fn honors_active_deployments() {
    let text = r#"
        active_deployments = "staging"

        [deployments.prod]
        url = "https://api.axiom.co"
        token = "p"
        org_id = "o"

        [deployments.staging]
        url = "https://staging.example.com"
        token = "s"
        org_id = "o"
    "#;
    let cfg = Config::parse(text).unwrap();
    let (name, dep) = cfg.select(None).unwrap();
    assert_eq!(name, "staging");
    assert_eq!(dep.url, "https://staging.example.com");
}

#[test]
fn errors_when_multiple_deployments_and_no_active() {
    let text = r#"
        [deployments.a]
        url = "u"
        token = "t"
        org_id = "o"
        [deployments.b]
        url = "u"
        token = "t"
        org_id = "o"
    "#;
    let cfg = Config::parse(text).unwrap();
    assert!(cfg.select(None).is_err());
}

#[test]
fn errors_when_no_deployments() {
    assert!(Config::parse("").is_err());
}

#[test]
fn errors_when_active_deployment_missing() {
    let text = r#"
        active_deployments = "ghost"
        [deployments.prod]
        url = "u"
        token = "t"
        org_id = "o"
    "#;
    let cfg = Config::parse(text).unwrap();
    assert!(cfg.select(None).is_err());
}

#[test]
fn select_override_beats_active_deployments() {
    let text = r#"
        active_deployments = "prod"

        [deployments.prod]
        url = "https://prod.example.com"
        token = "p"
        org_id = "o"

        [deployments.staging]
        url = "https://staging.example.com"
        token = "s"
        org_id = "o"
    "#;
    let cfg = Config::parse(text).unwrap();
    let (name, dep) = cfg.select(Some("staging")).unwrap();
    assert_eq!(name, "staging");
    assert_eq!(dep.url, "https://staging.example.com");
}

#[test]
fn select_override_errors_on_unknown_name() {
    let text = r#"
        [deployments.prod]
        url = "u"
        token = "t"
        org_id = "o"
    "#;
    let cfg = Config::parse(text).unwrap();
    let err = cfg.select(Some("ghost")).unwrap_err().to_string();
    assert!(err.contains("ghost"), "got {err}");
    assert!(err.contains("not found"), "got {err}");
}

#[test]
fn select_empty_override_falls_back_to_active_logic() {
    // Empty/whitespace overrides shouldn't masquerade as an explicit pick;
    // they should defer to the persistent config field.
    let text = r#"
        active_deployments = "prod"
        [deployments.prod]
        url = "https://prod.example.com"
        token = "t"
        org_id = "o"
        [deployments.staging]
        url = "https://staging.example.com"
        token = "t"
        org_id = "o"
    "#;
    let cfg = Config::parse(text).unwrap();
    let (name, _) = cfg.select(Some("   ")).unwrap();
    assert_eq!(name, "prod");
}
