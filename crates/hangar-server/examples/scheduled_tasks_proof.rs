//! Servidor de prova isolado: usa o agendador e as rotas reais, com entrega gravada em arquivo.
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

use hangar_scheduled_tasks::{
    Attempt, Dispatcher, Engine, Error, Host, HostFuture, Outcome, Status, Store, SystemClock,
    Target,
};
use hangar_server::{
    accounts::{AccountService, environment::AccountEnvironment},
    auth::TrustedHosts,
    config::Config,
    routes::AppState,
    scheduled_tasks::Service,
};
use serde_json::json;

struct ProofHost {
    directory: PathBuf,
    pause_after_delivery: bool,
}
impl Host for ProofHost {
    fn prepare_target(&self, target: Target) -> HostFuture<'_, Result<Target, Error>> {
        Box::pin(async move {
            Ok(match target {
                Target::Session {
                    name,
                    session_key: _,
                } => Target::Session {
                    name,
                    session_key: Some("proof-life".into()),
                },
                other => other,
            })
        })
    }
    fn dispatch(&self, attempt: Attempt) -> HostFuture<'_, Outcome> {
        Box::pin(async move {
            let session = match &attempt.task.input.target {
                Target::Session { name, .. } => name.clone(),
                _ => attempt.session_name.clone(),
            };
            let row = json!({"attempt_id":attempt.id,"task_id":attempt.task.id,"name":session,"prompt":attempt.task.input.prompt,"at":chrono::Utc::now().to_rfc3339()});
            let written: std::io::Result<()> = (|| {
                let mut file = std::fs::OpenOptions::new()
                    .append(true)
                    .create(true)
                    .open(self.directory.join("deliveries.jsonl"))?;
                writeln!(file, "{row}")?;
                file.sync_all()
            })();
            if self.pause_after_delivery && written.is_ok() {
                std::future::pending::<()>().await;
            }
            Outcome {
                status: if written.is_ok() {
                    Status::Sent
                } else {
                    Status::Uncertain
                },
                error: written
                    .err()
                    .map(|_| "Não foi possível gravar a entrega de prova.".into()),
                session: Some(session),
            }
        })
    }
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let directory = std::env::var_os("HANGAR_SCHEDULED_PROOF_DIR")
        .map(PathBuf::from)
        .ok_or("Informe HANGAR_SCHEDULED_PROOF_DIR com uma pasta temporária exclusiva.")?;
    std::fs::create_dir_all(&directory)?;
    let address: std::net::SocketAddr = std::env::var("HANGAR_SCHEDULED_PROOF_LISTEN")
        .unwrap_or_else(|_| "127.0.0.1:8793".into())
        .parse()?;
    if !address.ip().is_loopback() {
        return Err("A prova só escuta em loopback.".into());
    }
    let timezone = std::env::var("HANGAR_SCHEDULED_PROOF_TIMEZONE")
        .unwrap_or_else(|_| "America/Sao_Paulo".into())
        .parse::<chrono_tz::Tz>()?;
    let token =
        std::env::var("HANGAR_SCHEDULED_PROOF_TOKEN").unwrap_or_else(|_| "scheduled-proof".into());
    let store = Arc::new(Store::open(&directory.join("tasks.sqlite3"))?);
    store.recover()?;
    let engine = Arc::new(Engine::new(
        store,
        timezone,
        Arc::new(ProofHost {
            directory: directory.clone(),
            pause_after_delivery: std::env::var("HANGAR_SCHEDULED_PROOF_PAUSE_AFTER_DELIVERY")
                .is_ok_and(|value| value == "1"),
        }),
        Arc::new(SystemClock),
    ));
    let (handle, dispatcher) = Dispatcher::new(engine.clone(), true);
    let mut state = AppState::new(Config {
        listen: address,
        upstream: "127.0.0.1:1".parse()?,
        internal_secret: "proof-internal".into(),
        auth_token: token,
        log_path: None,
        trusted: TrustedHosts::parse("127.0.0.1"),
    });
    state.accounts = AccountService::new(AccountEnvironment::from_map(
        [
            ("HOME".into(), directory.to_string_lossy().into_owned()),
            (
                "USERPROFILE".into(),
                directory.to_string_lossy().into_owned(),
            ),
        ]
        .into(),
    ));
    state.scheduled_tasks = Arc::new(Service::with_engine(engine, handle));
    let listener = tokio::net::TcpListener::bind(address).await?;
    println!(
        "Prova de tarefas: http://{address}; entregas em {}",
        directory.join("deliveries.jsonl").display()
    );
    let worker = tokio::spawn(dispatcher.run());
    let result = axum::serve(
        listener,
        hangar_server::scheduled_tasks::router()
            .with_state(Arc::new(state))
            .into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await;
    worker.abort();
    result?;
    Ok(())
}
