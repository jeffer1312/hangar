//! Arquivos referidos pelos hooks importados mantêm a fonte correspondente no Claude.
use super::{phase::Error,plugins::resolved};
use crate::config_sync::runtime_values::ValueGuard;
use serde_json::Value;
use std::{fs,path::{Path,PathBuf},sync::OnceLock};
fn paths(document:&Value)->Vec<String>{
 static TOKEN:OnceLock<regex::Regex>=OnceLock::new();
 let regex=TOKEN.get_or_init(||regex::Regex::new(r#"'([^']+)'|"([^"]+)"|([^\s\x1c-\x1f]+)"#).expect("Expressão fixa"));
 let mut out=vec![];
 if let Some(events)=document.get("hooks").and_then(Value::as_object){for groups in events.values().filter_map(Value::as_array){for group in groups{if let Some(hooks)=group.get("hooks").and_then(Value::as_array){for hook in hooks{if let Some(command)=hook.get("command").and_then(Value::as_str){for matched in regex.captures_iter(command){for token in matched.iter().skip(1).flatten(){let token=token.as_str().to_owned();if !out.contains(&token){out.push(token);}}}}}}}}}
 out
}
fn resolve(path:&Path)->Result<PathBuf,Error>{resolved(path).map_err(|message|Error{kind:"OSError",message})}
pub fn materialize(account:&Path,home:&Path,document:Option<&Value>,windows:bool)->Result<(Vec<String>,Vec<String>),Error>{
 let owned;
 let document=match document{Some(document)=>document,None=>{
  let Ok(raw)=fs::read_to_string(account.join("hooks.json"))else{return Ok((vec![],vec![]))};
  if raw.starts_with('\u{feff}'){return Ok((vec![],vec![]))}
  let Ok(value)=crate::orchestration::body::parse(raw.as_bytes())else{return Ok((vec![],vec![]))};owned=ValueGuard(value);&owned
 }};
 if !document.is_object(){return Ok((vec![],vec![]))}
 let source_root=home.join(".claude/hooks");let root=resolve(&account.join("hooks"))?;let mut stale=vec![];let mut missing=vec![];
 for raw in paths(document){let path=Path::new(&raw);if !path.is_absolute(){continue}let Some(parent)=path.parent()else{continue};let Ok(parent)=resolve(parent)else{continue};let Some(name)=path.file_name()else{continue};let target=parent.join(name);let Ok(relative)=target.strip_prefix(&root)else{continue};if relative.as_os_str().is_empty(){continue}
  if !path.exists(){missing.push(target.clone());}
  if windows&&target.is_file(){let source=source_root.join(relative);if source.is_file(){if let (Ok(a),Ok(b))=(fs::read(source),fs::read(&target)){if a!=b{stale.push(target);}}}}
 }
 let mut created=vec![];let mut orphans=vec![];
 for target in stale.into_iter().chain(missing){let relative=target.strip_prefix(&root).expect("Caminho conferido");let name=relative.to_string_lossy().replace('\\',"/");let source=source_root.join(relative);if !source.exists(){orphans.push(name);continue}let real=resolve(&source)?;
  let result=(||->std::io::Result<()>{fs::create_dir_all(target.parent().expect("Caminho absoluto"))?;if !windows&&target.is_symlink(){fs::remove_file(&target)?;}if windows{refresh_copy(account,&target,&fs::read(&real)?).map_err(|e|std::io::Error::other(e.to_string()))?;}else{
   #[cfg(unix)]std::os::unix::fs::symlink(&real,&target)?;
   #[cfg(windows)]std::os::windows::fs::symlink_file(&real,&target)?;
  }Ok(())})();if result.is_ok(){created.push(name)}else{orphans.push(name)}
 }
 Ok((created,orphans))
}

#[cfg(test)]
#[test]
fn copied_hook_preserves_the_previous_bytes_before_refresh() {
 let dir=tempfile::tempdir().unwrap();let home=dir.path().join("home");let account=home.join(".codex");let target=account.join("hooks/tool.py");
 fs::create_dir_all(target.parent().unwrap()).unwrap();fs::create_dir_all(home.join(".claude/hooks")).unwrap();fs::write(&target,b"personal edit").unwrap();fs::write(home.join(".claude/hooks/tool.py"),b"updated").unwrap();
 let document=serde_json::json!({"hooks":{"Stop":[{"hooks":[{"command":format!("python '{}'",target.display())}]}]}});
 let (created,orphans)=materialize(&account,&home,Some(&document),true).unwrap();assert_eq!(created,vec!["tool.py"]);assert!(orphans.is_empty());assert_eq!(fs::read(&target).unwrap(),b"updated");
 let backup=account.join(".hangar-hook-backups").join(super::files::hash(b"personal edit"));let file=fs::read_dir(backup).unwrap().next().unwrap().unwrap().path();let saved:Value=serde_json::from_slice(&fs::read(file).unwrap()).unwrap();assert_eq!(saved["conteudo_hex"],"706572736f6e616c2065646974");
}

fn refresh_copy(account:&Path,target:&Path,data:&[u8])->Result<(),super::files::Error>{
 let broken=target.is_symlink()&&!target.exists();
 let previous=if broken{None}else{super::files::read(target)?};
 let backups=account.join(".hangar-hook-backups").join(super::files::hash(previous.as_deref().unwrap_or_default()));
 if broken{
  use std::io::Write;
  let link=fs::read_link(target)?;super::files::backup(target,b"",&backups)?;
  let mut temporary=tempfile::NamedTempFile::new_in(target.parent().unwrap())?;temporary.write_all(data)?;temporary.as_file().sync_all()?;
  let temporary=temporary.into_temp_path();super::files::replace_preserving(&temporary,target,None,Some(&link))?;
 }else{super::files::write(target,data,previous.as_deref(),Some(&backups))?;}
 Ok(())
}
#[cfg(test)]
#[test]
fn copied_hook_repairs_a_broken_link_and_preserves_its_destination(){
 let dir=tempfile::tempdir().unwrap();let account=dir.path().join("account");fs::create_dir(&account).unwrap();let target=account.join("tool.py");let missing=Path::new("missing.py");
 #[cfg(unix)] std::os::unix::fs::symlink(missing,&target).unwrap();
 #[cfg(windows)] std::os::windows::fs::symlink_file(missing,&target).unwrap();
 refresh_copy(&account,&target,b"updated").unwrap();assert!(!target.is_symlink());assert_eq!(fs::read(&target).unwrap(),b"updated");
 let backup=account.join(".hangar-hook-backups").join(super::files::hash(b""));let file=fs::read_dir(backup).unwrap().next().unwrap().unwrap().path();let saved:Value=serde_json::from_slice(&fs::read(file).unwrap()).unwrap();assert_eq!(saved["symlink"],"missing.py");
}
