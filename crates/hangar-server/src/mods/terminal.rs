//! O elo da sessão com terminal que o Rust atende: leva os pedidos dos apps ao clique (`click.rs`), dentro
//! do prazo de quem pediu, lê o painel na frente pela tela e vigia o tamanho do terminal (T9 e T10). Sem
//! terminal de verdade ligado à sessão o Hangar é dono do tamanho e repõe o mínimo; com um ligado, o
//! tamanho é da pessoa.
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::click::{self, Limits, Pane, Parts, Undo};
use super::model::{ModsCall, no_answer};
use super::state::{CallFuture, Mods, ShownFuture, SurfaceLink, TerminalProbe};
use crate::runtime::terminal::ModsAnchor;

/// Prazo da reposição do mínimo pelo vigia: redimensionar e assentar (até 1 s) com o piso das ações.
const FLOOR_BUDGET: Duration = Duration::from_secs(5);
/// Depois disto esperando a vez do pane, o vigia avisa no diário e segue esperando: passa do pedido mais longo
/// (7,5 s) com a limpeza mais longa (~32 s), então é um clique preso.
const FLOOR_WAIT_WARN: Duration = Duration::from_secs(45);
/// Prazo da leitura do painel na frente, que não é pedido de app.
const SHOWN_READ_MAX: Duration = Duration::from_secs(2);

pub struct TerminalLink {
    parts: Parts,
    watch: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// A âncora da faixa que o executor do terminal lê antes de escrever uma entrada.
    anchor: ModsAnchor,
}

/// Aborta a tarefa ao sair de escopo: o leitor do vigia morre junto com o laço que o consome.
struct AbortOnDrop(tokio::task::JoinHandle<()>);
impl Drop for AbortOnDrop {
    fn drop(&mut self) { self.0.abort(); }
}

impl TerminalLink {
    /// `life` é a vida com que a sessão é ligada (`Mods::attach_terminal`): o clique, a leitura do painel na
    /// frente e o vigia só leem e escrevem o registro dessa vida.
    pub fn new(name: String, life: u64, pane: Arc<dyn Pane>, mods: Mods, limits: Limits) -> Arc<Self> {
        Self::anchored(name, life, pane, mods, limits, ModsAnchor::default())
    }

    /// `new` com a âncora do executor do terminal (`TerminalHandle::anchor`), que o elo mantém em dia.
    pub fn anchored(name: String, life: u64, pane: Arc<dyn Pane>, mods: Mods, limits: Limits, anchor: ModsAnchor) -> Arc<Self> {
        Arc::new(Self { parts: Parts { name, pane, mods, limits, busy: Arc::default(), life, clicked: Arc::default() }, watch: Mutex::new(None), anchor })
    }

    /// Repõe o tamanho mínimo quando nenhum terminal de verdade está ligado (T9). Com um pedido do app em
    /// curso espera a vez dele sem prazo, e relê clientes e tamanho assim que o clique solta o pane: o
    /// terminal que se desliga no meio do clique não avisa de novo, e desistir deixaria a janela abaixo do
    /// mínimo. A espera acaba: todo clique solta a vez no fim da limpeza, que tem prazo.
    pub async fn floor(&self) {
        let _busy = loop {
            match tokio::time::timeout(FLOOR_WAIT_WARN, self.parts.busy.lock()).await {
                Ok(busy) => break busy,
                Err(_) => tracing::warn!(session = %self.parts.name, code = "mods_floor_wait",
                    "o clique segura o pane além do teto; o tamanho mínimo do terminal segue esperando a vez"),
            }
        };
        let undo = Undo::default();
        if let Err(error) = click::floor(&self.parts.ctx(Instant::now() + FLOOR_BUDGET, &undo)).await {
            tracing::warn!(session = %self.parts.name, code = %error.code, "tamanho mínimo do terminal não reposto");
        }
    }

    /// Liga o vigia (T10): a cada aviso de cliente ou de janela, relê clientes e tamanho e repõe o mínimo se
    /// ninguém estiver ligado. Um aumento de altura em curso não precisa de aviso: o `window-size latest`
    /// que acompanha todo redimensionamento já entrega o tamanho a quem se ligar. No Windows o
    /// `watch_notices` recusa, e o mínimo volta no `prepare` de cada operação.
    pub fn watch(self: &Arc<Self>, mux_argv: &[String]) {
        match crate::terminal_control::watch_notices(mux_argv, &self.parts.name) {
            Ok((mut notices, reader)) => {
                let link = Arc::downgrade(self);
                let task = tokio::spawn(async move {
                    let _reader = AbortOnDrop(reader);
                    while notices.recv().await.is_some() {
                        // Uma reposição atende a rajada inteira: ela lê o estado de agora.
                        while notices.try_recv().is_ok() {}
                        let Some(link) = link.upgrade() else { return };
                        link.floor().await;
                    }
                });
                if let Some(old) = self.watch.lock().unwrap().replace(task) { old.abort(); }
            }
            // No Windows a recusa é o esperado (ruling C7): não é aviso.
            Err(error) if cfg!(windows) => tracing::debug!(session = %self.parts.name, code = error.0, "vigia de tamanho do terminal recusado"),
            Err(error) => tracing::warn!(session = %self.parts.name, code = error.0, "vigia de tamanho do terminal não subiu"),
        }
    }

    fn stop_watch(&self) {
        if let Some(task) = self.watch.lock().unwrap().take() { task.abort(); }
    }
}

/// O elo que ninguém mais tem não deixa o cliente de controle do vigia ligado à sessão.
impl Drop for TerminalLink {
    fn drop(&mut self) { self.stop_watch(); }
}

impl SurfaceLink for TerminalLink {
    /// O pedido roda numa tarefa própria (`click::spawn`): se a rota desistir no fim do orçamento, ele não
    /// começa ação nova e a limpeza roda mesmo assim. A resposta chega antes da limpeza.
    fn call(&self, call: ModsCall, deadline: Instant) -> CallFuture {
        let (_task, reply) = click::spawn(self.parts.clone(), call, deadline);
        Box::pin(async move { reply.await.unwrap_or_else(|_| Err(no_answer())) })
    }
}

impl TerminalProbe for TerminalLink {
    fn read_shown(&self) -> ShownFuture {
        let parts = self.parts.clone();
        Box::pin(async move {
            let undo = Undo::default();
            click::read_shown(&parts.ctx(Instant::now() + SHOWN_READ_MAX, &undo)).await
        })
    }

    /// Encerra o vigia e, com ele, o cliente de controle (`kill_on_drop`).
    fn stop(&self) { self.stop_watch(); }

    fn anchor(&self, anchor: Option<String>) { *self.anchor.lock().unwrap() = anchor; }
}
