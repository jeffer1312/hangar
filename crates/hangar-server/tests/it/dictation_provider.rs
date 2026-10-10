use hangar_server::{
    accounts::{AccountService, environment::AccountEnvironment},
    auth::TrustedHosts,
    config::Config,
    dictation::{context, harness, model::DictationStyle},
    routes::AppState,
};

#[test]
fn dictation_codex_keeps_the_effective_provider_without_putting_credentials_in_arguments() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("account");
    std::fs::create_dir(&home).unwrap();
    std::fs::write(home.join("config.toml"),"model_provider = \"local\"\n[model_providers.local]\nname = \"Local\"\nbase_url = \"http://127.0.0.1:1234/v1\"\nwire_api = \"responses\"\nrequires_openai_auth = false\nexperimental_bearer_token = \"fixture-secret\"\n").unwrap();
    let binary = std::path::Path::new(env!("CARGO_BIN_EXE_hangar-dictation-fixture"));
    let bin = root.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    std::fs::copy(
        binary,
        bin.join(if cfg!(windows) { "codex.exe" } else { "codex" }),
    )
    .unwrap();
    let env = AccountEnvironment::from_map(
        [
            ("HOME".into(), root.path().to_string_lossy().into_owned()),
            ("PATH".into(), bin.to_string_lossy().into_owned()),
        ]
        .into(),
    );
    let mut state = AppState::new(Config {
        listen: "127.0.0.1:0".parse().unwrap(),
        upstream: "127.0.0.1:1".parse().unwrap(),
        internal_secret: "fixture".into(),
        auth_token: "fixture".into(),
        log_path: None,
        trusted: TrustedHosts::parse("127.0.0.1"),
    });
    state.accounts = AccountService::new(env);
    let context = context::account("codex", &home, &state.accounts.env).unwrap();
    let (command, _directory) =
        harness::build_command(&state, &context, "chosen", DictationStyle::Prose, None).unwrap();
    let args = command
        .as_std()
        .get_args()
        .map(|arg| arg.to_string_lossy())
        .collect::<Vec<_>>();
    assert!(
        args.iter()
            .any(|arg| arg.as_ref() == "model_provider=\"local\""),
        "provedor selecionado desapareceu: {args:?}"
    );
    assert!(
        !args.iter().any(|arg| arg.contains("fixture-secret")),
        "credencial no argv"
    );
    assert!(
        command
            .as_std()
            .get_envs()
            .any(|(key, value)| key == "HANGAR_DICTATION_PROVIDER_TOKEN"
                && value.is_some_and(|value| value == "fixture-secret"))
    );
}

#[test]
fn dictation_codex_default_account_preserves_its_keyring_authentication() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join(".codex");
    std::fs::create_dir(&home).unwrap();
    std::fs::write(
        home.join("config.toml"),
        "cli_auth_credentials_store = \"keyring\"\n",
    )
    .unwrap();
    let bin = root.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    std::fs::copy(
        env!("CARGO_BIN_EXE_hangar-dictation-fixture"),
        bin.join(if cfg!(windows) { "codex.exe" } else { "codex" }),
    )
    .unwrap();
    let env = AccountEnvironment::from_map(
        [
            ("HOME".into(), root.path().to_string_lossy().into_owned()),
            ("PATH".into(), bin.to_string_lossy().into_owned()),
        ]
        .into(),
    );
    let mut state = AppState::new(Config {
        listen: "127.0.0.1:0".parse().unwrap(),
        upstream: "127.0.0.1:1".parse().unwrap(),
        internal_secret: "fixture".into(),
        auth_token: "fixture".into(),
        log_path: None,
        trusted: TrustedHosts::parse("127.0.0.1"),
    });
    state.accounts = AccountService::new(env);
    let context = context::account("codex", &home, &state.accounts.env).unwrap();
    let (command, _) =
        harness::build_command(&state, &context, "chosen", DictationStyle::Prose, None).unwrap();
    let args = command
        .as_std()
        .get_args()
        .map(|arg| arg.to_string_lossy())
        .collect::<Vec<_>>();
    assert!(
        args.iter()
            .any(|arg| arg.as_ref() == "cli_auth_credentials_store=\"keyring\""),
        "autenticação da conta substituída: {args:?}"
    );
    assert!(
        !args
            .iter()
            .any(|arg| arg.as_ref() == "cli_auth_credentials_store=\"file\"")
    );
}
