//! Esperados tirados da captura do Python (`contract/boot/b01..b16`), com as diferenças aceitas
//! (decisão 10: sem `export` nem `${VAR}`) escritas no próprio caso.
use hangar_boot::settings::{bind_ip, parse_env_file, standalone_env, Places, Settings, GUEST_PORT};
use proptest::prelude::*;
use std::net::IpAddr;

fn resolve(file: &str, environ: &[(&str, &str)]) -> Result<Settings, String> {
    let environ: Vec<(String, String)> = environ.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    Settings::resolve(Some(file), &environ)
}

fn s(bind: &str, port: u16, token: &str) -> Settings {
    Settings { bind: bind.into(), port, token: token.into(), forwarded: "127.0.0.1".into() }
}

#[test]
fn b01_env_file_is_read() {
    assert_eq!(resolve("CP_PORT=9999\nCP_AUTH_TOKEN=abc\n", &[]).unwrap(), s("127.0.0.1", 9999, "abc"));
}

#[test]
fn b01_process_wins_over_the_env_file() {
    let got = resolve("CP_PORT=9999\nCP_AUTH_TOKEN=abc\n", &[("CP_PORT", "7777"), ("CP_AUTH_TOKEN", "proc")]).unwrap();
    assert_eq!(got, s("127.0.0.1", 7777, "proc"));
}

#[test]
fn b01_export_and_expansion_are_not_read() {
    // Diferença aceita (decisão 10): o Python lia `export X=` e expandia `${VAR}`.
    assert_eq!(resolve("export CP_PORT=9999\nexport CP_AUTH_TOKEN=abc\n", &[]).unwrap(), s("127.0.0.1", 8765, "change-me"));
    assert_eq!(resolve("BASE=xyz\nCP_AUTH_TOKEN=${BASE}-tok\n", &[]).unwrap().token, "${BASE}-tok");
}

#[test]
fn b01_quotes_and_comments() {
    let got = resolve("CP_AUTH_TOKEN=\"a b\" # comentario\nCP_LAN_BIND_IP='0.0.0.0'\nCP_PORT=9000 # x\n", &[]).unwrap();
    assert_eq!(got, s("0.0.0.0", 9000, "a b"));
    assert_eq!(resolve("CP_AUTH_TOKEN=a#b\n", &[]).unwrap().token, "a#b", "# sem espaço antes é valor");
}

#[test]
fn b01_escapes_inside_quotes_follow_python_dotenv() {
    assert_eq!(resolve(r#"CP_AUTH_TOKEN="a\"b\\c\nd\q""#, &[]).unwrap().token, "a\"b\\c\nd\\q");
    assert_eq!(resolve(r"CP_AUTH_TOKEN='a\'b\\c\nd'", &[]).unwrap().token, "a'b\\c\\nd");
    assert_eq!(resolve(r#"CP_AUTH_TOKEN="a" # fora"#, &[]).unwrap().token, "a");
}

#[test]
fn b01_bom_on_the_first_line_loses_that_key() {
    assert_eq!(resolve("\u{feff}CP_AUTH_TOKEN=abc\nCP_PORT=9001\n", &[]).unwrap(), s("127.0.0.1", 9001, "change-me"));
}

#[test]
fn b01_empty_value_is_kept() {
    assert_eq!(resolve("CP_AUTH_TOKEN=\nCP_FRONT_PORT=\n", &[]).unwrap(), s("127.0.0.1", 8765, ""));
}

#[test]
fn b01_keys_have_no_case() {
    assert_eq!(resolve("cp_port=9002\ncp_auth_token=low\n", &[]).unwrap(), s("127.0.0.1", 9002, "low"));
    assert_eq!(resolve("", &[("cp_port", "9004")]).unwrap().port, 9004);
}

#[test]
fn b01_bad_port_refuses() {
    assert!(resolve("CP_PORT=abc\n", &[]).unwrap_err().contains("CP_PORT"));
}

#[test]
fn b01_crlf_and_missing_file() {
    assert_eq!(resolve("CP_AUTH_TOKEN=abc\r\nCP_PORT=9003\r\n", &[]).unwrap(), s("127.0.0.1", 9003, "abc"));
    assert_eq!(Settings::resolve(None, &[]).unwrap(), s("127.0.0.1", 8765, "change-me"));
}

fn places(home: &std::path::Path) -> Places<'_> {
    Places { home, claude_config: None, log_base: None }
}

fn vars(settings: &Settings, home: &std::path::Path) -> std::collections::HashMap<&'static str, String> {
    standalone_env(settings, &places(home), |_| false).unwrap().vars.into_iter().collect()
}

#[test]
fn b04_change_me_refuses_off_loopback() {
    let home = tempfile::tempdir().unwrap();
    let error = standalone_env(&s("192.168.0.10", 8765, "change-me"), &places(home.path()), |_| false).unwrap_err();
    assert_eq!(error, "Refusing to start: CP_AUTH_TOKEN is still the default 'change-me' while binding 192.168.0.10. \
        Set CP_AUTH_TOKEN to a strong secret, or bind 127.0.0.1 for local dev.");
}

#[test]
fn d27_empty_token_refuses_on_any_bind() {
    let home = tempfile::tempdir().unwrap();
    for bind in ["127.0.0.1", "0.0.0.0"] {
        let error = standalone_env(&s(bind, 8765, ""), &places(home.path()), |_| false).unwrap_err();
        assert_eq!(error, "Refusing to start: CP_AUTH_TOKEN is empty. Set CP_AUTH_TOKEN to a strong secret.");
    }
}

#[test]
fn b04_change_me_on_loopback_only_warns() {
    let home = tempfile::tempdir().unwrap();
    for bind in ["127.0.0.1", "localhost", "::1"] {
        let boot = standalone_env(&s(bind, 8765, "change-me"), &places(home.path()), |_| false).unwrap();
        assert_eq!(boot.warning.as_deref(), Some("WARNING: using the default 'change-me' token on loopback. Set CP_AUTH_TOKEN before exposing this on your LAN."));
    }
    assert!(standalone_env(&s("0.0.0.0", 8765, "forte"), &places(home.path()), |_| false).unwrap().warning.is_none());
}

#[test]
fn b01_b05_b06_listen_token_and_forwarded() {
    let home = tempfile::tempdir().unwrap();
    let got = vars(&Settings { forwarded: "10.0.0.1,127.0.0.1".into(), ..s("0.0.0.0", 9000, "tok") }, home.path());
    assert_eq!(got["HANGAR_SERVER_LISTEN"], "0.0.0.0:9000");
    assert_eq!(got["CP_AUTH_TOKEN"], "tok");
    assert_eq!(got["CP_FORWARDED_ALLOW_IPS"], "10.0.0.1,127.0.0.1");
    assert!(!got.contains_key("HANGAR_SERVER_UPSTREAM"), "sem Python atrás");
    assert_eq!(vars(&s("::1", 9000, "tok"), home.path())["HANGAR_SERVER_LISTEN"], "[::1]:9000");
}

#[test]
fn b03_hostname_and_auto_become_ip_literals() {
    assert_eq!(bind_ip("localhost").unwrap(), IpAddr::from([127, 0, 0, 1]));
    assert!(bind_ip("auto").is_ok());
    assert!(bind_ip("nao-existe.invalid").is_err());
}

#[test]
fn b09_guest_port_follows_the_public_bind() {
    let home = tempfile::tempdir().unwrap();
    assert_eq!(vars(&s("0.0.0.0", 8765, "t"), home.path())["HANGAR_SERVER_GUEST_LISTEN"], "0.0.0.0:8766");
    assert_eq!(vars(&s("::", 8765, "t"), home.path())["HANGAR_SERVER_GUEST_LISTEN"], "0.0.0.0:8766");
    assert_eq!(vars(&s("127.0.0.1", 8765, "t"), home.path())["HANGAR_SERVER_GUEST_LISTEN"], "127.0.0.1:8766");
    assert!(!vars(&s("127.0.0.1", GUEST_PORT, "t"), home.path()).contains_key("HANGAR_SERVER_GUEST_LISTEN"));
}

#[test]
fn b10_shares_file_under_the_claude_config() {
    let home = tempfile::tempdir().unwrap();
    assert_eq!(vars(&s("127.0.0.1", 8765, "t"), home.path())["HANGAR_SHARES_FILE"],
        home.path().join(".claude").join("shares.json").to_string_lossy());
    let other = home.path().join("conta");
    let got: std::collections::HashMap<_, _> = standalone_env(&s("127.0.0.1", 8765, "t"),
        &Places { home: home.path(), claude_config: Some(&other), log_base: None }, |_| false).unwrap().vars.into_iter().collect();
    assert_eq!(got["HANGAR_SHARES_FILE"], other.join("shares.json").to_string_lossy());
}

#[test]
fn b12_server_log_in_the_private_folder() {
    let home = tempfile::tempdir().unwrap();
    let base = home.path().join("logs");
    let got: std::collections::HashMap<_, _> = standalone_env(&s("127.0.0.1", 8765, "t"),
        &Places { home: home.path(), claude_config: None, log_base: Some(&base) }, |_| false).unwrap().vars.into_iter().collect();
    assert_eq!(got["HANGAR_SERVER_LOG"], base.join("privado").join("hangar-server.log").to_string_lossy());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(base.join("privado")).unwrap().permissions().mode() & 0o777, 0o700);
    }
}

#[test]
fn b13_b14_instance_and_secret_are_new_each_boot() {
    let home = tempfile::tempdir().unwrap();
    let (a, b) = (vars(&s("127.0.0.1", 8765, "t"), home.path()), vars(&s("127.0.0.1", 8765, "t"), home.path()));
    for (key, len) in [("HANGAR_INTERNAL_SECRET", 64), ("HANGAR_RUNTIME_INSTANCE", 32)] {
        assert_eq!(a[key].len(), len);
        assert!(a[key].bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_ne!(a[key], b[key]);
    }
}

#[test]
fn process_env_keeps_what_it_already_has() {
    let home = tempfile::tempdir().unwrap();
    let boot = standalone_env(&s("127.0.0.1", 8765, "t"), &places(home.path()), |key| key == "HANGAR_SERVER_GUEST_LISTEN").unwrap();
    assert!(boot.vars.iter().all(|(key, _)| *key != "HANGAR_SERVER_GUEST_LISTEN"));
    assert!(boot.vars.iter().any(|(key, _)| *key == "CP_AUTH_TOKEN"), "o token resolvido sempre vai");
}

proptest! {
    #[test]
    fn any_env_file_parses_without_panic(text in "\\PC*") {
        let _ = parse_env_file(&text);
        let _ = Settings::resolve(Some(&text), &[]);
    }

    #[test]
    fn process_value_always_wins(file_port in 1u16.., process_port in 1u16..) {
        let environ = vec![("CP_PORT".to_string(), process_port.to_string())];
        let got = Settings::resolve(Some(&format!("CP_PORT={file_port}\n")), &environ).unwrap();
        prop_assert_eq!(got.port, process_port);
    }
}
