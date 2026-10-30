//! As leituras de saúde dos harnesses nunca reescrevem a configuração externa.
use hangar_server::{accounts::environment::AccountEnvironment, harness_setup::health};

proptest::proptest! {
    #![proptest_config(proptest::test_runner::Config::with_cases(48))]
    #[test]
    fn harness_health_never_rewrites_external_configuration(raw in proptest::collection::vec(proptest::prelude::any::<u8>(),0..4096)) {
        let home=tempfile::tempdir().unwrap();let root=home.path().to_string_lossy().into_owned();
        let env=AccountEnvironment::from_pairs([("HOME",root.as_str()),("USERPROFILE",root.as_str())]);
        let paths=[".claude.json",".claude/settings.json",".codex/config.toml",".codex/hooks.json",".pi/agent/fullscreen-tui.json",".bashrc"];
        for relative in paths {let path=home.path().join(relative);std::fs::create_dir_all(path.parent().unwrap()).unwrap();std::fs::write(path,&raw).unwrap();}
        for cli in ["claude","codex","pi","omp","kimi"] {
            let _=health::mcp(&env,cli);let _=health::default_model(&env,cli);
            health::fullscreen(&env,cli);health::wrapper(&env,cli,false);
        }
        health::codex_hooks(&env);health::claude_hooks(&home.path().join(".claude"));
        for relative in paths {proptest::prop_assert!(std::fs::read(home.path().join(relative)).unwrap()==raw);}
    }
}
