use super::*;
use tempfile::tempdir;

#[test]
fn default_is_all_unset() {
    let store = SettingsStore::in_memory();
    assert!(store.trace().dataset.is_none());
    assert!(store.trace().deployment.is_none());
}

#[test]
fn parse_full_document_round_trips() {
    let text = r#"
[trace]
dataset    = "axiom-traces-dev"
deployment = "staging"
"#;
    let parsed = SettingsStore::parse(text).unwrap();
    assert_eq!(parsed.trace.dataset.as_deref(), Some("axiom-traces-dev"));
    assert_eq!(parsed.trace.deployment.as_deref(), Some("staging"));
}

#[test]
fn parse_empty_string_yields_default() {
    // An empty (or freshly-touched) file must not error: the load
    // path swallows IO failure already, but a 0-byte read still
    // hits parse(). Default is the same shape as a missing file.
    let parsed = SettingsStore::parse("").unwrap();
    assert_eq!(parsed, Settings::default());
}

#[test]
fn parse_partial_document_fills_missing_with_default() {
    // Only `[trace] dataset = ...` — deployment must default to
    // None so adding new keys later doesn't break old documents.
    let parsed = SettingsStore::parse(
        r#"[trace]
dataset = "only-this"
"#,
    )
    .unwrap();
    assert_eq!(parsed.trace.dataset.as_deref(), Some("only-this"));
    assert!(parsed.trace.deployment.is_none());
}

#[test]
fn parse_malformed_returns_error_without_panicking() {
    // Truncated table header is plain invalid TOML.
    let err = SettingsStore::parse("[trace\ndataset = ").unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("parsing settings"),
        "unexpected error chain: {msg}"
    );
}

#[test]
fn save_and_load_round_trip_through_disk() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("settings.toml");
    {
        let mut store = SettingsStore::load_from(path.clone());
        store.set_trace_dataset(Some("ds-prod".into()));
        store.set_trace_deployment(Some("eu".into()));
        store.save().unwrap();
    }
    // Reload from the same path — values must survive.
    let store2 = SettingsStore::load_from(path.clone());
    assert_eq!(store2.trace().dataset.as_deref(), Some("ds-prod"));
    assert_eq!(store2.trace().deployment.as_deref(), Some("eu"));

    // And the serialised file must be valid TOML we can re-parse
    // directly (no proprietary framing).
    let text = std::fs::read_to_string(&path).unwrap();
    let reparsed = SettingsStore::parse(&text).unwrap();
    assert_eq!(reparsed.trace.dataset.as_deref(), Some("ds-prod"));
}

#[test]
fn load_from_missing_file_yields_default() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("does-not-exist.toml");
    let store = SettingsStore::load_from(path);
    assert_eq!(store.settings(), &Settings::default());
}

#[test]
fn load_from_malformed_file_yields_default_without_overwriting() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("settings.toml");
    std::fs::write(&path, "this is not toml = = =").unwrap();
    let store = SettingsStore::load_from(path.clone());
    // Defaulted in memory.
    assert_eq!(store.settings(), &Settings::default());
    // ...but the bad file is still on disk (we don't auto-clobber).
    let still = std::fs::read_to_string(&path).unwrap();
    assert!(still.starts_with("this is not toml"));
}

#[test]
fn save_is_noop_for_in_memory_store() {
    // No path -> no IO, no error.
    let store = SettingsStore::in_memory();
    store.save().unwrap();
}

#[test]
fn empty_value_normalises_to_unset() {
    // `:trace set dataset=` (or `   `) must collapse to "unset"
    // rather than persisting an empty string.
    let mut store = SettingsStore::in_memory();
    store.set_trace_dataset(Some("   ".into()));
    assert!(store.trace().dataset.is_none());
    store.set_trace_deployment(Some(String::new()));
    assert!(store.trace().deployment.is_none());
}

#[test]
fn unset_clears_existing_value() {
    let mut store = SettingsStore::in_memory();
    store.set_trace_dataset(Some("x".into()));
    store.set_trace_dataset(None);
    assert!(store.trace().dataset.is_none());
}

#[test]
fn serialised_form_omits_none_fields() {
    // Unset fields shouldn't appear in the TOML — that way a
    // fresh-install settings.toml is just `[trace]` with nothing
    // under it, rather than two confusing `null` lines.
    let store = SettingsStore::in_memory();
    let text = toml::to_string_pretty(store.settings()).unwrap();
    assert!(!text.contains("dataset"), "got:\n{text}");
    assert!(!text.contains("deployment"), "got:\n{text}");
}
