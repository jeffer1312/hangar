//! O toggle do painel conserva a ordem de limpeza, leitura e publicação do livro.
use super::*;
use std::io::Write;
pub(super) const UNICODE_ERROR:&str="livro de peers contém Unicode inválido";
pub(super) fn toggle(panel:&Panel,id:&str,on:bool)->Result<(),String>{
    let path=panel.path("peers.json")?;
    let _lock=store::lock(&path).map_err(|error|format!("não consegui abrir o lock do peers.json: {}",error.msg))?;
    store::clean_temporary(&path).map_err(|error|error.msg)?;
    let raw=std::fs::read(&path).map_err(|error|format!("peers.json ilegível: {}",spawn_error(&error,&path.to_string_lossy())))?;
    let text=std::str::from_utf8(&raw).map_err(|error|format!("peers.json ilegível: {error}"))?;
    let mut data=ValueGuard(codec::loads(text).ok_or_else(||format!("peers.json ilegível: {}",json_diagnostic(text)))?);
    let entry=data.0.as_object_mut().and_then(|book|book.get_mut(id)).and_then(Value::as_object_mut)
        .ok_or_else(||format!("servidor '{id}' não está no peers.json"))?;
    if let Some(previous)=entry.insert("enabled".into(),json!(on)){codec::discard(previous);}
    let raw=codec::dumps_indented_utf8(&data,2).map_err(|_|UNICODE_ERROR.to_owned())?+"\n";
    let publish=||->std::io::Result<()>{
        let mut temporary=tempfile::Builder::new().prefix("peers.json.").suffix(".tmp").tempfile_in(path.parent().unwrap())?;
        temporary.write_all(raw.as_bytes())?;temporary.flush()?;
        let mut permissions=std::fs::metadata(&path)?.permissions();
        #[cfg(unix)] {use std::os::unix::fs::PermissionsExt;permissions.set_mode(permissions.mode()&0o777);}
        temporary.as_file().set_permissions(permissions)?;
        temporary.persist(&path).map_err(|error|error.error)?;
        Ok(())
    };
    publish().map_err(|error|format!("falha ao gravar peers.json: {}",spawn_error(&error,&path.to_string_lossy())))
}
fn json_diagnostic(text:&str)->String{
    let Err((_,detail))=crate::orchestration::body::parse(text.as_bytes()) else{return "JSON inválido".into()};
    let detail=ValueGuard(detail);let error=&detail["detail"][0];
    let Some(position)=error["loc"][1].as_u64() else{return "JSON inválido".into()};
    let position=position as usize;let before=text.chars().take(position).collect::<String>();
    let line=before.chars().filter(|character|*character=='\n').count()+1;
    let column=before.rsplit('\n').next().unwrap_or("").chars().count()+1;
    format!("{}: line {line} column {column} (char {position})",error["ctx"]["error"].as_str().unwrap_or("JSON inválido"))
}
