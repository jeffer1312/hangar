use super::*;
use crate::codex_setup::{files,instructions_toml};
use serde_json::json;
use std::{collections::BTreeMap,fs,path::PathBuf};

struct Fixture {
    name:String,home:PathBuf,inventory:Vec<Value>,changed:bool,
    calls:Vec<Value>,checkpoints:Vec<Value>,edits:Vec<Value>,
}
fn failure(message:&str)->Error{Error{kind:"RuntimeError",message:message.into()}}
impl Actions for Fixture {
    async fn installed(&mut self)->Result<Vec<Value>,Error>{self.calls.push(json!(["list"]));Ok(self.inventory.clone())}
    async fn detect(&mut self)->Result<Vec<Value>,Error>{
        self.calls.push(json!(["detect"]));
        Ok(vec![json!({"itemType":"CONFIG"}),json!({"itemType":"PLUGINS","description":"Plugins","details":{
            "plugins":[{"marketplaceName":"m","pluginNames":["a","unrelated"],"extra":"preserved"}]}})])
    }
    async fn import(&mut self,items:Vec<Value>)->Result<Value,Error>{
        self.calls.push(json!(["import",items]));self.inventory=vec![json!({"pluginId":"a@m","version":"1"})];
        Ok(json!({"itemTypeResults":[{"failures":if self.name=="import_partial"{vec![json!({"message":"Falha parcial"})]}else{vec![]}}]}))
    }
    async fn upgrade(&mut self,market:&str)->Result<Value,Error>{
        self.calls.push(json!(["upgrade",market]));if self.name=="upgrade_failure"{return Err(failure("falha de atualização"));}Ok(json!({"errors":[]}))
    }
    async fn add(&mut self,id:&str)->Result<Value,Error>{
        self.calls.push(json!(["add",id]));if self.name=="install_failure"{return Err(failure("falha de instalação"));}
        Ok(json!({"installedPath":self.home.join(".codex/plugin"),"version":"1"}))
    }
    async fn hooks(&mut self,path:&Path)->Result<(),Error>{
        self.calls.push(json!(["hooks",path]));
        if self.name=="hook_failure"{return Err(Error{kind:"OSError",message:"falha de hook".into()});}
        if self.name=="source_changes_forever" || self.name=="source_changed" && !self.changed {
            let path=self.home.join(".claude/settings.json");let mut bytes=fs::read(&path).unwrap();bytes.push(b' ');fs::write(path,bytes).unwrap();self.changed=true;
        }
        Ok(())
    }
    async fn edit(&mut self,choices:Vec<(String,bool)>)->Result<(),Error>{
        let current=instructions_toml::read(&self.home.join(".codex/config.toml"))?;
        let edits=choices.into_iter().filter(|(id,value)|!crate::config_sync::value_equality::equal(&current["plugins"][id]["enabled"],&json!(value)))
            .map(|(id,value)|{
                let key_path=["plugins",&id,"enabled"].map(|part|crate::transcript::pyjson::dumps_unicode(&json!(part),false)).join(".");
                json!({"keyPath":key_path,"value":value,"mergeStrategy":"replace"})
            }).collect::<Vec<_>>();
        self.edits.push(json!(edits));Ok(())
    }
    fn checkpoint(&mut self,registry:&mut Value,state:&State)->Result<(),Error>{
        registry["status"]=state.snapshot();
        let path=self.home.join(".codex/.hangar-integracao/estado.json");
        files::write(&path,&files::bytes(registry)?,files::read(&path)?.as_deref(),None)?;
        self.checkpoints.push(registry.clone());Ok(())
    }
    fn now(&self)->f64{100000.0}
}

fn decode(text:&str)->Vec<u8>{text.as_bytes().chunks_exact(2).map(|pair|u8::from_str_radix(std::str::from_utf8(pair).unwrap(),16).unwrap()).collect()}
fn normalize(value:&Value,home:&Path)->Value{
    serde_json::from_str(&serde_json::to_string(value).unwrap().replace(home.to_str().unwrap(),"$ROOT")).unwrap()
}
fn snapshot(home:&Path)->Value {
    fn visit(home:&Path,path:&Path,output:&mut BTreeMap<String,String>){
        let mut entries=fs::read_dir(path).unwrap().map(|entry|entry.unwrap().path()).collect::<Vec<_>>();entries.sort();
        for path in entries{
            if path.is_dir(){visit(home,&path,output);continue;}
            let raw=fs::read(&path).unwrap();let mut key=path.strip_prefix(home).unwrap().to_string_lossy().into_owned();
            if key.starts_with(".codex/backups/"){
                let data:Value=serde_json::from_slice(&raw).unwrap();let original=Path::new(data["path"].as_str().unwrap());
                assert_eq!(path.file_name().unwrap().to_str().unwrap(),format!("{}.json",files::hash(original.to_string_lossy().as_bytes())));
                key=format!(".codex/backups/$HASH/{}",original.strip_prefix(home).unwrap().display());
            }
            let data=String::from_utf8(raw).unwrap().replace(home.to_str().unwrap(),"$ROOT");
            assert!(output.insert(key,data.as_bytes().iter().map(|byte|format!("{byte:02x}")).collect()).is_none());
        }
    }
    let mut result=BTreeMap::new();visit(home,home,&mut result);json!(result)
}

#[tokio::test]
async fn plugin_reconciliation_preserves_original_retries_checkpoints_and_files(){
    let corpus:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/codex-integration/plugin-reconciliation.json")).unwrap();
    let mut differences=Vec::new();
    for case in corpus["cases"].as_array().unwrap(){
        let root=tempfile::tempdir().unwrap();let home=root.path();
        fs::create_dir_all(home.join(".codex/.hangar-integracao")).unwrap();
        for (name,data) in case["before"].as_object().unwrap(){
            let path=home.join(name);fs::create_dir_all(path.parent().unwrap()).unwrap();
            let data=String::from_utf8(decode(data.as_str().unwrap())).unwrap().replace("$ROOT",home.to_str().unwrap());fs::write(path,data).unwrap();
        }
        let mut registry:Value=serde_json::from_str(&case["registry_before"].to_string().replace("$ROOT",home.to_str().unwrap())).unwrap();
        let state=State::new(json!({"erros":[],"avisos":[],"progresso":{"passo":3,"total":9,"sub":null}}));
        let name=case["name"].as_str().unwrap();
        let mut actions=Fixture{name:name.into(),home:home.into(),inventory:if matches!(name,"missing"|"import_partial"){vec![]}else{vec![json!({"pluginId":"a@m","version":"1"})]},changed:false,calls:vec![],checkpoints:vec![],edits:vec![]};
        let paths=Paths{home,codex:&home.join(".codex"),backups:&home.join(".codex/backups")};
        let result=match run(&paths,&mut registry,&state,case["forced"].as_bool().unwrap(),&mut actions).await {
            Ok(())=>json!({"ok":true}),Err(error)=>json!({"error":error.message,"class":error.kind}),
        };
        let actual=normalize(&json!({"result":result,"registry":registry,"state":state.snapshot(),"confirmed":state.confirmed(),
            "calls":actions.calls,"checkpoints":actions.checkpoints,"config_edits":actions.edits,"after":snapshot(home)}),home);
        let expected=json!({"result":case["result"],"registry":case["registry"],"state":case["state"],"confirmed":case["confirmed"],
            "calls":case["calls"],"checkpoints":case["checkpoints"],"config_edits":case["config_edits"],"after":case["after"]});
        if actual!=expected{differences.push(json!({"name":name,"actual":actual,"expected":expected}));}
    }
    assert!(differences.is_empty(),"Reconciliação divergente: {differences:#?}");
}
