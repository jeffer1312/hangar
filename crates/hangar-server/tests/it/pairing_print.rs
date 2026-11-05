use hangar_server::machine_peers::{dotenv::{SettingsEnvironment,SettingsSnapshot},pairing};
proptest::proptest!{
 #[test]
 fn pairing_configuration_never_panics(raw in ".{0,120}", public in ".{0,120}", token in ".{0,120}"){
  let environment=SettingsEnvironment::from_pairs([("CP_PORT".to_owned(),raw),("CP_PUBLIC_URL".to_owned(),public),("CP_AUTH_TOKEN".to_owned(),token)]);
  if let Ok(settings)=pairing::Settings::from_snapshot(&SettingsSnapshot::from_parts(environment,String::new())){
   let mut output=Vec::new();let _=pairing::print(&settings,"192.0.2.10",false,&mut output);
  }
 }
}
