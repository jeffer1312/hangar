//! Liga os rascunhos do WhatsApp (`hangar_whatsapp::outbox`) ao servidor: o evento da lista do dono, o aviso
//! ao celular e o diário. A leitura e a recusa da subida moram no crate.
use std::sync::Arc;

use serde_json::Value;

use crate::routes::AppState;

struct Hooks {
    st: Arc<AppState>,
}

impl hangar_whatsapp::outbox::Hooks for Hooks {
    fn new_draft(&self, draft: &Value) {
        let session = origin_session(draft["origin"].as_str().unwrap_or_default());
        let id = draft["id"].as_str().unwrap_or_default().to_owned();
        match crate::push::send::Sender::from_state(self.st.clone()) {
            Ok(sender) => sender.whatsapp_draft(session, self.st.server_id.clone(), id),
            Err(code) => self.st.diag.report("rust.push_config_failed", session, code, "configuração inválida impediu o aviso do rascunho de WhatsApp"),
        }
    }

    fn failed(&self, code: &'static str, _detail: &str) {
        self.st.diag.report("rust.whatsapp_outbox", "", code, "rascunhos do WhatsApp sem leitura ou recusa; vale a última lista");
    }
}

/// A sessão de `servidor::sessao`; o que não tem cara de nome de sessão não vai ao aviso.
fn origin_session(origin: &str) -> &str {
    let name = origin.rsplit_once("::").map_or(origin, |(_, name)| name);
    if hangar_core::ids::valid_session_name(name) { name } else { "" }
}

/// A lista dos rascunhos pendentes, com ou sem lista aberta: o aviso ao celular não espera app aberto. Liga também o
/// evento da lista do dono, aqui e só aqui: o WhatsApp nasce depois do `ListHub`, pela pasta das contas que as provas
/// trocam depois de criar o estado.
pub fn spawn(st: Arc<AppState>) -> tokio::task::JoinHandle<()> {
    let whatsapp = st.whatsapp.get_or_init(|| hangar_whatsapp::WhatsApp::new(&st.accounts.env.home)).clone();
    st.hub.bind_whatsapp(whatsapp.outbox().feed());
    tokio::spawn(async move { whatsapp.watch_outbox(&Hooks { st }).await })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_push_names_only_a_plain_session() {
        assert_eq!(origin_session("casa::s0"), "s0");
        assert_eq!(origin_session("s0"), "s0");
        assert_eq!(origin_session("casa::a/b"), "");
        assert_eq!(origin_session("casa::"), "");
    }
}
