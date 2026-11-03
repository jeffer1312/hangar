//! Sidecar do Codex (`~/.hangar/codex-sessions/<nome>.json`), gravado como o `sessions.py` grava: a
//! mesma trava `<nome>.lock` (o `File::lock` é `flock` no Unix e `LockFileEx` do arquivo inteiro no
//! Windows, que cobre o byte 0 do `msvcrt.locking`) e tmp+replace. Lançador e Rust escrevem o mesmo
//! arquivo, por isso toda escrita confere a vida (endpoint e pid do app-server) sob a trava.
//! Bloqueante: chamar fora do runtime (`spawn_blocking`).
use super::protocol::RuntimeError;
use crate::list::discover_other::sanitize_session_name;
use serde_json::{Map,Value,json};
use std::io;
use std::path::{Path,PathBuf};
use std::sync::{Arc,Mutex};

pub type Meta = Map<String,Value>;

/// Campos que o `session.patch_meta` do Codex grava (o `_PATCH["codex"]` do Python).
const CATALOG:&[&str] = &["thread_id","rollout_path","cwd","model","effort","mode","service_tier","skipped_async_questions","problema","cano","permission_mode"];

/// Da conversa: trocar de thread os descarta (`_merge_conversation`).
const CONVERSATION_ONLY:&[&str] = &["transfer_id","tool_output_token_limit","service_tier"];

pub fn dir(home:&Path) -> PathBuf { home.join(".hangar").join("codex-sessions") }

pub fn path(dir:&Path,name:&str) -> PathBuf { dir.join(format!("{}.json",sanitize_session_name(name))) }

/// Roda `body` com a trava do sidecar `name`; fechar o arquivo solta a trava.
pub fn locked<T>(dir:&Path,name:&str,body:impl FnOnce(&Path) -> io::Result<T>) -> io::Result<T> {
    std::fs::create_dir_all(dir)?;
    let lock = std::fs::OpenOptions::new().read(true).append(true).create(true)
        .open(dir.join(format!("{}.lock",sanitize_session_name(name))))?;
    lock.lock()?;
    let result = body(&path(dir,name));
    drop(lock);
    result
}

/// Ausente, ilegível ou não-objeto: nenhum sidecar, como o `load` do Python.
pub fn load(path:&Path) -> Option<Meta> {
    match serde_json::from_slice::<Value>(&super::queue::read_replaced(path).ok()?).ok()? { Value::Object(meta)=>Some(meta), _=>None }
}

/// Como `load`, mas só a ausência é `Ok(None)`: arquivo que existe e não se lê é erro, para quem
/// decide pelo sidecar se a sessão é dele.
pub fn read(path:&Path) -> io::Result<Option<Meta>> {
    let raw = match super::queue::read_replaced(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    match serde_json::from_slice::<Value>(&raw).map_err(|error|io::Error::new(io::ErrorKind::InvalidData,error))? {
        Value::Object(meta) => Ok(Some(meta)),
        _ => Err(io::Error::new(io::ErrorKind::InvalidData,"sidecar não é um objeto")),
    }
}

pub(super) fn write(path:&Path,meta:&Meta) -> io::Result<()> { super::queue::atomic_write(path,&serde_json::to_vec(meta)?) }

fn text<'a>(meta:&'a Meta,field:&str) -> Option<&'a str> { meta.get(field).and_then(Value::as_str).filter(|s|!s.is_empty()) }

/// O mesmo app-server: o lançador ressubido ou a troca de modo reescrevem endpoint e pid.
fn same_process(meta:&Meta,endpoint:&str,app_pid:u32) -> bool {
    text(meta,"endpoint") == Some(endpoint) && meta.get("app_pid").and_then(Value::as_u64) == Some(u64::from(app_pid))
}

/// `_merge_conversation`: thread nova descarta o que era da anterior.
pub fn merge_conversation(mut meta:Meta,fields:&Meta) -> Meta {
    let switched = text(&meta,"thread_id").zip(fields.get("thread_id")).is_some_and(|(old,new)|new.as_str() != Some(old));
    if switched { meta.retain(|key,_|!CONVERSATION_ONLY.contains(&key.as_str())); }
    for (key,value) in fields { meta.insert(key.clone(),value.clone()); }
    meta
}

/// Grava `key` no sidecar desta vida se a chave dele ainda é `expected`; devolve a chave que ficou
/// (a nova ou a que outro escritor pôs antes). `None`: o arquivo sumiu ou é de outra vida.
pub fn swap_key(dir:&Path,name:&str,endpoint:&str,app_pid:u32,expected:Option<&str>,key:&str) -> io::Result<Option<String>> {
    locked(dir,name,|path| {
        let Some(mut meta) = load(path) else { return Ok(None) };
        if !same_process(&meta,endpoint,app_pid) { return Ok(None); }
        let current = text(&meta,"key").map(str::to_owned);
        if current.as_deref() != expected { return Ok(current); }
        meta.insert("key".into(),json!(key));
        write(path,&meta)?;
        Ok(Some(key.to_owned()))
    })
}

/// Apaga o sidecar só se ele ainda é desta vida: outra vida (o lançador de novo, a troca de modo) já o
/// reescreveu, a mesma regra do `finally` do `hangar-codex-tui`.
pub fn delete_if_life(dir:&Path,name:&str,endpoint:&str,app_pid:u32,key:Option<&str>) -> io::Result<bool> {
    locked(dir,name,|path| {
        match load(path) {
            Some(meta) if same_process(&meta,endpoint,app_pid) && text(&meta,"key") == key => {}
            _ => return Ok(false),
        }
        match std::fs::remove_file(path) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        }
    })
}

/// Funde `patch` no sidecar só se cada campo de `expect` ainda tem o valor esperado (ausente = nulo).
/// `None`: o arquivo sumiu ou outra vida o reescreveu, e nada foi gravado.
pub fn update_if(dir:&Path,name:&str,expect:&[(&str,Value)],patch:&Meta) -> io::Result<Option<Meta>> {
    locked(dir,name,|path| {
        let Some(mut meta) = load(path) else { return Ok(None) };
        if expect.iter().any(|(field,value)|meta.get(*field).unwrap_or(&Value::Null) != value) { return Ok(None); }
        for (field,value) in patch { meta.insert(field.clone(),value.clone()); }
        write(path,&meta)?;
        Ok(Some(meta))
    })
}

/// O modo que o seletor do terminal confirmou, no arquivo desta vida (chave e app-server): o rollout só
/// o grava na rodada seguinte, e o adormecer parte daqui. `false`: o arquivo já é de outra vida.
pub fn remember_permission(dir:&Path,name:&str,key:&str,app_pid:u32,mode:&str) -> io::Result<bool> {
    let patch:Meta = std::iter::once(("permission_mode".to_owned(),json!(mode))).collect();
    update_if(dir,name,&[("key",json!(key)),("app_pid",json!(app_pid))],&patch).map(|meta|meta.is_some())
}

/// Grava o sidecar inteiro sob a trava: a volta da troca de modo cobre o lançador que o apagou ao sair.
pub fn put(dir:&Path,name:&str,meta:&Meta) -> io::Result<()> { locked(dir,name,|path|write(path,meta)) }

/// O que o ator do Codex com terminal precisa para gravar o sidecar sem o Python.
#[derive(Clone,Debug)]
pub struct SidecarLife {
    pub dir:PathBuf,
    pub name:String,
    pub key:String,
    pub endpoint:String,
    pub app_pid:u32,
    /// Conversa que o sidecar tem nesta vida: a troca só vale a partir dela (a CAS do `switch_thread`).
    pub thread:Arc<Mutex<String>>,
}

fn io_error(error:io::Error) -> RuntimeError { RuntimeError::new("codex_sidecar_io",&format!("sidecar do Codex inacessível: {:?}",error.kind())) }

/// O `session.patch_meta` do Python para o Codex com terminal, com as mesmas recusas: campo fora do
/// catálogo e sidecar de outra vida são erro; valor diferente da vista salva (`view`) e Fast de outra
/// thread voltam `stale`. Troca de thread é a CAS do `switch_thread`: mesmo app-server e a thread anterior.
pub fn patch(life:&SidecarLife,payload:&Value,view:&Value) -> Result<Value,RuntimeError> {
    let stale = ||Ok(json!({"updated":false,"stale":true}));
    let fields = catalog_fields(payload)?;
    if differs_from_view(fields,view) { return stale(); }
    locked(&life.dir,&life.name,|path| {
        let Some(current) = load(path).filter(|meta|same_process(meta,&life.endpoint,life.app_pid) && text(meta,"key") == Some(life.key.as_str())) else {
            return Ok(Err(RuntimeError::new("session_patch_owner","sidecar de outra vida")));
        };
        let mut thread = life.thread.lock().unwrap_or_else(|e|e.into_inner());
        if let Some(new) = fields.get("thread_id") {
            let recorded = text(&current,"thread_id").unwrap_or("");
            // Já trocada por outro escritor (o lançador grava com a mesma CAS) ou ainda na anterior.
            if new.as_str() != Some(recorded) && recorded != thread.as_str() { return Ok(stale()); }
        } else if fast_of_other_thread(fields,view,&current) {
            return Ok(stale());
        }
        let merged = merge_conversation(current,fields);
        write(path,&merged)?;
        if let Some(new) = merged.get("thread_id").and_then(Value::as_str) { new.clone_into(&mut thread); }
        Ok(Ok(json!({"updated":true})))
    }).map_err(io_error)?
}

fn catalog_fields(payload:&Value) -> Result<&Meta,RuntimeError> { catalog_fields_in(payload,CATALOG) }

/// O patch como objeto só com campos de `catalog`; o resto é a recusa `session_patch_field`.
pub(super) fn catalog_fields_in<'a>(payload:&'a Value,catalog:&[&str]) -> Result<&'a Meta,RuntimeError> {
    let fields = payload.as_object().ok_or_else(||RuntimeError::new("session_patch_field","patch do sidecar não é objeto"))?;
    if fields.keys().any(|key|!catalog.contains(&key.as_str())) {
        return Err(RuntimeError::new("session_patch_field","campo fora do catálogo do sidecar"));
    }
    Ok(fields)
}

fn differs_from_view(fields:&Meta,view:&Value) -> bool { fields.iter().any(|(key,value)|view.get(key).is_some_and(|saved|saved != value)) }

/// Fast é da conversa: o sidecar já noutra thread não herda a escolha da vista.
fn fast_of_other_thread(fields:&Meta,view:&Value,current:&Meta) -> bool {
    !fields.contains_key("thread_id") && fields.contains_key("service_tier")
        && view["thread_id"].as_str().is_some_and(|saved|!saved.is_empty() && Some(saved) != text(current,"thread_id"))
}

/// O `session.patch_meta` do Python para o Codex sem terminal: a vida é a chave mais `headless` (não há
/// app-server fixo; o `cano` muda a cada subida e é gravado por aqui). Mesmas recusas: campo fora do
/// catálogo e sidecar ausente, de outra chave ou já de terminal são erro; valor diferente da vista salva e Fast de outra thread
/// voltam `stale`.
pub fn patch_headless(dir:&Path,name:&str,key:&str,payload:&Value,view:&Value) -> Result<Value,RuntimeError> {
    let stale = json!({"updated":false,"stale":true});
    let fields = catalog_fields(payload)?;
    if differs_from_view(fields,view) { return Ok(stale); }
    locked(dir,name,|path| {
        // A chave é a mesma nos dois modos: um patch atrasado depois da troca nunca entra no sidecar de terminal.
        let Some(current) = load(path).filter(|meta|text(meta,"key") == Some(key) && meta.get("headless") == Some(&Value::Bool(true))) else {
            return Ok(Err(RuntimeError::new("session_patch_owner","sidecar de outra vida")));
        };
        if fast_of_other_thread(fields,view,&current) { return Ok(Ok(stale)); }
        write(path,&merge_conversation(current,fields))?;
        Ok(Ok(json!({"updated":true})))
    }).map_err(io_error)?
}

/// O `session.clear_cano` do Python: tira o `cano` só se ele ainda é o processo `pid` desta chave;
/// arquivo apagado fica apagado.
pub fn clear_cano(dir:&Path,name:&str,key:&str,payload:&Value) -> Result<Value,RuntimeError> { clear_cano_with(dir,name,key,payload,io_error) }

/// O mesmo para o sidecar do Claude, que só muda o código da falha de disco.
pub(super) fn clear_cano_with(dir:&Path,name:&str,key:&str,payload:&Value,io_error:fn(io::Error) -> RuntimeError) -> Result<Value,RuntimeError> {
    let pid = payload.as_object().filter(|fields|fields.len() == 1).and_then(|fields|fields.get("pid")).and_then(Value::as_i64)
        .ok_or_else(||RuntimeError::new("session_clear_cano_pid","pid do cano inválido"))?;
    locked(dir,name,|path| {
        let Some(mut current) = load(path) else { return Ok(json!({"cleared":false})) };
        if text(&current,"key") != Some(key) || current.get("cano").and_then(|cano|cano.get("pid")).and_then(Value::as_i64) != Some(pid) {
            return Ok(json!({"cleared":false}));
        }
        current.insert("cano".into(),Value::Null);
        write(path,&current)?;
        Ok(json!({"cleared":true}))
    }).map_err(io_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sidecar(dir:&Path,fields:Value) {
        std::fs::create_dir_all(dir).unwrap();
        let mut meta = json!({"name":"cx","provider":"codex","thread_id":"t1","rollout_path":"/r1","cwd":"/w",
            "endpoint":"ws://127.0.0.1:4000","app_pid":40,"key":"k1","service_tier":"fast","transfer_id":"x","tool_output_token_limit":9});
        for (key,value) in fields.as_object().unwrap() { meta[key] = value.clone(); }
        std::fs::write(path(dir,"cx"),meta.to_string()).unwrap();
    }

    fn life(dir:&Path) -> SidecarLife {
        SidecarLife { dir:dir.into(),name:"cx".into(),key:"k1".into(),endpoint:"ws://127.0.0.1:4000".into(),app_pid:40,thread:Arc::new(Mutex::new("t1".into())) }
    }

    fn read(dir:&Path) -> Value { Value::Object(load(&path(dir,"cx")).unwrap()) }

    #[test]
    fn thread_switch_is_a_cas_and_drops_what_belonged_to_the_old_thread() {
        let dir = tempfile::tempdir().unwrap();
        sidecar(dir.path(),json!({}));
        let life = life(dir.path());
        let switched = patch(&life,&json!({"thread_id":"t2","rollout_path":"/r2"}),&json!({"thread_id":"t2"})).unwrap();
        assert_eq!(switched["updated"],true);
        let meta = read(dir.path());
        assert_eq!((meta["thread_id"].as_str(),meta["rollout_path"].as_str()),(Some("t2"),Some("/r2")));
        for gone in CONVERSATION_ONLY { assert!(meta.get(gone).is_none(),"{gone} era da thread anterior"); }
        assert_eq!(*life.thread.lock().unwrap(),"t2");
        // Já igual: confirma sem descartar nada.
        assert_eq!(patch(&life,&json!({"thread_id":"t2","rollout_path":"/r2"}),&json!({})).unwrap()["updated"],true);
        // O sidecar seguiu para outra thread que não é a anterior desta vida: recusa como velho.
        sidecar(dir.path(),json!({"thread_id":"t9"}));
        assert_eq!(patch(&life,&json!({"thread_id":"t3"}),&json!({})).unwrap()["stale"],true);
        assert_eq!(read(dir.path())["thread_id"],"t9");
    }

    #[test]
    fn patch_refuses_like_the_python_policy() {
        let dir = tempfile::tempdir().unwrap();
        sidecar(dir.path(),json!({}));
        let life = life(dir.path());
        assert_eq!(patch(&life,&json!({"tui_pid":1}),&json!({})).unwrap_err().code,"session_patch_field");
        assert_eq!(patch(&life,&json!({"model":"m"}),&json!({"model":"other"})).unwrap()["stale"],true,"difere da vista salva");
        // Fast de outra thread: a vista salva já está noutra conversa.
        assert_eq!(patch(&life,&json!({"service_tier":"flex"}),&json!({"thread_id":"t0"})).unwrap()["stale"],true);
        assert_eq!(patch(&life,&json!({"service_tier":"flex","model":"m"}),&json!({"thread_id":"t1"})).unwrap()["updated"],true);
        assert_eq!(read(dir.path())["service_tier"],"flex");
        // Outra vida: o lançador ressubiu o app-server com outro pid.
        sidecar(dir.path(),json!({"app_pid":41}));
        assert_eq!(patch(&life,&json!({"model":"m"}),&json!({})).unwrap_err().code,"session_patch_owner");
        std::fs::remove_file(path(dir.path(),"cx")).unwrap();
        assert_eq!(patch(&life,&json!({"model":"m"}),&json!({})).unwrap_err().code,"session_patch_owner");
    }

    /// As recusas do `run("session.patch_meta")` do `runtime_policy.py` para o sem terminal.
    #[test]
    fn headless_patch_refuses_like_the_python_policy() {
        let dir = tempfile::tempdir().unwrap();
        sidecar(dir.path(),json!({"headless":true,"endpoint":null,"app_pid":null,"cano":{"pid":7}}));
        let patch = |payload:Value,view:Value|patch_headless(dir.path(),"cx","k1",&payload,&view);
        assert_eq!(patch(json!({"tui_pid":1}),json!({})).unwrap_err().code,"session_patch_field");
        assert_eq!(patch(json!([1]),json!({})).unwrap_err().code,"session_patch_field");
        assert_eq!(patch(json!({"model":"m"}),json!({"model":"other"})).unwrap()["stale"],true,"difere da vista salva");
        assert_eq!(patch(json!({"service_tier":"flex"}),json!({"thread_id":"t0"})).unwrap()["stale"],true,"Fast de outra thread");
        // O cano novo de uma subida grava sem conferir o anterior (a vida é a chave).
        assert_eq!(patch(json!({"cano":{"pid":8,"escuta":"unix:/s"}}),json!({})).unwrap()["updated"],true);
        assert_eq!(read(dir.path())["cano"]["pid"],8);
        // Thread nova descarta o que era da anterior, como o `sessions.update`.
        assert_eq!(patch(json!({"thread_id":"t2","rollout_path":"/r2"}),json!({"thread_id":"t2"})).unwrap()["updated"],true);
        let meta = read(dir.path());
        assert_eq!(meta["thread_id"],"t2");
        for gone in CONVERSATION_ONLY { assert!(meta.get(gone).is_none(),"{gone} era da thread anterior"); }
        // Mesma chave já no modo terminal (troca de modo gravou antes deste patch atrasado): erro, nada gravado.
        for terminal in [json!({"headless":false,"thread_id":"t2"}),json!({"thread_id":"t2"})] {
            sidecar(dir.path(),terminal);
            assert_eq!(patch(json!({"cano":{"pid":9},"model":"m"}),json!({})).unwrap_err().code,"session_patch_owner");
            let meta = read(dir.path());
            assert!(meta["cano"].is_null() && meta["model"].is_null(),"o sidecar de terminal fica intacto");
        }
        // Outra chave ou arquivo apagado: erro, nada gravado.
        sidecar(dir.path(),json!({"headless":true,"key":"k2"}));
        assert_eq!(patch(json!({"model":"m"}),json!({})).unwrap_err().code,"session_patch_owner");
        assert_eq!(read(dir.path())["model"],Value::Null);
        std::fs::remove_file(path(dir.path(),"cx")).unwrap();
        assert_eq!(patch(json!({"model":"m"}),json!({})).unwrap_err().code,"session_patch_owner");
        assert!(!path(dir.path(),"cx").exists(),"apagado fica apagado");
    }

    /// `runtime_policy.clear_cano`: só o cano `pid` desta chave sai; o resto do arquivo fica.
    #[test]
    fn clear_cano_only_drops_the_same_process() {
        let dir = tempfile::tempdir().unwrap();
        sidecar(dir.path(),json!({"cano":{"pid":7,"escuta":"unix:/s"}}));
        let clear = |payload:Value|clear_cano(dir.path(),"cx","k1",&payload);
        for bad in [json!({}),json!({"pid":"7"}),json!({"pid":7,"x":1}),json!({"pid":true}),json!(7)] {
            assert_eq!(clear(bad.clone()).unwrap_err().code,"session_clear_cano_pid","{bad}");
        }
        assert_eq!(clear(json!({"pid":8})).unwrap()["cleared"],false,"outro processo");
        assert_eq!(clear_cano(dir.path(),"cx","k2",&json!({"pid":7})).unwrap()["cleared"],false,"outra chave");
        assert_eq!(read(dir.path())["cano"]["pid"],7);
        assert_eq!(clear(json!({"pid":7})).unwrap()["cleared"],true);
        let meta = read(dir.path());
        assert!(meta["cano"].is_null() && meta["thread_id"] == "t1","só o cano sai");
        assert_eq!(clear(json!({"pid":7})).unwrap()["cleared"],false,"já limpo");
        std::fs::remove_file(path(dir.path(),"cx")).unwrap();
        assert_eq!(clear(json!({"pid":7})).unwrap()["cleared"],false);
        assert!(!path(dir.path(),"cx").exists(),"apagado fica apagado");
    }

    #[test]
    fn key_and_delete_only_touch_the_same_life() {
        let dir = tempfile::tempdir().unwrap();
        sidecar(dir.path(),json!({"key":null}));
        let (endpoint,pid) = ("ws://127.0.0.1:4000",40);
        assert_eq!(swap_key(dir.path(),"cx",endpoint,pid,None,"k2").unwrap().as_deref(),Some("k2"));
        assert_eq!(swap_key(dir.path(),"cx",endpoint,pid,None,"k3").unwrap().as_deref(),Some("k2"),"outro escritor gravou antes");
        assert_eq!(swap_key(dir.path(),"cx",endpoint,41,Some("k2"),"k3").unwrap(),None,"outro app-server");
        assert!(!delete_if_life(dir.path(),"cx",endpoint,pid,Some("k1")).unwrap(),"outra chave");
        assert!(!delete_if_life(dir.path(),"cx",endpoint,pid,None).unwrap(),"sem chave não apaga o que tem chave");
        assert!(!delete_if_life(dir.path(),"cx",endpoint,41,Some("k2")).unwrap(),"outro pid");
        assert!(delete_if_life(dir.path(),"cx",endpoint,pid,Some("k2")).unwrap());
        assert!(!path(dir.path(),"cx").exists());
        assert!(!delete_if_life(dir.path(),"cx",endpoint,pid,Some("k2")).unwrap(),"apagar de novo é nada");
    }

    #[test]
    fn the_lock_excludes_another_holder_of_the_same_file() {
        let dir = tempfile::tempdir().unwrap();
        locked(dir.path(),"cx",|_| {
            let other = std::fs::OpenOptions::new().read(true).append(true).open(dir.path().join("cx.lock")).unwrap();
            assert!(other.try_lock().is_err(),"a trava do Python (flock/LockFileEx) fica de fora enquanto o Rust grava");
            Ok(())
        }).unwrap();
        let other = std::fs::OpenOptions::new().read(true).append(true).open(dir.path().join("cx.lock")).unwrap();
        assert!(other.try_lock().is_ok(),"fechar solta");
    }

    #[test]
    fn update_if_writes_only_over_the_expected_life() {
        let dir = tempfile::tempdir().unwrap();
        sidecar(dir.path(),json!({}));
        let park = json!({"app_pid":null}).as_object().unwrap().clone();
        let other = [("endpoint",json!("ws://127.0.0.1:4000")),("app_pid",json!(41)),("key",json!("k1"))];
        assert!(update_if(dir.path(),"cx",&other,&park).unwrap().is_none(),"outra vida: nada gravado");
        assert_eq!(read(dir.path())["app_pid"],40);
        let ours = [("endpoint",json!("ws://127.0.0.1:4000")),("app_pid",json!(40)),("key",json!("k1"))];
        let parked = update_if(dir.path(),"cx",&ours,&park).unwrap().unwrap();
        assert!(parked["app_pid"].is_null() && parked["thread_id"] == "t1","só o campo pedido muda");
        // Ausente conta como nulo: a volta do `app_pid` confere a vida estacionada.
        let back = json!({"app_pid":40}).as_object().unwrap().clone();
        assert!(update_if(dir.path(),"cx",&[("key",json!("k1")),("app_pid",Value::Null)],&back).unwrap().is_some());
        std::fs::remove_file(path(dir.path(),"cx")).unwrap();
        assert!(update_if(dir.path(),"cx",&[],&back).unwrap().is_none(),"arquivo apagado continua apagado");
        put(dir.path(),"cx",&back).unwrap();
        assert_eq!(read(dir.path()),json!({"app_pid":40}),"`put` regrava inteiro, mesmo sem arquivo");
    }
}
