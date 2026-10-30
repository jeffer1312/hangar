use hangar_server::machine_peers::{dotenv::SettingsEnvironment,live_config};
#[test]
fn live_configuration_requires_matching_nonce_disk_and_inherited_value() {
    use serde_json::json;
    let home=tempfile::tempdir().unwrap();let backend=home.path().join("backend");std::fs::create_dir(&backend).unwrap();let path=backend.join(".env");
    let environment=SettingsEnvironment::from_pairs([("cp_update_branch","old")]);
    let get=|env:&SettingsEnvironment|live_config::update_channel(home.path(),Some(&backend),env,"CP_UPDATE_BRANCH").unwrap();
    std::fs::write(&path,"CP_UPDATE_BRANCH=old\n").unwrap();
    std::fs::write(path.with_extension("env.update-channel.json"),json!({"nonce":"fixture-nonce","inherited":{"CP_UPDATE_BRANCH":"old"},"published":{"CP_UPDATE_BRANCH":"new"}}).to_string()).unwrap();
    assert_eq!(get(&environment),"old","marcador sozinho não publica");
    std::fs::write(&path,"CP_UPDATE_BRANCH=new\n# hangar-update-channel-publication=fixture-nonce\n").unwrap();
    assert_eq!(get(&environment),"new");
    assert_eq!(get(&SettingsEnvironment::from_pairs([("CP_UPDATE_BRANCH","manual")])),"manual");
    std::fs::write(&path,"CP_UPDATE_BRANCH=manual-file\n# hangar-update-channel-publication=fixture-nonce\n").unwrap();
    assert_eq!(get(&environment),"old","edição manual do arquivo respeita precedência normal do ambiente");
    assert_eq!(get(&SettingsEnvironment::default()),"manual-file");
}
