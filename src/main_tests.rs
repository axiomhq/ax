use super::*;

fn parse(args: &[&str]) -> std::result::Result<CliArgs, String> {
    // Re-implement the body of `parse_cli_args` but read from a slice
    // instead of `std::env::args`. Keeps the test hermetic.
    let mut iter = args.iter().copied().peekable();
    let mut cli = CliArgs::default();
    while let Some(arg) = iter.next() {
        match arg {
            "-h" | "--help" => return Err("help".to_string()),
            "-p" | "--param" => {
                let pair = iter
                    .next()
                    .ok_or_else(|| format!("missing argument to {arg}"))?;
                insert_param(&mut cli.params, pair)?;
            }
            s if s.starts_with("-p=") => insert_param(&mut cli.params, &s[3..])?,
            s if s.starts_with("--param=") => insert_param(&mut cli.params, &s[8..])?,
            "-d" | "--dashboard" => {
                let uid = iter
                    .next()
                    .ok_or_else(|| format!("missing argument to {arg}"))?;
                set_dashboard(&mut cli.dashboard, uid.to_string())?;
            }
            s if s.starts_with("-d=") => {
                set_dashboard(&mut cli.dashboard, s[3..].to_string())?
            }
            s if s.starts_with("--dashboard=") => {
                set_dashboard(&mut cli.dashboard, s[12..].to_string())?
            }
            s if s.starts_with('-') => return Err(format!("unknown flag: {s}")),
            _ if cli.file.is_none() => {
                cli.file = Some(std::path::PathBuf::from(arg));
            }
            _ => return Err(format!("unexpected positional: {arg}")),
        }
    }
    Ok(cli)
}

#[test]
fn file_only() {
    let cli = parse(&["q.mpl"]).unwrap();
    assert_eq!(
        cli.file.as_deref().map(|p| p.to_str().unwrap()),
        Some("q.mpl")
    );
    assert!(cli.params.is_empty());
}

#[test]
fn one_param() {
    let cli = parse(&["-p", "host=db-01"]).unwrap();
    assert_eq!(cli.params.get("host").map(String::as_str), Some("db-01"));
}

#[test]
fn multiple_params_with_file_in_any_order() {
    let cli = parse(&[
        "-p",
        "host=db-01",
        "q.mpl",
        "--param=region=us-east",
        "-p=window=1h",
    ])
    .unwrap();
    assert_eq!(
        cli.file.as_deref().map(|p| p.to_str().unwrap()),
        Some("q.mpl")
    );
    assert_eq!(cli.params.get("host").map(String::as_str), Some("db-01"));
    assert_eq!(
        cli.params.get("region").map(String::as_str),
        Some("us-east")
    );
    assert_eq!(cli.params.get("window").map(String::as_str), Some("1h"));
}

#[test]
fn dollar_prefix_is_stripped() {
    let cli = parse(&["-p", "$host=db-01"]).unwrap();
    assert_eq!(cli.params.get("host").map(String::as_str), Some("db-01"));
}

#[test]
fn value_with_equals_kept_intact() {
    // Only split on the FIRST `=`; values may contain `=`.
    let cli = parse(&["-p", "q=a=b=c"]).unwrap();
    assert_eq!(cli.params.get("q").map(String::as_str), Some("a=b=c"));
}

#[test]
fn missing_equals_errors() {
    let err = parse(&["-p", "host"]).unwrap_err();
    assert!(err.contains("NAME=VALUE"), "got {err}");
}

#[test]
fn empty_name_errors() {
    let err = parse(&["-p", "=val"]).unwrap_err();
    assert!(err.contains("empty parameter name"), "got {err}");
}

#[test]
fn unknown_flag_errors() {
    let err = parse(&["--frobnicate"]).unwrap_err();
    assert!(err.contains("unknown flag"));
}

#[test]
fn second_positional_errors() {
    let err = parse(&["a.mpl", "b.mpl"]).unwrap_err();
    assert!(err.contains("unexpected positional"));
}

#[test]
fn dashboard_flag_short_and_long() {
    let cli = parse(&["-d", "abc123"]).unwrap();
    assert_eq!(cli.dashboard.as_deref(), Some("abc123"));
    let cli = parse(&["--dashboard=xyz"]).unwrap();
    assert_eq!(cli.dashboard.as_deref(), Some("xyz"));
}

#[test]
fn dashboard_flag_missing_value_errors() {
    let err = parse(&["-d"]).unwrap_err();
    assert!(err.contains("missing argument"), "got {err}");
}

#[test]
fn dashboard_flag_empty_value_errors() {
    let err = parse(&["-d", "   "]).unwrap_err();
    assert!(err.contains("empty dashboard uid"), "got {err}");
}

#[test]
fn dashboard_flag_duplicated_errors() {
    let err = parse(&["-d", "one", "--dashboard", "two"]).unwrap_err();
    assert!(err.contains("more than once"), "got {err}");
}
