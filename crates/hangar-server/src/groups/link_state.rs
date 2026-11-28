//! `~/.hangar/link-returns.json`: quando cada filha foi ligada ao criador e quando a resposta dela
//! voltou a ele (`{"<filha>": {"linked_at": f64, "returned_at": f64|null, "returned_key": str|null}}`), mais a última
//! pergunta da filha avisada ao pai (`asked_key`, `asked_queue_id`, `asked_call`) e quem a respondeu (`parent_answer`), só quando houve.
use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LinkReturn {
    pub linked_at: f64,
    #[serde(default)]
    pub returned_at: Option<f64>,
    /// Qual resposta voltou: `last_reply_at` muda com qualquer escrita no transcript, a resposta não.
    #[serde(default)]
    pub returned_key: Option<String>,
    /// O que voltou com `returned_key` foi só o aviso de que a resposta não voltou (limite, turno sem texto): a chave
    /// segura o aviso repetido, mas o texto não chegou ao pai.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub returned_notice: bool,
    /// Última interação avisada ao pai: a pergunta pelo `interaction_id`, menu e permissão pelo texto do aviso
    /// (`notice:<hash>`). O ID nasce outro depois de reiniciar: só a pergunta vai de novo, com o novo.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asked_key: Option<String>,
    /// Linha do aviso na fila do pai: some a pergunta, ela sai se ainda espera.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asked_queue_id: Option<String>,
    /// `tool_use_id` da pergunta avisada que chegou ao pai (na fila ou entregue): é por ele que o fechamento se confere
    /// no transcript da filha. Ausente em menu, permissão e aviso recusado.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asked_call: Option<String>,
    /// A resposta que saiu do pai (`child_answer`) ou do servidor (repasse): a pergunta fechada com ela não é do dono.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_answer: Option<ParentAnswer>,
    /// Criada por `ensure_linked`, sem a ligação gravada: `linked_at` é a hora da recuperação, e resposta de antes
    /// dela não volta.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub recovered: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ParentAnswer {
    /// `tool_use_id` da pergunta respondida (o `interaction_id` quando não se conhece a chamada).
    pub call: String,
    /// Quando a marca foi gravada, antes da rota.
    pub at: f64,
    /// A rota respondeu e a resposta saiu (ou pode ter saído); sem isto, a rota ainda não respondeu.
    #[serde(default)]
    pub confirmed: bool,
}

pub type LinkReturns = BTreeMap<String, LinkReturn>;

/// Ler e regravar sem perder a escrita de outra tarefa do processo (a ligação e o retorno).
static LOCK: Mutex<()> = Mutex::new(());

pub fn path(hangar_dir: &Path) -> PathBuf { hangar_dir.join("link-returns.json") }

/// Roda E/S de disco fora do executor; pânico da tarefa vira erro de E/S.
pub async fn blocking<T: Send + 'static>(f: impl FnOnce() -> io::Result<T> + Send + 'static) -> io::Result<T> {
    tokio::task::spawn_blocking(f).await.map_err(io::Error::other).and_then(|r| r)
}

/// Arquivo ausente é vazio; ilegível é erro: regravar por cima apagaria os `returned_at` dos outros.
pub fn read(hangar_dir: &Path) -> io::Result<LinkReturns> {
    match std::fs::read(path(hangar_dir)) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(io::Error::other),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(LinkReturns::new()),
        Err(error) => Err(error),
    }
}

/// Lê, muda e grava (tmp+rename) sob a trava do processo.
pub fn update(hangar_dir: &Path, change: impl FnOnce(&mut LinkReturns)) -> io::Result<()> {
    modify(hangar_dir, |all| { change(all); true })
}

/// Como `update`, mas só grava quando `change` diz que mudou.
fn modify(hangar_dir: &Path, change: impl FnOnce(&mut LinkReturns) -> bool) -> io::Result<()> {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut all = read(hangar_dir)?;
    if !change(&mut all) { return Ok(()); }
    std::fs::create_dir_all(hangar_dir)?;
    crate::runtime::queue::atomic_write(&path(hangar_dir), &serde_json::to_vec_pretty(&all)?)
}

/// Ligação nova: `linked_at` agora e nenhum retorno ainda (o de uma vida antiga com o mesmo nome não vale).
pub fn record_linked(hangar_dir: &Path, children: &[String], now: f64) -> io::Result<()> {
    update(hangar_dir, |all| {
        for child in children { all.insert(child.clone(), LinkReturn { linked_at: now, ..LinkReturn::default() }); }
    })
}

/// Filha ligada sem registro (a gravação da ligação falhou): `linked_at` agora, então só resposta de
/// agora em diante volta. Devolve as que foram criadas; as que já tinham ficam intactas.
pub fn ensure_linked(hangar_dir: &Path, children: &[String], now: f64) -> io::Result<Vec<String>> {
    let mut created = Vec::new();
    modify(hangar_dir, |all| {
        for child in children {
            if !all.contains_key(child) {
                all.insert(child.clone(), LinkReturn { linked_at: now, recovered: true, ..LinkReturn::default() });
                created.push(child.clone());
            }
        }
        !created.is_empty()
    })?;
    Ok(created)
}

/// Filha renomeada leva o registro junto: sem ele, nunca mais volta resposta dela.
pub fn rename(hangar_dir: &Path, old: &str, new: &str) -> io::Result<()> {
    modify(hangar_dir, |all| match all.remove(old) {
        Some(entry) => { all.insert(new.to_owned(), entry); true }
        None => false,
    })
}

/// Tira as filhas que `keep` não reconhece mais (vínculo desfeito, sessão fechada).
pub fn retain(hangar_dir: &Path, keep: impl Fn(&str) -> bool) -> io::Result<()> {
    modify(hangar_dir, |all| { let before = all.len(); all.retain(|name, _| keep(name)); all.len() != before })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linking_writes_linked_at_and_keeps_the_other_children() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read(dir.path()).unwrap().is_empty());
        update(dir.path(), |all| { all.insert("b".into(), LinkReturn { linked_at: 1.0, returned_at: Some(2.0), ..LinkReturn::default() }); }).unwrap();
        record_linked(dir.path(), &["a".to_owned()], 10.5).unwrap();
        let raw: serde_json::Value = serde_json::from_slice(&std::fs::read(path(dir.path())).unwrap()).unwrap();
        assert_eq!(raw, serde_json::json!({"a": {"linked_at": 10.5, "returned_at": null, "returned_key": null},
            "b": {"linked_at": 1.0, "returned_at": 2.0, "returned_key": null}}));
        // Religar a mesma filha zera o retorno da vida anterior.
        record_linked(dir.path(), &["b".to_owned()], 20.0).unwrap();
        assert_eq!(read(dir.path()).unwrap()["b"], LinkReturn { linked_at: 20.0, ..LinkReturn::default() });
    }

    #[test]
    fn an_unreadable_file_is_not_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(path(dir.path()), "{torto").unwrap();
        assert!(record_linked(dir.path(), &["a".to_owned()], 1.0).is_err());
        assert_eq!(std::fs::read_to_string(path(dir.path())).unwrap(), "{torto");
    }

    #[test]
    fn old_files_load_rename_moves_the_entry_and_retain_prunes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(path(dir.path()), r#"{"a": {"linked_at": 1.0, "returned_at": 2.0}, "b": {"linked_at": 3.0}}"#).unwrap();
        assert_eq!(read(dir.path()).unwrap()["a"], LinkReturn { linked_at: 1.0, returned_at: Some(2.0), ..LinkReturn::default() });
        rename(dir.path(), "a", "a2").unwrap();
        rename(dir.path(), "sumiu", "x").unwrap();
        let all = read(dir.path()).unwrap();
        assert_eq!(all.keys().collect::<Vec<_>>(), ["a2", "b"]);
        assert_eq!(all["a2"].returned_at, Some(2.0));
        retain(dir.path(), |name| name == "b").unwrap();
        assert_eq!(read(dir.path()).unwrap().keys().collect::<Vec<_>>(), ["b"]);
    }

    #[test]
    fn an_old_file_without_the_question_fields_reads_none_and_writes_them_only_when_set() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(path(dir.path()), r#"{"a": {"linked_at": 1.0, "returned_at": null, "returned_key": "k"}}"#).unwrap();
        let a = read(dir.path()).unwrap()["a"].clone();
        assert_eq!((a.asked_key, a.asked_queue_id), (None, None));
        update(dir.path(), |all| {
            let a = all.get_mut("a").unwrap();
            a.asked_key = Some("q1".into());
            a.asked_queue_id = Some("row1".into());
        }).unwrap();
        let raw: serde_json::Value = serde_json::from_slice(&std::fs::read(path(dir.path())).unwrap()).unwrap();
        assert_eq!(raw["a"]["asked_key"], "q1");
        assert_eq!(raw["a"]["asked_queue_id"], "row1");
        // Religar zera a pergunta da vida anterior.
        record_linked(dir.path(), &["a".to_owned()], 5.0).unwrap();
        assert_eq!(read(dir.path()).unwrap()["a"], LinkReturn { linked_at: 5.0, ..LinkReturn::default() });
    }

    #[test]
    fn a_linked_child_without_entry_gets_one_from_now_and_existing_ones_stay() {
        let dir = tempfile::tempdir().unwrap();
        let old = LinkReturn { linked_at: 1.0, returned_at: Some(2.0), returned_key: Some("k".into()), ..LinkReturn::default() };
        update(dir.path(), |all| { all.insert("a".into(), old.clone()); }).unwrap();
        let created = ensure_linked(dir.path(), &["a".to_owned(), "b".to_owned()], 50.0).unwrap();
        assert_eq!(created, ["b"]);
        let all = read(dir.path()).unwrap();
        assert_eq!(all["a"], old);
        assert_eq!(all["b"], LinkReturn { linked_at: 50.0, recovered: true, ..LinkReturn::default() });
        let raw: serde_json::Value = serde_json::from_slice(&std::fs::read(path(dir.path())).unwrap()).unwrap();
        assert_eq!((&raw["b"]["recovered"], raw["a"].get("recovered")), (&serde_json::json!(true), None), "a ligação real não grava o campo");
        assert!(ensure_linked(dir.path(), &["b".to_owned()], 60.0).unwrap().is_empty(), "a segunda vez não recria");
        assert_eq!(read(dir.path()).unwrap()["b"].linked_at, 50.0);
        std::fs::write(path(dir.path()), "{torto").unwrap();
        assert!(ensure_linked(dir.path(), &["c".to_owned()], 1.0).is_err());
    }
}
