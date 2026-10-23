//! Porta de entrada das escritas por nome de sessão. Quem troca de dono ou relança (Python) fecha;
//! a rota do Rust espera reabrir em vez de escrever numa entrada que está fechando. Fechamento
//! retido (troca de conversa, que pode parar indefinidamente) recusa na hora em vez de esperar.
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::watch;

#[derive(Clone, Default)]
pub struct IngressGates { inner: Arc<Mutex<HashMap<String, Gate>>> }

// Fechamentos em curso (contador, não flag: dois `close` não desfazem um ao outro), quantos deles são
// retidos, quantos são exclusivos (rename) e escritas em curso.
struct Gate { closes: watch::Sender<usize>, holds: usize, exclusive: usize, passes: watch::Sender<usize> }

#[derive(Debug)]
pub struct GateClosed;
#[derive(Debug)]
pub struct IngressBusy;
#[derive(Debug)]
pub enum CloseRefused { Busy, Held }
pub struct IngressPass { gates: IngressGates, name: String }

/// Desfaz o fechamento de um `close` que desistiu ou foi cancelado no meio da espera.
struct CloseGuard<'a> { gates: &'a IngressGates, name: &'a str, held: bool, exclusive: bool, armed: bool }
impl Drop for CloseGuard<'_> {
    fn drop(&mut self) { if self.armed { self.gates.reopen(self.name, self.held, self.exclusive); } }
}

impl Gate {
    fn idle(&self) -> bool {
        *self.closes.borrow() == 0 && self.holds == 0 && self.exclusive == 0 && *self.passes.borrow() == 0 && self.closes.receiver_count() == 0 && self.passes.receiver_count() == 0
    }
}

impl IngressGates {
    fn with_gate<T>(&self, name: &str, f: impl FnOnce(&mut Gate) -> T) -> T {
        let mut map = self.inner.lock().unwrap();
        f(map.entry(name.into()).or_insert_with(|| Gate { closes: watch::channel(0).0, holds: 0, exclusive: 0, passes: watch::channel(0).0 }))
    }
    pub async fn enter(&self, name: &str, wait: Duration) -> Result<IngressPass, GateClosed> {
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            // Conferir e contar sob a mesma trava do `close`: senão um fechamento passa entre os dois.
            let mut closes = match self.with_gate(name, |g| {
                if g.holds > 0 { Ok(None) }
                else if *g.closes.borrow() > 0 { Err(g.closes.subscribe()) }
                else { g.passes.send_modify(|n| *n += 1); Ok(Some(())) }
            }) {
                Ok(Some(())) => return Ok(IngressPass { gates: self.clone(), name: name.into() }),
                Ok(None) => return Err(GateClosed),
                Err(rx) => rx,
            };
            // Qualquer mudança no contador reconfere: uma retenção que chega durante a espera recusa já.
            tokio::time::timeout_at(deadline, closes.changed()).await.map_err(|_| GateClosed)?.map_err(|_| GateClosed)?;
        }
    }
    pub async fn close(&self, name: &str, wait: Duration) -> Result<(), IngressBusy> { self.close_as(name, wait, false, false, false).await.map_err(|_| IngressBusy) }
    /// Fechamento que não passa por cima de um retido: com a troca de conversa em curso, `Held` na hora.
    pub async fn close_unless_held(&self, name: &str, wait: Duration) -> Result<(), CloseRefused> { self.close_as(name, wait, false, true, false).await }
    /// Como `close_unless_held`, e enquanto durar a retenção (`hold`) recusa: a troca de conversa não
    /// começa sobre um rename em curso. Desfeito por `open_exclusive`.
    pub async fn close_exclusive(&self, name: &str, wait: Duration) -> Result<(), CloseRefused> { self.close_as(name, wait, false, true, true).await }
    /// Fechamento retido: enquanto durar, `enter` recusa sem esperar. Desfeito por `release`.
    pub async fn hold(&self, name: &str, wait: Duration) -> Result<(), IngressBusy> { self.close_as(name, wait, true, false, false).await.map_err(|_| IngressBusy) }
    async fn close_as(&self, name: &str, wait: Duration, held: bool, refuse_held: bool, exclusive: bool) -> Result<(), CloseRefused> {
        // Declarado antes de `passes`: o receptor cai primeiro e a porta desistida pode sair do mapa.
        let mut guard = CloseGuard { gates: self, name, held, exclusive, armed: false };
        // Conferido sob a mesma trava que conta o fechamento: uma retenção não passa entre os dois.
        let mut passes = self.with_gate(name, |g| {
            if refuse_held && g.holds > 0 { return Err(CloseRefused::Held); }
            if held && g.exclusive > 0 { return Err(CloseRefused::Busy); }
            g.closes.send_modify(|n| *n += 1);
            if held { g.holds += 1; }
            if exclusive { g.exclusive += 1; }
            Ok(g.passes.subscribe())
        })?;
        guard.armed = true;
        if tokio::time::timeout(wait, passes.wait_for(|n| *n == 0)).await.is_err() { return Err(CloseRefused::Busy); }
        guard.armed = false;
        Ok(())
    }
    /// Desfaz um fechamento (o contador não passa de zero).
    pub fn open(&self, name: &str) { self.reopen(name, false, false) }
    /// Desfaz um `close_exclusive`.
    pub fn open_exclusive(&self, name: &str) { self.reopen(name, false, true) }
    /// Desfaz um fechamento retido.
    pub fn release(&self, name: &str) { self.reopen(name, true, false) }
    fn reopen(&self, name: &str, held: bool, exclusive: bool) {
        let mut map = self.inner.lock().unwrap();
        let Some(g) = map.get_mut(name) else { return };
        if held { g.holds = g.holds.saturating_sub(1); }
        if exclusive { g.exclusive = g.exclusive.saturating_sub(1); }
        g.closes.send_modify(|n| *n = n.saturating_sub(1));
        // Aberta e sem escrita em curso é igual a não existir: o mapa não cresce com nomes antigos.
        if g.idle() { map.remove(name); }
    }
}

impl Drop for IngressPass {
    fn drop(&mut self) {
        let mut map = self.gates.inner.lock().unwrap();
        let Some(g) = map.get(&self.name) else { return };
        g.passes.send_modify(|n| *n -= 1);
        if g.idle() { map.remove(&self.name); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn rename_and_conversation_transfer_refuse_each_other() {
        let gates = IngressGates::default();
        gates.close_exclusive("k", Duration::from_millis(10)).await.unwrap();
        assert!(gates.hold("k", Duration::from_secs(5)).await.is_err());
        gates.open_exclusive("k");
        gates.hold("k", Duration::from_millis(10)).await.unwrap();
        assert!(matches!(gates.close_exclusive("k", Duration::from_secs(5)).await, Err(CloseRefused::Held)));
        gates.release("k");
        assert!(gates.inner.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn close_waits_for_passes_and_blocks_new_ones() {
        let gates = IngressGates::default();
        let pass = gates.enter("k", Duration::from_millis(10)).await.unwrap();
        let g = gates.clone();
        let closing = tokio::spawn(async move { g.close("k", Duration::from_secs(1)).await.is_ok() });
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!closing.is_finished());
        drop(pass);
        assert!(closing.await.unwrap());
        assert!(gates.enter("k", Duration::from_millis(20)).await.is_err());
        gates.open("k");
        assert!(gates.enter("k", Duration::from_millis(20)).await.is_ok());
    }

    #[tokio::test]
    async fn closed_gate_lets_writer_through_when_reopened_in_time() {
        let gates = IngressGates::default();
        assert!(gates.close("k", Duration::from_secs(1)).await.is_ok());
        let g = gates.clone();
        tokio::spawn(async move { tokio::time::sleep(Duration::from_millis(30)).await; g.open("k") });
        assert!(gates.enter("k", Duration::from_secs(1)).await.is_ok());
    }

    #[tokio::test]
    async fn close_gives_up_busy_and_reopens() {
        let gates = IngressGates::default();
        let _pass = gates.enter("k", Duration::from_millis(10)).await.unwrap();
        assert!(gates.close("k", Duration::from_millis(30)).await.is_err());
        assert!(gates.enter("k", Duration::from_millis(20)).await.is_ok());
    }

    #[tokio::test]
    async fn gates_are_per_name() {
        let gates = IngressGates::default();
        assert!(gates.close("a", Duration::from_millis(20)).await.is_ok());
        assert!(gates.enter("b", Duration::from_millis(20)).await.is_ok());
        assert!(gates.enter("a", Duration::from_millis(20)).await.is_err());
    }

    #[tokio::test]
    async fn idle_gate_is_dropped_from_map() {
        let gates = IngressGates::default();
        drop(gates.enter("k", Duration::from_millis(10)).await.unwrap());
        assert!(gates.inner.lock().unwrap().is_empty());
        gates.close("k", Duration::from_millis(10)).await.unwrap();
        gates.open("k");
        assert!(gates.inner.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn no_pass_after_close_returns_until_open() {
        let gates = IngressGates::default();
        let mut tasks = Vec::new();
        for i in 0..100u64 {
            let g = gates.clone();
            tasks.push(tokio::spawn(async move {
                tokio::time::sleep(Duration::from_micros(i * 50)).await;
                let pass = g.enter("k", Duration::from_millis(5)).await;
                if pass.is_ok() { tokio::time::sleep(Duration::from_millis(1)).await; }
                pass.is_ok()
            }));
        }
        tokio::time::sleep(Duration::from_micros(2000)).await;
        gates.close("k", Duration::from_secs(1)).await.unwrap();
        // Depois que o close volta, nenhum enter novo passa até o open.
        let late: Vec<_> = (0..100).map(|_| { let g = gates.clone(); tokio::spawn(async move { g.enter("k", Duration::from_millis(5)).await.is_ok() }) }).collect();
        for t in late { assert!(!t.await.unwrap()); }
        for t in tasks { let _ = t.await; }
        gates.open("k");
        assert!(gates.enter("k", Duration::from_millis(20)).await.is_ok());
    }

    #[tokio::test]
    async fn timed_out_close_does_not_reopen_another_close() {
        let gates = IngressGates::default();
        let pass = gates.enter("k", Duration::from_millis(10)).await.unwrap();
        assert!(gates.close("k", Duration::from_millis(30)).await.is_err());
        drop(pass);
        assert!(gates.close("k", Duration::from_millis(30)).await.is_ok());
        assert!(gates.enter("k", Duration::from_millis(20)).await.is_err());
        // Primeiro close que deu certo: um open por close bem-sucedido.
        let held = gates.enter("j", Duration::from_millis(10)).await.unwrap();
        let g = gates.clone();
        let first = tokio::spawn(async move { g.close("j", Duration::from_secs(5)).await });
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(gates.close("j", Duration::from_millis(20)).await.is_err());
        drop(held);
        assert!(first.await.unwrap().is_ok());
        assert!(gates.enter("j", Duration::from_millis(20)).await.is_err());
        gates.open("j");
        assert!(gates.enter("j", Duration::from_millis(20)).await.is_ok());
    }

    #[tokio::test]
    async fn two_successful_closes_need_two_opens() {
        let gates = IngressGates::default();
        gates.close("k", Duration::from_millis(10)).await.unwrap();
        gates.close("k", Duration::from_millis(10)).await.unwrap();
        gates.open("k");
        assert!(gates.enter("k", Duration::from_millis(20)).await.is_err());
        gates.open("k");
        assert!(gates.enter("k", Duration::from_millis(20)).await.is_ok());
        gates.open("k");
        gates.open("k");
        assert!(gates.enter("k", Duration::from_millis(20)).await.is_ok());
    }

    #[tokio::test]
    async fn held_close_refuses_at_once_and_plain_close_still_waits() {
        let gates = IngressGates::default();
        gates.hold("k", Duration::from_millis(10)).await.unwrap();
        let start = std::time::Instant::now();
        assert!(gates.enter("k", Duration::from_secs(5)).await.is_err());
        assert!(start.elapsed() < Duration::from_millis(100));
        gates.release("k");
        assert!(gates.enter("k", Duration::from_millis(20)).await.is_ok());
        assert!(gates.inner.lock().unwrap().is_empty());

        gates.close("p", Duration::from_millis(10)).await.unwrap();
        let start = std::time::Instant::now();
        assert!(gates.enter("p", Duration::from_millis(80)).await.is_err());
        assert!(start.elapsed() >= Duration::from_millis(80));
    }

    #[tokio::test]
    async fn hold_arriving_while_waiting_refuses_the_waiter() {
        let gates = IngressGates::default();
        gates.close("k", Duration::from_millis(10)).await.unwrap();
        let g = gates.clone();
        let waiter = tokio::spawn(async move { let s = std::time::Instant::now(); (g.enter("k", Duration::from_secs(5)).await.is_ok(), s.elapsed()) });
        tokio::time::sleep(Duration::from_millis(20)).await;
        gates.hold("k", Duration::from_millis(10)).await.unwrap();
        let (ok, elapsed) = waiter.await.unwrap();
        assert!(!ok && elapsed < Duration::from_millis(200));
    }

    #[tokio::test]
    async fn held_and_plain_closes_count_separately() {
        let gates = IngressGates::default();
        gates.hold("k", Duration::from_millis(10)).await.unwrap();
        gates.close("k", Duration::from_millis(10)).await.unwrap();
        // Só o fechamento comum desfeito: a retenção continua e a recusa segue imediata.
        gates.open("k");
        let start = std::time::Instant::now();
        assert!(gates.enter("k", Duration::from_secs(5)).await.is_err());
        assert!(start.elapsed() < Duration::from_millis(100));
        gates.hold("k", Duration::from_millis(10)).await.unwrap();
        gates.close("k", Duration::from_millis(10)).await.unwrap();
        gates.release("k");
        gates.release("k");
        // Sem retenção, o fechamento comum que sobrou volta a esperar a reabertura.
        let g = gates.clone();
        tokio::spawn(async move { tokio::time::sleep(Duration::from_millis(30)).await; g.open("k") });
        assert!(gates.enter("k", Duration::from_secs(1)).await.is_ok());
    }

    #[tokio::test]
    async fn cancelled_hold_releases() {
        let gates = IngressGates::default();
        let pass = gates.enter("k", Duration::from_millis(10)).await.unwrap();
        assert!(gates.hold("k", Duration::from_millis(30)).await.is_err());
        drop(pass);
        assert!(gates.enter("k", Duration::from_millis(20)).await.is_ok());
        assert!(gates.inner.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn cancelled_close_leaves_gate_open() {
        let gates = IngressGates::default();
        let _pass = gates.enter("k", Duration::from_millis(10)).await.unwrap();
        let g = gates.clone();
        let closing = tokio::spawn(async move { g.close("k", Duration::from_secs(30)).await });
        tokio::time::sleep(Duration::from_millis(20)).await;
        closing.abort();
        let _ = closing.await;
        assert!(gates.enter("k", Duration::from_millis(20)).await.is_ok());
    }
}
