//! Tabela de despacho das escritas: quem atende cada rota para cada tipo de sessão. Só a tabela;
//! o corpo de cada rota mora nos módulos irmãos.
use axum::body::Bytes;

use crate::session_open::AgentProvider;

/// O recorte do `AgentProvider` que o Rust escreve.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider { Claude, Codex, Pi, Omp, Kimi }

impl Provider {
    /// Nome conhecido de um provedor; modo e rota são decididos pela tabela.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(name: &str) -> Option<Provider> {
        AgentProvider::parse(name)?.try_into().ok()
    }
}

impl From<Provider> for AgentProvider {
    fn from(provider: Provider) -> Self {
        match provider { Provider::Claude => AgentProvider::Claude, Provider::Codex => AgentProvider::Codex, Provider::Pi=>AgentProvider::Pi, Provider::Omp=>AgentProvider::Omp, Provider::Kimi=>AgentProvider::Kimi }
    }
}

impl TryFrom<AgentProvider> for Provider {
    type Error = AgentProvider;

    fn try_from(provider: AgentProvider) -> Result<Self, Self::Error> {
        match provider {
            AgentProvider::Claude => Ok(Provider::Claude),
            AgentProvider::Codex => Ok(Provider::Codex),
            AgentProvider::Pi=>Ok(Provider::Pi),
            AgentProvider::Omp=>Ok(Provider::Omp),
            AgentProvider::Kimi=>Ok(Provider::Kimi),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteRoute {
    Input, Steer, Interrupt, Select, SelectSubmit, Answer, Keys, TermInput, QueueRemove,
    /// Painel da fila: editar, reordenar e orientar uma entrada.
    QueueEdit, QueueReorder, QueueSteer,
    /// Rotas só do Codex (`/models`, `/model`, `/limits`, `/commands`…), com e sem terminal.
    CodexControl,
    /// `/codex-permissions`: como `CodexControl`, e a única que atende o sem terminal caído (`route_write`).
    CodexPermission,
    /// Parar uma tarefa do Claude: o Rust responde por todo provedor, sem rota no Python.
    StopTask,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Owner { Rust, Python }

/// Claude e Codex, com e sem terminal, são do Rust. Entrada doente não muda o dono: `route_write`
/// recusa com código, e o Python nunca relança nem responde por ela.
pub fn decide(route: WriteRoute, provider: Provider, terminal: bool) -> Owner {
    if route == WriteRoute::StopTask { return Owner::Rust; }
    if !terminal && matches!(provider,Provider::Pi|Provider::Omp|Provider::Kimi) {return Owner::Python;}
    match route {
        WriteRoute::CodexControl | WriteRoute::CodexPermission if provider == Provider::Claude => Owner::Python,
        // Teclas cruas e a aba Submit só existem num pane: sem terminal quem responde é o Python, como hoje.
        WriteRoute::Keys | WriteRoute::TermInput | WriteRoute::SelectSubmit if !terminal => Owner::Python,
        _ => Owner::Rust,
    }
}

impl Provider {
    pub fn name(self) -> &'static str { AgentProvider::from(self).name() }
}

/// Provedor+modo (`true` = sem terminal) que a tabela entrega ao Rust na entrada de texto: é o que a
/// saúde anuncia, para o Python não guardar a mesma lista.
pub fn owned_modes() -> Vec<(Provider, bool)> {
    let mut owned = Vec::new();
    for provider in [Provider::Claude, Provider::Codex, Provider::Pi, Provider::Omp, Provider::Kimi] {
        for headless in [true, false] {
            if decide(WriteRoute::Input, provider, !headless) == Owner::Rust { owned.push((provider, headless)); }
        }
    }
    owned
}

/// O corpo tem a forma mínima que a rota espera? Só a forma; o modelo estrito cada rota confere.
/// Sessão do Rust com corpo fora disso recebe `erro_corpo_invalido` daqui, nunca o repasse.
pub(crate) fn body_ok(route: WriteRoute, body: &Bytes) -> bool {
    use WriteRoute::*;
    let object = || serde_json::from_slice::<serde_json::Value>(&super::input::scrub_lone_surrogates(body)).is_ok_and(|v| v.is_object());
    match route {
        Input | Select | Answer | Keys | TermInput | QueueEdit | QueueReorder => object(),
        Steer | SelectSubmit | QueueSteer => body.is_empty() || object(),
        Interrupt | QueueRemove | CodexControl | CodexPermission | StopTask => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use WriteRoute::*;

    const ALL: [WriteRoute; 12] = [Input, Steer, Interrupt, Select, SelectSubmit, Answer, Keys, TermInput, QueueRemove, QueueEdit, QueueReorder, QueueSteer];

    #[test]
    fn with_a_terminal_every_route_is_rust_for_both_providers() {
        for provider in [Provider::Claude, Provider::Codex] {
            for route in ALL { assert_eq!(decide(route, provider, true), Owner::Rust, "{provider:?} {route:?}"); }
        }
    }

    #[test]
    fn without_a_terminal_only_the_pane_routes_go_to_python() {
        for provider in [Provider::Claude, Provider::Codex] {
            for route in ALL {
                let want = if matches!(route, Keys | TermInput | SelectSubmit) { Owner::Python } else { Owner::Rust };
                assert_eq!(decide(route, provider, false), want, "{provider:?} {route:?}");
            }
        }
    }

    #[test]
    fn codex_control_is_rust_for_codex_in_both_modes_only() {
        for terminal in [true, false] {
            assert_eq!(decide(CodexControl, Provider::Codex, terminal), Owner::Rust);
            assert_eq!(decide(CodexControl, Provider::Claude, terminal), Owner::Python);
        }
    }

    #[test]
    fn owned_modes_come_from_the_table() {
        assert_eq!(owned_modes(), vec![(Provider::Claude, true), (Provider::Claude, false), (Provider::Codex, true), (Provider::Codex, false), (Provider::Pi,false), (Provider::Omp,false), (Provider::Kimi,false)]);
    }

    #[test]
    fn provider_names() {
        assert_eq!(Provider::from_str("claude"), Some(Provider::Claude));
        assert_eq!(Provider::from_str("codex"), Some(Provider::Codex));
        assert_eq!(Provider::from_str("pi"), Some(Provider::Pi));
        assert_eq!(Provider::from_str("Claude"), None);
        for provider in [Provider::Claude, Provider::Codex, Provider::Pi, Provider::Omp, Provider::Kimi] {
            assert_eq!(Provider::try_from(AgentProvider::from(provider)), Ok(provider));
            assert_eq!(Provider::from_str(provider.name()), Some(provider));
        }

    }

    #[test]
    fn body_shape() {
        let b = |s: &'static str| Bytes::from_static(s.as_bytes());
        assert!(body_ok(Input, &b(r#"{"text":"oi","x":1}"#)));
        assert!(!body_ok(Input, &b("oi")) && !body_ok(Input, &b("[]")) && !body_ok(Input, &b("")));
        assert!(body_ok(Steer, &b("")) && body_ok(Steer, &b("{}")) && !body_ok(Steer, &b("x")));
        assert!(body_ok(Interrupt, &b("")) && body_ok(QueueRemove, &b("")));
        assert!(body_ok(QueueEdit, &b(r#"{"text":"oi"}"#)) && !body_ok(QueueEdit, &b("")));
        assert!(body_ok(QueueReorder, &b(r#"{"id":"a","before_id":null}"#)) && !body_ok(QueueReorder, &b("x")));
        assert!(body_ok(QueueSteer, &b("")) && body_ok(QueueSteer, &b("{}")) && !body_ok(QueueSteer, &b("x")));
    }
}
