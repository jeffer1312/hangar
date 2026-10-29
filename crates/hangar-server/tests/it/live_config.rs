use hangar_server::machine_peers::{dotenv::{SettingsEnvironment,SettingsSnapshot},live_config};
use serde_json::Value;
#[test]
fn live_configuration_uses_captured_settings_interpolation_and_precedence() {
    let fixture:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/update-git/dotenv-read.json")).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let home=tempfile::tempdir().unwrap();let backend=home.path().join("backend");std::fs::create_dir(&backend).unwrap();
        let raw=case["raw"].as_str().unwrap();std::fs::write(backend.join(".env"),raw).unwrap();
        let environment=SettingsEnvironment::from_pairs(case["environment"].as_object().unwrap().iter().map(|(key,value)|(key.clone(),value.as_str().unwrap().to_owned())));
        let snapshot=SettingsSnapshot::from_parts(environment,raw.into());
        assert_eq!(live_config::update_channel(home.path(),Some(&backend),snapshot.environment(),"CP_SERVER_ID").unwrap(),case["server_id"],"{}",case["name"]);
    }
}
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
