//! Sidecar do Antigravity sem terminal (`~/.hangar/antigravity-sessions/<nome>.json`), no formato e com a trava
//! dos outros dois (`codex_sidecar::locked`). Só o Rust grava. A vida é a chave mais `headless`: o `cano` muda a
//! cada subida e a conversa (`conversation_id`) é a que o `agy acp` devolveu ao abrir.
//! Bloqueante: chamar fora do runtime (`spawn_blocking`).
use super::codex_sidecar::{Meta,catalog_fields_in,load,locked,write};
use super::protocol::RuntimeError;
use serde_json::{Value,json};
use std::io;
use std::path::{Path,PathBuf};

pub fn dir(home:&Path) -> PathBuf { home.join(".hangar").join("antigravity-sessions") }

/// Campos que o ator grava pelo `session.patch_meta`.
const CATALOG:&[&str] = &["conversation_id","cwd","model","permission_mode","problema","cano"];

fn io_error(error:io::Error) -> RuntimeError { RuntimeError::new("antigravity_sidecar_io",&format!("sidecar do Antigravity inacessível: {:?}",error.kind())) }

fn text<'a>(meta:&'a Meta,field:&str) -> Option<&'a str> { meta.get(field).and_then(Value::as_str).filter(|s|!s.is_empty()) }

/// Campo fora do catálogo é erro; valor diferente da vista salva volta `stale` (a conversa da vista é
/// `conversation`); sidecar ausente, de outra chave ou já de terminal é erro.
pub fn patch(dir:&Path,name:&str,key:&str,payload:&Value,view:&Value) -> Result<Value,RuntimeError> {
    let fields = catalog_fields_in(payload,CATALOG)?;
    let stale = fields.iter().any(|(field,value)| {
        let source = if field == "conversation_id" { "conversation" } else { field.as_str() };
        view.get(source).is_some_and(|saved|!saved.is_null() && saved != value)
    });
    if stale { return Ok(json!({"updated":false,"stale":true})); }
    locked(dir,name,|path| {
        let Some(mut current) = load(path).filter(|meta|text(meta,"key") == Some(key) && meta.get("headless") == Some(&Value::Bool(true))) else {
            return Ok(Err(RuntimeError::new("session_patch_owner","sidecar de outra vida")));
        };
        for (field,value) in fields { current.insert(field.clone(),value.clone()); }
        write(path,&current)?;
        Ok(Ok(json!({"updated":true})))
    }).map_err(io_error)?
}

/// Tira o `cano` só se ele ainda é o processo `pid` desta chave; arquivo apagado fica apagado.
pub fn clear_cano(dir:&Path,name:&str,key:&str,payload:&Value) -> Result<Value,RuntimeError> {
    super::codex_sidecar::clear_cano_with(dir,name,key,payload,io_error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::codex_sidecar::path;

    fn sidecar(dir:&Path,fields:Value) {
        std::fs::create_dir_all(dir).unwrap();
        let mut meta = json!({"name":"ag","provider":"antigravity","headless":true,"key":"k1","cwd":"/w",
            "conversation_id":"904fd70e-847b-49ed-9999-a13d7a4d9a98","cano":{"pid":7,"escuta":"unix:/s"}});
        for (field,value) in fields.as_object().unwrap() { meta[field] = value.clone(); }
        std::fs::write(path(dir,"ag"),meta.to_string()).unwrap();
    }

    fn read(dir:&Path) -> Value { Value::Object(load(&path(dir,"ag")).unwrap()) }

    #[test]
    fn the_new_cano_and_the_opened_conversation_are_written_for_this_life_only() {
        let dir = tempfile::tempdir().unwrap();
        sidecar(dir.path(),json!({"conversation_id":null}));
        let patch = |payload:Value,view:Value|patch(dir.path(),"ag","k1",&payload,&view);
        assert_eq!(patch(json!({"cano":{"pid":8,"escuta":"unix:/t"}}),json!({})).unwrap()["updated"],true);
        assert_eq!(read(dir.path())["cano"]["pid"],8);
        let opened = "9abf1fce-b58c-43c2-98be-8071a4843f8d";
        assert_eq!(patch(json!({"conversation_id":opened}),json!({"conversation":opened})).unwrap()["updated"],true);
        assert_eq!(read(dir.path())["conversation_id"],opened);
        assert_eq!(patch(json!({"conversation_id":"x"}),json!({"conversation":opened})).unwrap()["stale"],true,"difere da vista salva");
        assert_eq!(patch(json!({"tui_pid":1}),json!({})).unwrap_err().code,"session_patch_field");
    }

    #[test]
    fn a_sidecar_of_another_life_or_of_the_terminal_is_never_written() {
        let dir = tempfile::tempdir().unwrap();
        sidecar(dir.path(),json!({"key":"k2"}));
        assert_eq!(patch(dir.path(),"ag","k1",&json!({"model":"m"}),&json!({})).unwrap_err().code,"session_patch_owner");
        sidecar(dir.path(),json!({"headless":false}));
        assert_eq!(patch(dir.path(),"ag","k1",&json!({"model":"m"}),&json!({})).unwrap_err().code,"session_patch_owner");
        std::fs::remove_file(path(dir.path(),"ag")).unwrap();
        assert_eq!(patch(dir.path(),"ag","k1",&json!({"model":"m"}),&json!({})).unwrap_err().code,"session_patch_owner");
    }

    #[test]
    fn the_cano_leaves_only_when_it_is_still_the_same_process() {
        let dir = tempfile::tempdir().unwrap();
        sidecar(dir.path(),json!({}));
        assert_eq!(clear_cano(dir.path(),"ag","k1",&json!({"pid":6})).unwrap()["cleared"],false);
        assert_eq!(clear_cano(dir.path(),"ag","k1",&json!({"pid":7})).unwrap()["cleared"],true);
        assert!(read(dir.path())["cano"].is_null());
    }
}
