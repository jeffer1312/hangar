use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use hangar_server::{
    accounts::{catalog::resolved, environment::AccountEnvironment},
    costs::{collect::ScopeSource, scopes::NativeScopes},
    list::discover_other::Dirs,
};

struct Fixture {
    _temp: tempfile::TempDir,
    home: PathBuf,
    dirs: Dirs,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        // O alias usa a identidade canônica, inclusive quando TEMP vem no formato 8.3 do Windows.
        let home = resolved(temp.path()).join("home");
        fs::create_dir(&home).unwrap();
        let dirs = Dirs {
            home: home.clone(),
            claude: home.join("effective-claude"),
            codex_home: home.join("effective-codex"),
            pi_sessions: home.join("pi"),
            omp_config: home.join("omp"),
            omp_agent: home.join("omp/agent"),
            kimi_home: home.join("kimi"),
        };
        Self {
            _temp: temp,
            home,
            dirs,
        }
    }

    fn write(&self, path: impl AsRef<Path>, content: &str) {
        let target = self.home.join(path);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(target, content).unwrap();
    }

    fn source(&self, fixed: &str) -> NativeScopes {
        let env = AccountEnvironment::from_map(BTreeMap::from([
            ("HOME".into(), self.home.to_string_lossy().into_owned()),
            (
                "USERPROFILE".into(),
                self.home.to_string_lossy().into_owned(),
            ),
            (
                "CLAUDE_CONFIG_DIR".into(),
                self.home
                    .join("wrong-claude")
                    .to_string_lossy()
                    .into_owned(),
            ),
            (
                "CODEX_HOME".into(),
                self.home.join("wrong-codex").to_string_lossy().into_owned(),
            ),
            ("CP_CLAUDE_CONFIG_DIRS".into(), fixed.into()),
        ]));
        NativeScopes::new(env, self.dirs.clone(), self.home.join("repo"))
    }
}

#[test]
fn scopes_native_preserves_accounts_labels_roots_and_effective_overrides() {
    let f = Fixture::new();
    f.write(
        "café-a/.claude.json",
        r#"{"oauthAccount":{"accountUuid":"uuid-a","emailAddress":"café@example.test"}}"#,
    );
    f.write("c-b/.claude.json", "[1,2]");
    f.write(
        ".claude.json",
        r#"{"oauthAccount":{"accountUuid":"uuid-home","emailAddress":"home@example.test"}}"#,
    );
    f.write(
        ".codex-extra/.hangar-codex-conta",
        r#"{"version":1,"id":"extra"}"#,
    );
    for dir in [
        &f.dirs.pi_sessions,
        &f.dirs.omp_agent.join("sessions"),
        &f.dirs.kimi_home.join("sessions"),
    ] {
        fs::create_dir_all(dir).unwrap();
    }
    let fixed = format!(
        "Nome A:{},Nome B:{}",
        f.home.join("café-a").display(),
        f.home.join("c-b").display()
    );
    let result = f.source(&fixed).fetch();
    assert!(
        result.is_ok(),
        "as fontes locais ficam disponíveis sem Python: {result:?}"
    );
    let got = result.unwrap();
    let claude: Vec<_> = got
        .claude
        .iter()
        .map(|c| (&c.root, c.account.as_str(), c.label.as_str()))
        .collect();
    let root_a = f.home.join("café-a").join("projects");
    let root_b = f.home.join("c-b").join("projects");
    assert_eq!(
        claude,
        vec![
            (&root_a, "anthropic:uuid-a", "café@example.test"),
            (&root_b, "anthropic:uuid-home", "home@example.test")
        ]
    );
    let extra = f.home.join(".codex-extra");
    let codex: Vec<_> = got
        .codex
        .iter()
        .map(|c| (&c.home, c.account.clone(), c.label.as_str()))
        .collect();
    assert_eq!(
        codex,
        vec![
            (
                &f.dirs.codex_home,
                format!("codex:{}", f.dirs.codex_home.display()),
                "Codex · default"
            ),
            (
                &extra,
                format!("codex:{}", extra.display()),
                "Codex · extra"
            ),
        ]
    );
    let omp = f.dirs.omp_agent.join("sessions");
    let roots: Vec<_> = got
        .pi
        .iter()
        .map(|p| (&p.root, p.source.as_str()))
        .collect();
    assert_eq!(roots, vec![(&f.dirs.pi_sessions, "pi"), (&omp, "omp")]);
    assert!(got.kimi.is_some());
    let kimi = got.kimi.unwrap();
    assert_eq!(kimi.root, f.dirs.kimi_home.join("sessions"));
    assert_eq!(kimi.index, f.dirs.kimi_home.join("session_index.jsonl"));
    assert_eq!(got.repo, f.home.join("repo"));
}

#[test]
fn scopes_native_same_uuid_uses_the_last_catalog_label_for_both_roots() {
    let f = Fixture::new();
    for name in ["a", "b"] {
        f.write(
            format!("{name}/.claude.json"),
            r#"{"oauthAccount":{"accountUuid":"same-uuid"}}"#,
        );
    }
    let fixed = format!(
        "Primeiro:{},Segundo:{}",
        f.home.join("a").display(),
        f.home.join("b").display()
    );
    let result = f.source(&fixed).fetch();
    assert!(
        result.is_ok(),
        "o catálogo mantém as duas raízes da mesma conta: {result:?}"
    );
    let got = result.unwrap();
    let labels: Vec<_> = got
        .claude
        .iter()
        .map(|s| (s.account.as_str(), s.label.as_str()))
        .collect();
    assert_eq!(
        labels,
        vec![
            ("anthropic:same-uuid", "Segundo"),
            ("anthropic:same-uuid", "Segundo")
        ]
    );
}

#[test]
fn scopes_native_omits_absent_agent_directories_and_counts_shared_root_once() {
    let mut f = Fixture::new();
    let absent = f.source("").fetch();
    assert!(
        absent.is_ok(),
        "ausência das pastas dos agentes não impede o catálogo"
    );
    let absent = absent.unwrap();
    assert!(absent.pi.is_empty());
    assert!(absent.kimi.is_none());
    f.dirs.pi_sessions = f.dirs.omp_agent.join("sessions");
    fs::create_dir_all(&f.dirs.pi_sessions).unwrap();
    let shared = f.source("").fetch().unwrap();
    let roots: Vec<_> = shared
        .pi
        .iter()
        .map(|s| (&s.root, s.source.as_str()))
        .collect();
    assert_eq!(roots, vec![(&f.dirs.pi_sessions, "pi")]);
}

#[test]
fn scopes_native_preserves_alias_deduplication_and_fallback_labels() {
    let f = Fixture::new();
    let account = f.home.join("conta");
    fs::create_dir(&account).unwrap();
    f.write(
        ".claude/.hangar-apelidos.json",
        &serde_json::json!({format!("claude:{}", account.display()): "Apelido de conta"})
            .to_string(),
    );
    let fixed = format!(
        "Primeira:{},Duplicada:{}",
        account.display(),
        account.display()
    );
    let got = f.source(&fixed).fetch().unwrap();
    let rows: Vec<_> = got
        .claude
        .iter()
        .map(|s| (&s.root, s.account.as_str(), s.label.as_str()))
        .collect();
    let root = account.join("projects");
    assert_eq!(
        rows,
        vec![(&root, "anthropic:Apelido de conta", "Apelido de conta")]
    );
    let defaults = f.source("").fetch().unwrap();
    let effective = f.dirs.claude.join("projects");
    let roots: Vec<_> = defaults
        .claude
        .iter()
        .map(|s| (&s.root, s.account.as_str()))
        .collect();
    assert_eq!(roots, vec![(&effective, "anthropic:effective-claude")]);
}

#[test]
fn scopes_native_invalid_local_oauth_uses_home_then_catalog_label() {
    let f = Fixture::new();
    let account = f.home.join("conta");
    fs::create_dir(&account).unwrap();
    let fixed = format!("Rótulo:{}", account.display());
    for raw in [
        b"[1,2]".as_slice(),
        b"\xff".as_slice(),
        br#"{"oauthAccount":{"accountUuid":""}}"#.as_slice(),
    ] {
        fs::write(account.join(".claude.json"), raw).unwrap();
        f.write(
            ".claude.json",
            r#"{"oauthAccount":{"accountUuid":"uuid-home","emailAddress":"home@example.test"}}"#,
        );
        let from_home = f.source(&fixed).fetch().unwrap();
        let rows: Vec<_> = from_home
            .claude
            .iter()
            .map(|s| (s.account.as_str(), s.label.as_str()))
            .collect();
        assert_eq!(rows, vec![("anthropic:uuid-home", "home@example.test")]);
        f.write(".claude.json", "null");
        let fallback = f.source(&fixed).fetch().unwrap();
        let rows: Vec<_> = fallback
            .claude
            .iter()
            .map(|s| (s.account.as_str(), s.label.as_str()))
            .collect();
        assert_eq!(rows, vec![("anthropic:Rótulo", "Rótulo")]);
    }
    f.write(
        "conta/.claude.json",
        r#"{"oauthAccount":{"accountUuid":"uuid-no-email"}}"#,
    );
    let no_email = f.source(&fixed).fetch().unwrap();
    let rows: Vec<_> = no_email
        .claude
        .iter()
        .map(|s| (s.account.as_str(), s.label.as_str()))
        .collect();
    assert_eq!(rows, vec![("anthropic:uuid-no-email", "Rótulo")]);
    f.write("conta/.claude.json", r#"{"oauthAccount":[1]}"#);
    assert!(matches!(
        f.source(&fixed).fetch(),
        Err(hangar_server::costs::collect::CollectError::NoScopes)
    ));
}

#[test]
fn scopes_native_codex_unreadable_catalog_keeps_the_default_source() {
    let mut f = Fixture::new();
    fs::remove_dir(&f.home).unwrap();
    fs::write(&f.home, "não é uma pasta").unwrap();
    f.dirs.claude = f._temp.path().join("external-claude");
    f.dirs.codex_home = f._temp.path().join("external-codex");
    fs::create_dir(&f.dirs.claude).unwrap();
    let fixed = format!("Externa:{}", f.dirs.claude.display());
    let result = f.source(&fixed).fetch();
    assert!(
        result.is_ok(),
        "o erro de enumeração mantém a fonte padrão: {result:?}"
    );
    let got = result.unwrap();
    let rows: Vec<_> = got
        .codex
        .iter()
        .map(|s| (&s.home, s.label.as_str()))
        .collect();
    assert_eq!(rows, vec![(&f.dirs.codex_home, "Codex · default")]);
}

#[cfg(unix)]
#[test]
fn scopes_native_codex_identity_uses_the_resolved_home() {
    let mut f = Fixture::new();
    let real = f.home.join("real-codex");
    fs::create_dir(&real).unwrap();
    let link = f.home.join("linked-codex");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    f.dirs.codex_home = link;
    let got = f.source("").fetch().unwrap();
    let rows: Vec<_> = got
        .codex
        .iter()
        .map(|s| (&s.home, s.account.clone()))
        .collect();
    assert_eq!(rows, vec![(&real, format!("codex:{}", real.display()))]);
}

proptest::proptest! {
    #![proptest_config(proptest::test_runner::Config {
        cases: 32,
        rng_seed: proptest::test_runner::RngSeed::Fixed(0x5343_4f50_4553),
        failure_persistence: None,
        ..proptest::test_runner::Config::default()
    })]
    #[test]
    fn scopes_native_arbitrary_oauth_bytes_never_panic(raw in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..512)) {
        let f = Fixture::new();
        let account = f.home.join("conta");
        fs::create_dir(&account).unwrap();
        fs::write(account.join(".claude.json"), raw).unwrap();
        let fixed = format!("Fuzz:{}", account.display());
        match f.source(&fixed).fetch() {
            Ok(scopes) => {
                proptest::prop_assert!(scopes.claude.iter().all(|s| s.account.starts_with("anthropic:")));
                proptest::prop_assert_eq!(scopes.repo, f.home.join("repo"));
            }
            Err(error) => proptest::prop_assert!(matches!(error, hangar_server::costs::collect::CollectError::NoScopes)),
        }
    }
}
