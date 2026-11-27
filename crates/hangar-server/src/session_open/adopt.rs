//! A adoção da reserva pré-aquecida no `POST /api/sessions`: a abertura tira a reserva do `Store` (quem
//! tira primeiro leva), confere se ela é o processo que a abertura subiria e, se for, grava o sidecar com a
//! chave, a conversa e o `cano` dela; o `wake` de sempre conecta ao processo vivo sem subir outro. Não
//! coube: a reserva sai e a abertura segue fria. Plano:
//! `docs/superpowers/plans/2026-10-22-reserva-pre-aquecida-claude.md`, Task 4.
use std::sync::Arc;
use std::time::Instant;

use serde_json::{Value, json};

use super::open::NameClaim;
use super::{OpenError, SessionOpenRequest, SessionSpec};
use crate::prewarm::{Reserve, Store};

/// A reserva tirada para uma abertura e o resultado dela (`adotada` ou `descartada:<motivo>`).
pub struct Adopting {
    store: Arc<Store>,
    reserve: Option<Reserve>,
    /// O nome da reserva, segurado para esta abertura (que o pediu) mesmo quando a reserva sai no meio.
    claim: Option<NameClaim>,
    outcome: Option<String>,
    started: Instant,
}

impl Adopting {
    /// Tira a reserva `id` do `Store`. Nome pedido diferente do dela: ela sai já e a abertura segue fria
    /// com o nome pedido; sem nome, a abertura passa a pedir o nome da reserva.
    pub async fn take(store: Arc<Store>, id: &str, request: &mut SessionOpenRequest) -> Self {
        let mut adopting = Self { reserve: store.take(id), store, claim: None, outcome: None, started: Instant::now() };
        let Some(reserve) = adopting.reserve.as_mut() else {
            adopting.outcome = Some("descartada:ausente".into());
            return adopting;
        };
        // Worktree, terminal, motor etc.: a reserva nunca é o processo dessa abertura.
        if crate::prewarm::adoptable(request).is_err() {
            adopting.discard("fora_do_escopo").await;
            return adopting;
        }
        let asked = request.name.as_deref().map(crate::list::discover_other::sanitize_session_name);
        if asked.as_deref().is_some_and(|asked| asked != reserve.name) {
            adopting.discard("nome_diferente").await;
            return adopting;
        }
        request.name = Some(reserve.name.clone());
        let claim = std::mem::take(&mut reserve.claim);
        claim.promote();
        adopting.claim = Some(claim);
        adopting
    }

    /// O spec que a abertura resolveu contra o da reserva, os dois sem a identidade (`adoption_spec`):
    /// diferente descarta. O resto do lançamento (ambiente do servidor, conta) fica com o `confirm`.
    pub async fn compare(&mut self, resolved: &super::Resolved) {
        let Some(reserve) = &self.reserve else { return };
        if crate::prewarm::adoption_spec(&resolved.spec) != reserve.spec || resolved.previous_non_plan != reserve.previous_non_plan {
            self.discard("impressao").await;
        }
    }

    pub fn reserve(&self) -> Option<&Reserve> { self.reserve.as_ref() }

    /// O nome segurado, se a abertura é desse nome.
    pub(crate) fn claim_for(&mut self, name: &str) -> Option<NameClaim> {
        if self.claim.as_ref().and_then(NameClaim::name) == Some(name) { self.claim.take() } else { None }
    }

    /// Logo antes de gravar, depois do `--prep`: o mesmo nome, o processo vivo e a configuração da conta
    /// igual à que ele leu ao nascer. Não: a reserva sai. Devolve se ainda adota.
    pub(crate) async fn confirm(&mut self, spec: &SessionSpec) -> bool {
        let Some(reserve) = &self.reserve else { return false };
        let (pid, key, dir) = (reserve.cano.pid, reserve.key.clone(), spec.account.config_dir().map(str::to_owned));
        let recorded = reserve.cano.extra.get("config_marca").and_then(Value::as_str).map(str::to_owned);
        // A worktree troca a pasta depois da impressão: o processo da reserva roda na pasta de origem.
        let motivo = if reserve.name != spec.name { Some("nome_diferente") } else if spec.cwd != reserve.cwd || spec.worktree.is_some() {
            Some("pasta_diferente")
        } else {
            let read = tokio::task::spawn_blocking(move || {
                let alive = crate::runtime::process::liveness(pid, &key) == crate::runtime::process::Liveness::Ours;
                (alive, alive.then(|| crate::runtime::gateway::claude_config_stamp(dir.as_deref())).flatten())
            }).await;
            match read {
                Ok((false, _)) => Some("processo_morto"),
                Ok((true, current)) if current != recorded => Some("configuracao"),
                Ok(_) => None,
                Err(_) => Some("conferencia_falhou"),
            }
        };
        match motivo {
            Some(motivo) => { self.discard(motivo).await; false }
            None => true,
        }
    }

    pub(crate) fn was_adopted(&self) -> bool { self.outcome.as_deref() == Some("adotada") }

    /// O sidecar já aponta o processo: o recibo sai (a subida do servidor não pode matá-lo).
    pub(crate) async fn adopted(&mut self) {
        let Some(reserve) = self.reserve.take() else { return };
        self.store.adopted(&reserve.key, &reserve.name).await;
        self.outcome = Some("adotada".into());
    }

    async fn discard(&mut self, motivo: &str) {
        let Some(reserve) = self.reserve.take() else { return };
        self.store.discard_taken(reserve, motivo).await;
        self.outcome = Some(format!("descartada:{motivo}"));
    }

    /// O fim do `POST`: a reserva que sobrou (abertura recusada ou fora do escopo) sai, e o resultado vai
    /// ao diário com o tempo da abertura. Devolve o evento.
    pub async fn finish(mut self, name: &str, error: Option<&OpenError>) -> Value {
        if self.reserve.is_some() { self.discard(error.and_then(|e| e.code).unwrap_or("nao_adotada")).await; }
        let resultado = self.outcome.take().unwrap_or_else(|| "descartada:nao_adotada".into());
        let nivel = if resultado == "adotada" { "ok" } else { "aviso" };
        json!({"evento": "sessao.criar_prewarm", "nivel": nivel, "sessao": name, "provider": "claude", "resultado": resultado,
            "ms": self.started.elapsed().as_millis() as u64})
    }
}
