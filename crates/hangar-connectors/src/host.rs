//! Host dos conectores: uma vida por conector, aberta no primeiro pedido e fechada pela
//! ociosidade, pelo desligamento, pela remoção ou pelo fim do host.
use crate::error::{AdminError, CallError};
use crate::manifest::Manifest;
use crate::process::Life;
use crate::protocol::ErrorCode;
use crate::registry::{Entry, Registry};
use crate::work_tracking::{self, GetParams, ListParams, ListResult, TicketDetail};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::time::Instant;

#[derive(Clone, Copy, Debug)]
pub struct HostOptions {
    /// Lançamento e `initialize`.
    pub init_timeout: Duration,
    /// Uma chamada inteira, incluindo a espera pela vez.
    pub call_timeout: Duration,
    /// Sem pedido por este tempo, a vida acaba.
    pub idle: Duration,
    /// Espera pela saída voluntária antes do encerramento forçado.
    pub exit_grace: Duration,
}

impl Default for HostOptions {
    fn default() -> Self {
        Self { init_timeout: Duration::from_secs(10), call_timeout: Duration::from_secs(30), idle: Duration::from_secs(60), exit_grace: Duration::from_secs(3) }
    }
}

/// Registro mais o estado do processo, sem iniciar nada.
#[derive(Clone, Debug, Serialize)]
pub struct ConnectorStatus {
    #[serde(flatten)]
    pub entry: Entry,
    pub running: bool,
}

#[derive(Default)]
struct Slot {
    life: tokio::sync::Mutex<Option<Life>>,
    /// Muda a cada parada: o pedido em andamento desiste e solta a vez.
    cancel: tokio::sync::watch::Sender<u64>,
    last_used: Mutex<Option<Instant>>,
    running: AtomicBool,
    reaper: AtomicBool,
}

impl Slot {
    /// Fim de uma vida já tirada do slot (com a vez segurada por quem chama).
    async fn end(&self, life: Option<Life>, grace: Duration) {
        let Some(life) = life else { return };
        self.running.store(false, Ordering::Release);
        life.shutdown(grace).await;
    }
}

pub struct Host {
    registry: Registry,
    options: HostOptions,
    slots: Mutex<HashMap<String, Arc<Slot>>>,
}

impl Host {
    pub fn new(registry: Registry, options: HostOptions) -> Arc<Self> {
        Arc::new(Self { registry, options, slots: Mutex::default() })
    }

    fn slot(&self, id: &str) -> Arc<Slot> {
        self.slots.lock().unwrap_or_else(|e| e.into_inner()).entry(id.to_owned()).or_default().clone()
    }

    fn existing_slot(&self, id: &str) -> Option<Arc<Slot>> {
        self.slots.lock().unwrap_or_else(|e| e.into_inner()).get(id).cloned()
    }

    pub fn list(&self) -> Result<Vec<ConnectorStatus>, AdminError> {
        Ok(self.registry.list()?.into_iter().map(|entry| {
            let running = self.existing_slot(&entry.id).is_some_and(|slot| slot.running.load(Ordering::Acquire));
            ConnectorStatus { entry, running }
        }).collect())
    }

    pub fn add(&self, manifest: Manifest) -> Result<Entry, AdminError> { self.registry.add(manifest) }

    pub fn entry(&self, id: &str) -> Result<Entry, AdminError> { self.registry.get(id) }


    /// Ligar só marca; desligar também cancela o pedido em andamento e termina a árvore.
    pub async fn set_enabled(&self, id: &str, enabled: bool) -> Result<Entry, AdminError> {
        let entry = self.registry.set_enabled(id, enabled)?;
        if !enabled { self.stop(id).await; }
        Ok(entry)
    }

    /// Remove depois de confirmar a parada.
    pub async fn remove(&self, id: &str) -> Result<Entry, AdminError> {
        self.registry.set_enabled(id, false)?;
        self.stop(id).await;
        let entry = self.registry.remove(id)?;
        self.slots.lock().unwrap_or_else(|e| e.into_inner()).remove(id);
        Ok(entry)
    }

    /// Fim do host: cancela os pedidos e termina todas as árvores.
    pub async fn shutdown(&self) {
        let ids: Vec<String> = self.slots.lock().unwrap_or_else(|e| e.into_inner()).keys().cloned().collect();
        // Juntos: cada parada pode esperar o prazo de saída inteiro.
        futures_util::future::join_all(ids.iter().map(|id| self.stop(id))).await;
    }

    async fn stop(&self, id: &str) {
        let Some(slot) = self.existing_slot(id) else { return };
        slot.cancel.send_modify(|epoch| *epoch += 1);
        let mut life = slot.life.lock().await;
        slot.end(life.take(), self.options.exit_grace).await;
    }

    pub async fn list_tickets(self: &Arc<Self>, id: &str, params: ListParams) -> Result<ListResult, CallError> {
        let limit = params.limit;
        let value = self.call(id, work_tracking::CAPABILITY, work_tracking::LIST, json!(params)).await?;
        Ok(ListResult::from_connector(value, limit)?)
    }

    pub async fn get_ticket(self: &Arc<Self>, id: &str, params: GetParams) -> Result<TicketDetail, CallError> {
        let value = self.call(id, work_tracking::CAPABILITY, work_tracking::GET, json!(params)).await?;
        Ok(TicketDetail::from_connector(value)?)
    }

    async fn call(self: &Arc<Self>, id: &str, capability: &str, method: &str, params: Value) -> Result<Value, CallError> {
        self.usable(id, capability)?;
        let slot = self.slot(id);
        let mut cancel = slot.cancel.subscribe();
        let work = self.call_in_slot(&slot, id, capability, method, params);
        let outcome = tokio::select! {
            outcome = tokio::time::timeout(self.options.call_timeout, work) => match outcome {
                Ok(outcome) => outcome,
                Err(_) => {
                    self.discard_if_poisoned(&slot);
                    Err(CallError::connector(ErrorCode::Timeout, "o conector não respondeu no prazo"))
                }
            },
            _ = cancel.changed() => Err(CallError::connector(ErrorCode::Unavailable, "pedido cancelado: o conector foi parado")),
        };
        self.touch(&slot);
        if let Err(error) = &outcome { tracing::info!(code = "connector_call_failed", connector = id, method, error = error.code()); }
        outcome
    }

    fn usable(&self, id: &str, capability: &str) -> Result<Entry, CallError> {
        let entry = self.registry.get(id)?;
        if !entry.enabled { return Err(CallError::Disabled(id.to_owned())); }
        if !entry.capabilities.contains_key(capability) {
            return Err(CallError::connector(ErrorCode::UnsupportedCapability, format!("o conector não oferece {capability}")));
        }
        Ok(entry)
    }

    async fn call_in_slot(&self, slot: &Slot, id: &str, capability: &str, method: &str, params: Value) -> Result<Value, CallError> {
        let mut guard = slot.life.lock().await;
        // Relido na vez: desligar ou remover enquanto esperava vale para este pedido.
        let entry = self.usable(id, capability)?;
        let stale = guard.take_if(|life| life.poisoned || life.manifest != entry.manifest);
        slot.end(stale, self.options.exit_grace).await;
        if guard.is_none() {
            *guard = Some(Life::start(&entry.manifest, self.options.init_timeout, self.options.exit_grace).await?);
            slot.running.store(true, Ordering::Release);
        }
        let Some(life) = guard.as_mut() else { return Err(CallError::connector(ErrorCode::Unavailable, "conector sem vida")) };
        life.poisoned = true;
        match life.request(method, params).await {
            Ok(reply) => {
                life.poisoned = false;
                Ok(reply?)
            }
            Err(error) => {
                slot.end(guard.take(), self.options.exit_grace).await;
                Err(error.into())
            }
        }
    }

    /// Pedido abandonado pelo prazo: a resposta ainda pode chegar, então a vida acaba.
    fn discard_if_poisoned(self: &Arc<Self>, slot: &Arc<Slot>) {
        let (slot, grace) = (slot.clone(), self.options.exit_grace);
        tokio::spawn(async move {
            let mut guard = slot.life.lock().await;
            slot.end(guard.take_if(|life| life.poisoned), grace).await;
        });
    }

    fn touch(self: &Arc<Self>, slot: &Arc<Slot>) {
        *slot.last_used.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
        if slot.reaper.swap(true, Ordering::AcqRel) { return; }
        let (slot, idle, grace) = (slot.clone(), self.options.idle, self.options.exit_grace);
        tokio::spawn(async move {
            loop {
                let last = slot.last_used.lock().unwrap_or_else(|e| e.into_inner()).unwrap_or_else(Instant::now);
                tokio::time::sleep_until(last + idle).await;
                let mut guard = slot.life.lock().await;
                let last = slot.last_used.lock().unwrap_or_else(|e| e.into_inner()).unwrap_or_else(Instant::now);
                if last + idle > Instant::now() { continue; }
                slot.end(guard.take(), grace).await;
                slot.reaper.store(false, Ordering::Release);
                return;
            }
        });
    }
}

