//! Seleção e execução usam os mesmos Git congelados e limites de injeção do original.
#![cfg(unix)]
use hangar_server::update::git_engine::{Service,releases::{Source,DownloadError}};
use serde_json::{Value,json};
use std::{path::Path,sync::Mutex,time::Duration,os::unix::fs::PermissionsExt};
fn git(root:&Path,args:&[&str])->String{let out=std::process::Command::new("/usr/bin/git").arg("-C").arg(root).args(args).env("GIT_CONFIG_GLOBAL","/dev/null").env("GIT_CONFIG_NOSYSTEM","1").output().unwrap();assert!(out.status.success(),"{:?}: {}",args,String::from_utf8_lossy(&out.stderr));String::from_utf8(out.stdout).unwrap().trim().into()}
struct Captured{requests:Vec<Value>,seen:Mutex<Vec<String>>}
impl Source for Captured{
 fn get(&self,url:&str,_:Duration)->Result<Vec<u8>,DownloadError>{
  if let Some(name)=self.requests.iter().filter_map(|r|r.get("manifest")).filter_map(|m|m["files"].as_object()).flat_map(|m|m.iter()).find_map(|(key,v)|v["name"].as_str().filter(|name|url.ends_with(&format!("/{name}"))).map(|_|key.rsplit('/').next().unwrap().to_owned())) {return Ok(format!("fixture-{name}").into_bytes());}
  self.seen.lock().unwrap().push(url.into());let request=self.requests.iter().find(|r|format!("{}/server-latest.json",r["url"].as_str().unwrap())==url).unwrap_or_else(||panic!("consulta não capturada: {url}"));
  if let Some(manifest)=request.get("manifest"){return Ok(serde_json::to_vec(manifest).unwrap());}
  Err(DownloadError{kind:if request["error"]=="HTTPError"{"HTTPError"}else{"OSError"},status:request["status"].as_u64().map(|v|v as u16),message:request["message"].as_str().unwrap().into()})
 }
}
#[test]
fn update_selection_pipeline_matches_original_head_manifest_and_upstream(){
 use base64::Engine;
 let corpus:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/update-git/selection-pipeline.json")).unwrap();let mut differences=vec![];
 for row in corpus["cases"].as_array().unwrap(){
 for real in [false,true] {if real && row["name"]!="test_topo_sem_binario_para_no_commit_do_binario_publicado" {continue;}
  let name=format!("{} {} real={real}",row["name"],row["parameters"]);let temp=tempfile::tempdir().unwrap();let root=temp.path().join("repo");let origin=temp.path().join("origin");let home=temp.path().join("home");let bin=temp.path().join("bin");std::fs::create_dir(&bin).unwrap();
  for(key,path)in [("origin",&origin),("repo",&root)]{let bundle=temp.path().join(format!("{key}.bundle"));std::fs::write(&bundle,base64::engine::general_purpose::STANDARD.decode(row["bundles"][key].as_str().unwrap()).unwrap()).unwrap();let mut args=vec!["clone"];if key=="origin"{args.push("--bare");}args.extend([bundle.to_str().unwrap(),path.to_str().unwrap()]);git(temp.path(),&args);}
  git(&root,&["remote","set-url","origin",origin.to_str().unwrap()]);git(&root,&["checkout","-B",row["branch"].as_str().unwrap(),row["head_before"].as_str().unwrap()]);git(&root,&["config","user.name","Fixture"]);git(&root,&["config","user.email","fixture@example.invalid"]);
  for name in ["uv","node","npm","tmux","systemctl"]{let path=bin.join(name);std::fs::write(&path,if name=="systemctl"{"#!/bin/sh\nexit 1\n"}else if name=="npm"{"#!/bin/sh\n/bin/mkdir -p frontend/dist\nprintf screen > frontend/dist/index.html\n"}else{"#!/bin/sh\nexit 0\n"}).unwrap();std::fs::set_permissions(path,std::fs::Permissions::from_mode(0o755)).unwrap();}
  std::fs::create_dir_all(root.join("frontend")).unwrap();
  let service=Service::new(home.clone(),root.clone(),temp.path().join("claude"),[("HOME".into(),home.display().to_string()),("PATH".into(),format!("{}:/usr/bin:/bin",bin.display())),("CP_RUST_SERVER".into(),if row["enabled"]==true{"1"}else{"0"}.into()),("CP_UPDATE_BRANCH".into(),row["target"].as_str().unwrap_or("").into()),("GIT_CONFIG_GLOBAL".into(),"/dev/null".into()),("GIT_CONFIG_NOSYSTEM".into(),"1".into())].into());
  let source=Captured{requests:row["requests"].as_array().unwrap().clone(),seen:Mutex::new(vec![])};let prepared=Mutex::new(vec![]);
  let result=service.execute_with_preparer(1,&source,Some("linux-x86_64"),&|s,topology,dist,release,source,platform|{assert_eq!(s.environment.get("HANGAR_UPDATE_OWNS_RUST").map(String::as_str),Some("1"));prepared.lock().unwrap().push(json!({"topology":topology,"dist":dist,"release":release.map(|r|r.manifest.clone())}));if real{s.prepare(topology,dist,release,source,platform)}else{Ok(())}});
  for(key,value)in row["state"].as_object().unwrap(){if result.get(key).unwrap_or(&Value::Null)!=value{differences.push(format!("{name}: estado {key}: {:?} != {value}",result.get(key)));}}
  for(key,actual)in [("head_after",git(&root,&["rev-parse","HEAD"])),("branch_after",git(&root,&["rev-parse","--abbrev-ref","HEAD"]))]{if actual!=row[key].as_str().unwrap(){differences.push(format!("{name}: {key}: {actual} != {}",row[key]));}}
  let upstream=std::process::Command::new("/usr/bin/git").arg("-C").arg(&root).args(["rev-parse","--abbrev-ref","@{upstream}"]).output().unwrap();let upstream=String::from_utf8(upstream.stdout).unwrap().trim().to_owned();if upstream!=row["upstream"].as_str().unwrap(){differences.push(format!("{name}: upstream {upstream} != {}",row["upstream"]));}
  let target=if row["target"].as_str().unwrap_or("").is_empty(){"main"}else{row["target"].as_str().unwrap()};let behind=git(&root,&["rev-list","--count",&format!("HEAD..origin/{target}")]).parse::<u64>().unwrap()>0;if behind!=row["behind"].as_bool().unwrap(){differences.push(format!("{name}: behind {behind}"));}
  let requests=source.seen.into_inner().unwrap();let expected=row["requests"].as_array().unwrap().iter().map(|r|format!("{}/server-latest.json",r["url"].as_str().unwrap())).collect::<Vec<_>>();if requests!=expected{differences.push(format!("{name}: consultas {requests:?} != {expected:?}"));}
  if row["prepare_reused_manifest"]==true{assert_eq!(prepared.lock().unwrap().first().unwrap()["release"],row["requests"][0]["manifest"],"{name}: manifesto entregue à preparação");}
 }
 }
 assert!(differences.is_empty(),"{}",differences.join("\n"));
}
