//! Eventos locais da sincronização, sem credenciais ou conteúdo do cofre.
use super::store::Result;
use serde_json::{Value, json};
use std::{cell::RefCell, path::PathBuf, sync::Mutex, time::Instant};

thread_local! { static OPERATION: RefCell<Option<String>> = const { RefCell::new(None) }; }
struct Operation(Option<String>);
impl Drop for Operation {
    fn drop(&mut self) { OPERATION.with(|slot| *slot.borrow_mut() = self.0.take()); }
}

#[derive(Default)]
pub(super) struct Diary {
    pub directory: Option<PathBuf>,
    refusals: Mutex<indexmap::IndexMap<(String, String), (String, String)>>,
}
impl Diary {
    fn record(&self, mut event: Value) {
        let Some(dir) = self.directory.as_deref() else { return };
        OPERATION.with(|slot| {
            if let Some(id) = slot.borrow().as_ref() { event["operacao"] = json!(id); }
        });
        crate::diag::record_blocking(Some(dir), &[event], "sincronização");
    }
    pub fn event(&self, name: &'static str) { self.record(json!({"evento":name,"nivel":"ok"})); }
    pub fn refused(&self, name: &'static str, reason: &'static str, code: &'static str) {
        self.record(json!({"evento":name,"nivel":"aviso","detalhe":reason,"codigo":code}));
    }
    pub fn access(&self, domain: &'static str, ip: &str, mechanism: &'static str, reason: Option<&'static str>) {
        let key = (domain.to_owned(), ip.to_owned());
        let mut refusals = self.refusals.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(reason) = reason {
            let current = (mechanism.to_owned(), reason.to_owned());
            if refusals.get(&key) == Some(&current) { return; }
            if !refusals.contains_key(&key) && refusals.len() >= 512 { refusals.shift_remove_index(0); }
            refusals.insert(key, current);
        } else if refusals.shift_remove(&key).is_none() { return; }
        // A origem limita a repetição apenas em memória; nunca entra no registro exportado.
        self.record(json!({"evento":format!("{domain}.{}",if reason.is_some(){"recusado"}else{"recuperado"}),
            "nivel":if reason.is_some(){"aviso"}else{"ok"},"etapa":mechanism,
            "codigo":match reason {Some("bloqueio_temporario")=>"429",Some(_)=>"401",None=>"200"},
            "detalhe":reason.unwrap_or("acesso_restabelecido")}));
    }
    pub fn trace<T>(&self, name: &'static str, operation: impl FnOnce() -> Result<T>) -> Result<T> {
        let previous = OPERATION.with(|slot| slot.replace(Some(super::store::id())));
        let _operation = Operation(previous);
        let started = Instant::now();
        self.record(json!({"evento":format!("{name}.inicio"),"nivel":"ok"}));
        let result = operation();
        let mut end = json!({"evento":format!("{name}.fim"),"nivel":if result.is_ok(){"ok"}else{"erro"},
            "codigo":if result.is_ok(){"retornou"}else{"excecao"},"ms":started.elapsed().as_millis() as u64});
        if let Err(error) = &result {
            end["erro_tipo"] = json!(if error.status < 500 { "HTTPException" } else {
                error.body["detail"]["code"].as_str().filter(|code|code.starts_with("sync_")).unwrap_or("sync_operation_failed")
            });
        }
        self.record(end);
        result
    }
}
