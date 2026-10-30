use hangar_server::update::git_engine::periodic_tick::{self,Event,Source};
use serde_json::{Value,json};
use std::sync::Mutex;
struct Captured<'a>{name:&'a str,events:Mutex<Vec<Value>>,sleeps:Mutex<usize>}
impl Captured<'_>{fn event(&self,value:Value){self.events.lock().unwrap().push(value);}}
impl Source for Captured<'_>{
 fn package(&self)->bool{self.name=="package"}
 async fn sleep(&self)->Result<(),()>{self.event(json!(["sleep",3600]));let mut count=self.sleeps.lock().unwrap();*count+=1;if *count==2{Err(())}else{Ok(())}}
 fn enabled(&self)->bool{self.name!="disabled"}
 async fn fetch(&self)->Result<(i32,String),String>{self.event(json!(["git",["fetch","origin"],{"timeout":120}]));if self.name=="fetch-error"{Err("fixture".into())}else{Ok((i32::from(self.name=="fetch-failed"),"fixture-tail".into()))}}
 async fn reason(&self)->Result<Option<String>,String>{self.event(json!(["reason"]));if self.name=="reason-error"{Err("fixture".into())}else{Ok(match self.name{"up-to-date"=>Some("em dia".into()),"skipped"=>Some("arvore suja".into()),_=>None})}}
 async fn working(&self)->Result<bool,String>{self.event(json!(["list"]));if self.name=="list-error"{Err("fixture".into())}else{Ok(self.name=="working")}}
 async fn launch(&self)->Result<Value,String>{self.event(json!(["start",[8765],{"expected_branch":"main"}]));if self.name=="launch-error"{Err("fixture".into())}else{Ok(json!({"ok":self.name!="refused","erro":"fixture-refused"}))}}
 fn report(&self,event:Event){self.event(match event{
  Event::FetchFailed(code,tail)=>json!(["warning","auto-update: fetch falhou (rc=%s): %s",code,tail]),
  Event::Launch=>json!(["info","auto-update: disparando atualizacao"]),
  Event::Refused(error)=>json!(["warning","auto-update: iniciar recusou (%s)",error]),
  Event::Skipped(reason)=>json!(["info","auto-update: pulando (%s)",reason]),
  Event::Failed(_)=>json!(["exception","auto-update: tick falhou"]),
 });}
}
#[tokio::test]
async fn update_periodic_checkout_matches_python_effects_notices_and_schedule(){
 let fixture:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/update-git/periodic-tick.json")).unwrap();
 let mut failures=Vec::new();
 for case in fixture["cases"].as_array().unwrap(){
  let source=Captured{name:case["name"].as_str().unwrap(),events:Mutex::new(vec![]),sleeps:Mutex::new(0)};
  periodic_tick::run(&source).await;
  let actual=Value::Array(source.events.into_inner().unwrap());
  if actual!=case["events"]{failures.push(json!({"name":case["name"],"actual":actual,"expected":case["events"]}));}
 }
 assert!(failures.is_empty(),"{}",serde_json::to_string_pretty(&failures).unwrap());
}

#[test]
fn update_invalid_timestamp_notice_keeps_python_repr(){
 let fixture:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/update-git/auto-update.json")).unwrap();
 for case in fixture["cases"].as_array().unwrap(){
  for expected in case["rendered_warnings"].as_array().unwrap(){
   assert_eq!(hangar_server::update::git_engine::automatic::invalid_timestamp_notice(&case["input"]["state"]["ts"]),expected.as_str().unwrap());
  }
 }
}
