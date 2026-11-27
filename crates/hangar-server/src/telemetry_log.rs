//! As linhas de aviso e erro do log que vão à telemetria. A camada só põe a linha num canal limitado
//! (cheio, descarta e conta o descarte); a tarefa de `telemetry_routes` limpa (`telemetry_scrub`) e conta.
use std::fmt::{self, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use hangar_telemetry::wire::is_name;

use tokio::sync::mpsc;
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::{Context, Layer};

const CAPACITY: usize = 256;
/// Os únicos campos do evento que entram, e só com valor no formato de nome: o resto pode trazer
/// texto da pessoa (nome de sessão, caminho, resposta de um serviço).
const SAFE_KEYS: &[&str] = &[
    "code", "codigo", "status", "kind", "cause", "step", "provider", "surface", "error", "ok", "result", "attempt",
    "ms", "http_status", "method",
];

#[derive(Debug, PartialEq)]
pub(crate) struct Line {
    /// Ordem de captura, para o consumo saber o que veio antes do consentimento.
    pub(crate) seq: u64,
    pub(crate) error: bool,
    pub(crate) target: String,
    /// A mensagem e os campos seguros `k=v`; a mensagem ainda sem limpeza.
    pub(crate) message: String,
}

type Channel = (mpsc::Sender<Line>, Mutex<Option<mpsc::Receiver<Line>>>, Arc<AtomicU64>);

static CHANNEL: LazyLock<Channel> = LazyLock::new(|| {
    let (tx, rx) = mpsc::channel(CAPACITY);
    (tx, Mutex::new(Some(rx)), Arc::new(AtomicU64::new(0)))
});

static SEQ: AtomicU64 = AtomicU64::new(0);

/// A próxima ordem de captura: toda linha capturada daqui em diante tem ordem igual ou maior.
pub(crate) fn next_seq() -> u64 { SEQ.load(Ordering::SeqCst) }

/// O que o consumo deixa passar: só linha capturada depois da ligação atual da contagem. Linha que
/// esperava no canal quando a pessoa consentiu é de antes do consentimento e não conta.
pub(crate) struct ConsentGate {
    epoch: u64,
    from: u64,
}

impl ConsentGate {
    /// A época em que o consumo começa: ligada desde a subida, o consentimento é de antes desta vida.
    pub(crate) fn new(epoch: u64) -> ConsentGate { ConsentGate { epoch, from: 0 } }

    /// `next_seq` lido depois da época: o que veio entre ligar e esta leitura também fica fora.
    pub(crate) fn update(&mut self, epoch: u64, next_seq: u64) {
        if epoch != self.epoch {
            self.epoch = epoch;
            self.from = next_seq;
        }
    }

    pub(crate) fn admits(&self, line: &Line) -> bool { line.seq >= self.from }
}

/// Quantas linhas o canal cheio descartou desde a última leitura.
pub(crate) fn take_dropped() -> u64 { CHANNEL.2.swap(0, Ordering::Relaxed) }

/// O consumidor desta vida; só o primeiro pedido leva.
pub(crate) fn take_lines() -> Option<mpsc::Receiver<Line>> { CHANNEL.1.lock().ok()?.take() }

/// A camada que `init_log` instala ao lado do `fmt`.
pub fn layer() -> Capture { Capture(CHANNEL.0.clone(), CHANNEL.2.clone()) }

pub struct Capture(mpsc::Sender<Line>, Arc<AtomicU64>);

impl<S: Subscriber> Layer<S> for Capture {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let meta = event.metadata();
        let error = match *meta.level() {
            Level::ERROR => true,
            Level::WARN => false,
            _ => return,
        };
        // O aviso da própria telemetria viraria outra linha a contar.
        let target = meta.target();
        if target.starts_with("hangar_telemetry") || target.starts_with("hangar_server::telemetry") {
            return;
        }
        let mut text = Text::default();
        event.record(&mut text);
        let seq = SEQ.fetch_add(1, Ordering::SeqCst);
        if self.0.try_send(Line { seq, error, target: target.to_owned(), message: text.0 }).is_err() {
            self.1.fetch_add(1, Ordering::Relaxed);
        }
    }
}

#[derive(Default)]
struct Text(String);

impl Text {
    fn push(&mut self, args: fmt::Arguments<'_>) {
        if !self.0.is_empty() {
            self.0.push(' ');
        }
        let _ = self.0.write_fmt(args);
    }

    /// Campo de chave segura com valor de nome; em minúsculas porque o `kind` do `io::Error` vem `NotFound`.
    fn field(&mut self, key: &str, value: &str) {
        let value = value.to_ascii_lowercase();
        if SAFE_KEYS.contains(&key) && is_name(&value) {
            self.push(format_args!("{key}={value}"));
        }
    }
}

impl Visit for Text {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" { self.push(format_args!("{value}")) } else { self.field(field.name(), value) }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if field.name() == "message" { self.push(format_args!("{value:?}")) } else { self.field(field.name(), &format!("{value:?}")) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing_subscriber::layer::SubscriberExt;

    #[test]
    fn only_warn_and_error_go_out_with_their_fields() {
        let (tx, mut rx) = mpsc::channel(8);
        let subscriber = tracing_subscriber::registry().with(Capture(tx, Arc::default()));
        // Com um assinante só, o interesse que outro teste em paralelo gravou desligaria estes pontos
        // (o mesmo cuidado de `groups/store.rs`).
        let _second = tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default());
        tracing::subscriber::with_default(subscriber, || {
            // O alvo deste módulo é da telemetria e seria ignorado: os pontos levam o de outro.
            tracing::info!(target: "hangar_server::list", "só no arquivo");
            tracing::warn!(target: "hangar_server::list", code = "list_failed", n = 3, session = "minha-sessao", "lista não lida: {}", "x");
            tracing::error!(target: "hangar_server::telemetry_routes", "da própria telemetria");
            tracing::error!(target: "hangar_telemetry::send", "do crate");
            tracing::error!(target: "hangar_server::list", kind = ?std::io::ErrorKind::NotFound, code = "Fulano Silva", status = 502, "sumiu");
        });
        let target = "hangar_server::list".to_owned();
        let (first, second) = (rx.try_recv().unwrap(), rx.try_recv().unwrap());
        assert!(second.seq > first.seq, "a ordem de captura cresce");
        assert_eq!((first.error, first.target, first.message), (false, target.clone(), "lista não lida: x code=list_failed".to_owned()));
        assert_eq!((second.error, second.target, second.message), (true, target, "sumiu kind=notfound status=502".to_owned()), "código fora do formato sai inteiro");
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn lines_captured_before_consent_are_not_admitted() {
        let line = |seq| Line { seq, error: true, target: "hangar_server::list".into(), message: "falhou".into() };
        let mut gate = ConsentGate::new(0);
        assert!(gate.admits(&line(0)), "ligada desde a subida");
        gate.update(1, 5);
        assert!(!gate.admits(&line(4)), "esperava no canal quando a pessoa consentiu");
        assert!(gate.admits(&line(5)));
        gate.update(1, 9);
        assert!(gate.admits(&line(6)), "a mesma época não move o corte");
        gate.update(2, 12);
        assert!(!gate.admits(&line(11)));
    }

    #[test]
    fn full_channel_drops_instead_of_waiting() {
        let (tx, mut rx) = mpsc::channel(1);
        let dropped = Arc::new(AtomicU64::new(0));
        let _second = tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default());
        tracing::subscriber::with_default(tracing_subscriber::registry().with(Capture(tx, dropped.clone())), || {
            tracing::warn!(target: "hangar_server::list", "primeira");
            tracing::warn!(target: "hangar_server::list", "segunda");
            tracing::warn!(target: "hangar_server::list", "terceira");
        });
        assert_eq!(rx.try_recv().unwrap().message, "primeira");
        assert!(rx.try_recv().is_err());
        assert_eq!(dropped.load(Ordering::Relaxed), 2, "o descarte é contado");
    }
}
