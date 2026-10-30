use hangar_server::machine_peers::{dotenv::{SettingsEnvironment,SettingsSnapshot},pairing};
use serde_json::Value;
#[test]
fn pairing_matches_original_text_and_decodable_destinations() {
    let fixture:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/peers/pairing-print.json")).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let environment=SettingsEnvironment::from_pairs(case["settings"].as_object().unwrap().iter().map(|(key,value)|(format!("CP_{}",key.to_uppercase()),value.as_str().map(str::to_owned).unwrap_or_else(||value.to_string()))));
        let snapshot=SettingsSnapshot::from_parts(environment,String::new());
        let settings=pairing::Settings::from_snapshot(&snapshot).unwrap();
        let urls=settings.urls("192.0.2.10");
        assert_eq!(urls,(case["pwa"].as_str().unwrap().into(),case["api"].as_str().unwrap().into()));
        let tty=case["tty"].as_bool().unwrap();
        let mut bytes=Vec::new();pairing::print(&settings,"192.0.2.10",tty,&mut bytes).unwrap();
        let text=String::from_utf8(bytes).unwrap();
        if !tty {assert_eq!(text,case["stdout"].as_str().unwrap());continue;}
        let blocks=text.split("\n\n").filter(|part|part.starts_with('█')).collect::<Vec<_>>();
        assert_eq!(blocks.len(),2);
        for (block,url) in blocks.iter().zip([urls.0,urls.1]) {
            let rows=block.lines().map(|line|line.chars().collect::<Vec<_>>()).collect::<Vec<_>>();
            let width=rows[0].len()-2;
            let grid=rqrr::Grid::new(rqrr::SimpleGrid::from_func(width,|x,y| {
                let c=rows[(y+1)/2][x+1];
                if (y+1)%2==0 {matches!(c,'▄'|'\u{a0}')} else {matches!(c,'▀'|'\u{a0}')}
            }));
            assert_eq!(grid.decode().unwrap().1,url);
        }
        let captions=|value:&str|value.lines().filter(|line|line.starts_with("  ")).collect::<Vec<_>>().join("\n");
        assert_eq!(captions(&text),captions(case["stdout"].as_str().unwrap()));
    }
    for case in fixture["settings_coercions"].as_array().unwrap() {
        let key=format!("CP_{}",case["key"].as_str().unwrap().to_uppercase());
        let environment=SettingsEnvironment::from_pairs([(key.clone(),case["raw"].as_str().unwrap().to_owned())]);
        let settings=pairing::Settings::from_snapshot(&SettingsSnapshot::from_parts(environment,String::new()));
        if case.get("error").is_some(){assert_eq!(settings.err().unwrap(),format!("pairing_config_invalid: {key}"));}
        else {let settings=settings.unwrap();assert_eq!(if key=="CP_PORT"{settings.port}else{settings.front_port},case["value"].to_string());}
    }
}

proptest::proptest!{
 #[test]
 fn pairing_configuration_never_panics(raw in ".{0,120}", public in ".{0,120}", token in ".{0,120}"){
  let environment=SettingsEnvironment::from_pairs([("CP_PORT".to_owned(),raw),("CP_PUBLIC_URL".to_owned(),public),("CP_AUTH_TOKEN".to_owned(),token)]);
  if let Ok(settings)=pairing::Settings::from_snapshot(&SettingsSnapshot::from_parts(environment,String::new())){
   let mut output=Vec::new();let _=pairing::print(&settings,"192.0.2.10",false,&mut output);
  }
 }
}
