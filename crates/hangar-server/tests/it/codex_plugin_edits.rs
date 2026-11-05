use hangar_server::codex_setup::plugin_edits;
use serde_json::{Value,json};

#[cfg(unix)]
#[tokio::test]
async fn codex_service_plugin_selection_persists_through_the_real_writer() {
 use hangar_server::accounts::environment::AccountEnvironment;
 use std::os::unix::fs::PermissionsExt;
 let root=tempfile::tempdir().unwrap();let account=root.path().join("account");let bin=root.path().join("bin");
 std::fs::create_dir(&account).unwrap();std::fs::create_dir(&bin).unwrap();let script=bin.join("codex");
 std::fs::write(&script,r##"#!/bin/sh
while IFS= read -r message; do
 id=$(printf '%s' "$message" | /usr/bin/sed -n 's/.*"id":\([0-9]*\).*/\1/p')
 case "$message" in
 *'"method":"config/batchWrite"'*)
  printf '%s' "$message" > "$FIXTURE_REQUEST"
  printf 'model = "personal"\n[plugins."a@m"]\nenabled = true\n' > "$CODEX_HOME/config.toml"
  printf '{"id":%s,"result":{}}\n' "$id";;
 *'"method":"initialize"'*) printf '{"id":%s,"result":{}}\n' "$id";;
 esac
done
"##).unwrap();std::fs::set_permissions(&script,std::fs::Permissions::from_mode(0o700)).unwrap();
 let request=root.path().join("request.json");let env=AccountEnvironment::from_pairs([("HOME",root.path().to_string_lossy().into_owned()),("PATH",bin.to_string_lossy().into_owned()),("FIXTURE_REQUEST",request.to_string_lossy().into_owned())]);
 let original="model = \"personal\"\n[plugins.\"a@m\"]\nenabled = false\n";std::fs::write(account.join("config.toml"),original).unwrap();
 plugin_edits::apply(&env,&account,&root.path().join("backups"),&root.path().join("work"),&[("a@m".into(),true)]).await.unwrap();
 let sent:Value=serde_json::from_slice(&std::fs::read(request).unwrap()).unwrap();assert_eq!(sent["params"]["edits"],json!([{"keyPath":"\"plugins\".\"a@m\".\"enabled\"","value":true,"mergeStrategy":"replace"}]));
 let actual:Value=toml::from_str(&std::fs::read_to_string(account.join("config.toml")).unwrap()).unwrap();assert_eq!(actual["plugins"]["a@m"]["enabled"],true);assert_eq!(actual["model"],"personal");
 let backup=std::fs::read_dir(root.path().join("backups")).unwrap().next().unwrap().unwrap().path();let saved:Value=serde_json::from_slice(&std::fs::read(backup).unwrap()).unwrap();assert_eq!(saved["conteudo_hex"],original.as_bytes().iter().map(|b|format!("{b:02x}")).collect::<String>());
}
