//! As linhas de aviso e erro do log que vão à telemetria. A camada só põe a linha num canal limitado
//! (cheio, descarta); a tarefa de `telemetry_routes` limpa (`telemetry_scrub`) e conta.
use std::fmt::{self, Write};
use std::sync::{LazyLock, Mutex};

use tokio::sync::mpsc;
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::{Context, Layer};

const CAPACITY: usize = 256;

#[derive(Debug, PartialEq)]
pub(crate) struct Line {
    pub(crate) error: bool,
    pub(crate) target: String,
    /// A mensagem e os campos `k=v`, como o `fmt` escreve no arquivo; ainda sem limpeza.
    pub(crate) message: String,
}

type Channel = (mpsc::Sender<Line>, Mutex<Option<mpsc::Receiver<Line>>>);

static CHANNEL: LazyLock<Channel> = LazyLock::new(|| {
    let (tx, rx) = mpsc::channel(CAPACITY);
    (tx, Mutex::new(Some(rx)))
});

/// O consumidor desta vida; só o primeiro pedido leva.
pub(crate) fn take_lines() -> Option<mpsc::Receiver<Line>> { CHANNEL.1.lock().ok()?.take() }

/// A camada que `init_log` instala ao lado do `fmt`.
pub fn layer() -> Capture { Capture(CHANNEL.0.clone()) }

pub struct Capture(mpsc::Sender<Line>);

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
        let _ = self.0.try_send(Line { error, target: target.to_owned(), message: text.0 });
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
}

impl Visit for Text {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" { self.push(format_args!("{value}")) } else { self.push(format_args!("{}={value:?}", field.name())) }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if field.name() == "message" { self.push(format_args!("{value:?}")) } else { self.push(format_args!("{}={value:?}", field.name())) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing_subscriber::layer::SubscriberExt;

    #[test]
    fn only_warn_and_error_go_out_with_their_fields() {
        let (tx, mut rx) = mpsc::channel(8);
        let subscriber = tracing_subscriber::registry().with(Capture(tx));
        // Com um assinante só, o interesse que outro teste em paralelo gravou desligaria estes pontos
        // (o mesmo cuidado de `groups/store.rs`).
        let _second = tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default());
        tracing::subscriber::with_default(subscriber, || {
            // O alvo deste módulo é da telemetria e seria ignorado: os pontos levam o de outro.
            tracing::info!(target: "hangar_server::list", "só no arquivo");
            tracing::warn!(target: "hangar_server::list", code = "list_failed", n = 3, "lista não lida: {}", "x");
            tracing::error!(target: "hangar_server::telemetry_routes", "da própria telemetria");
            tracing::error!(target: "hangar_telemetry::send", "do crate");
            tracing::error!(target: "hangar_server::list", kind = ?std::io::ErrorKind::NotFound, "sumiu");
        });
        let target = "hangar_server::list".to_owned();
        assert_eq!(rx.try_recv().unwrap(), Line { error: false, target: target.clone(), message: r#"lista não lida: x code="list_failed" n=3"#.into() });
        assert_eq!(rx.try_recv().unwrap(), Line { error: true, target, message: "sumiu kind=NotFound".into() });
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn full_channel_drops_instead_of_waiting() {
        let (tx, mut rx) = mpsc::channel(1);
        let _second = tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default());
        tracing::subscriber::with_default(tracing_subscriber::registry().with(Capture(tx)), || {
            tracing::warn!(target: "hangar_server::list", "primeira");
            tracing::warn!(target: "hangar_server::list", "segunda");
        });
        assert_eq!(rx.try_recv().unwrap().message, "primeira");
        assert!(rx.try_recv().is_err());
    }
}
