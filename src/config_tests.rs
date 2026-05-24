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
    let (name, dep) = cfg.active().unwrap();
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
    let (name, dep) = cfg.active().unwrap();
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
    assert!(cfg.active().is_err());
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
    assert!(cfg.active().is_err());
}
