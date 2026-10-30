//! Uma instalação por vez; a consulta recebe um retrato completo de cada publicação.
use super::{commands, health, service::Service};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::Duration;

struct State {
    diary: Option<std::path::PathBuf>,
    operation: String,
    generation: u64,
    closed: bool,
    value: Value,
    workers: Vec<tokio::task::JoinHandle<()>>,
}
pub struct Installer {
    state: Mutex<State>,
    finished: tokio::sync::watch::Sender<bool>,
}
#[derive(Debug)]
pub enum StartError {
    Unsupported,
    Busy(String),
    Closed,
}
impl Default for Installer {
    fn default() -> Self {
        Self {
            state: Mutex::new(State {
                diary: None,
                operation: String::new(),
                generation: 0,
                closed: false,
                value: empty(),
                workers: vec![],
            }),
            finished: tokio::sync::watch::channel(false).0,
        }
    }
}
fn empty() -> Value {
    json!({"fase":"ocioso","harness":null,"etapa":null,"passo":0,"total":4,"log":[],"avisos":[],"ok":null,"erro":null})
}
impl Installer {
    pub fn status(&self) -> Value {
        let mut value = self.state.lock().unwrap().value.clone();
        value["comandos"] = commands::public(cfg!(windows));
        value["manual"] = commands::manual();
        value
    }
    pub fn start(self: &Arc<Self>, service: Service, cli: &str) -> Result<Value, StartError> {
        let argv = commands::command(cli, cfg!(windows)).ok_or(StartError::Unsupported)?;
        let mut state = self.state.lock().unwrap();
        if state.closed {
            return Err(StartError::Closed);
        }
        if state.value["fase"] == "rodando" {
            if state.value["harness"] != cli {
                return Err(StartError::Busy(
                    state.value["harness"].as_str().unwrap_or_default().into(),
                ));
            }
            drop(state);
            return Ok(self.status());
        }
        state.diary = crate::diag::diary_dir_in(&service.accounts.env);
        state.operation = crate::session_open::open::uuid4(&crate::mods::state::random_hex(16));
        state.generation += 1;
        let generation = state.generation;
        state.value = empty();
        state.value["fase"] = json!("rodando");
        state.value["harness"] = json!(cli);
        state.value["etapa"] = json!("comando");
        state.value["passo"] = json!(1);
        let owner = self.clone();
        let cli = cli.to_owned();
        let worker = tokio::spawn(async move {
            let run_owner = owner.clone();
            let task =
                tokio::spawn(
                    async move { run_owner.execute(generation, service, &cli, argv).await },
                );
            match task.await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => owner.fail(generation, None, &error),
                Err(error) => owner.fail(
                    generation,
                    None,
                    &format!("a instalação foi interrompida: {error}"),
                ),
            }
        });
        state.workers.retain(|worker| !worker.is_finished());
        state.workers.push(worker);
        let mut status = state.value.clone();
        status["comandos"] = commands::public(cfg!(windows));
        status["manual"] = commands::manual();
        drop(state);
        Ok(status)
    }
    pub async fn shutdown(self: &Arc<Self>) {
        let mut completion = self.finished.subscribe();
        {
            let mut state = self.state.lock().unwrap();
            if !state.closed {
                state.closed = true;
                let workers = std::mem::take(&mut state.workers);
                let owner = self.clone();
                tokio::spawn(async move {
                    for worker in workers {
                        let _ = worker.await;
                    }
                    owner.finished.send_replace(true);
                });
            }
        }
        while !*completion.borrow_and_update() {
            if completion.changed().await.is_err() {
                break;
            }
        }
    }
    fn publish(&self, generation: u64, fields: Value) {
        let mut state = self.state.lock().unwrap();
        if state.generation != generation {
            return;
        }
        for (key, value) in fields.as_object().unwrap() {
            state.value[key] = value.clone();
        }
        if ["fase", "etapa", "ok", "avisos"]
            .iter()
            .any(|key| fields.get(*key).is_some())
        {
            let failed = state.value["ok"] == false;
            crate::diag::record_blocking(
                state.diary.as_deref(),
                &[
                    json!({"evento":"harness.instalacao","nivel":if failed{"erro"}else{"ok"},"operacao":state.operation,"provider":state.value["harness"],"etapa":state.value["etapa"],"codigo":if failed{"falhou"}else{state.value["fase"].as_str().unwrap_or("ocioso")},"quantidade":state.value["avisos"].as_array().map_or(0,Vec::len)}),
                ],
                "instalação do harness",
            );
        }
    }
    fn step(&self, generation: u64, name: &str) {
        let step = ["comando", "conferir", "wrapper", "ajustes"]
            .iter()
            .position(|s| *s == name)
            .unwrap()
            + 1;
        self.publish(
            generation,
            json!({"fase":"rodando","etapa":name,"passo":step}),
        );
    }
    fn fail(&self, generation: u64, step: Option<&str>, message: &str) {
        let step = step.map(str::to_owned).unwrap_or_else(|| {
            self.state.lock().unwrap().value["etapa"]
                .as_str()
                .unwrap_or("comando")
                .into()
        });
        self.publish(
            generation,
            json!({"fase":"pronto","ok":false,"etapa":step,"erro":message}),
        );
    }
    fn log(&self, generation: u64, text: &str) {
        let mut state = self.state.lock().unwrap();
        if state.generation != generation {
            return;
        }
        let lines = state.value["log"].as_array_mut().unwrap();
        let parts = split_lines(text);
        if parts.is_empty() {
            lines.push(json!(text));
        } else {
            lines.extend(parts.into_iter().map(|s| json!(s)));
        }
        if lines.len() > 400 {
            lines.drain(..lines.len() - 400);
        }
    }
    async fn run(
        self: &Arc<Self>,
        generation: u64,
        service: &Service,
        args: Vec<String>,
    ) -> Result<i32, String> {
        let env = service.accounts.env.base.clone();
        let cwd = service.repo.clone();
        let owner = self.clone();
        tokio::task::spawn_blocking(move || {
            crate::update::process::run_with_env(
                &args,
                &cwd,
                Duration::from_secs(900),
                &env,
                &|text| owner.log(generation, text),
            )
        })
        .await
        .map_err(|e| e.to_string())?
        .map(|p| p.code)
        .map_err(|e| e.to_string())
    }
    async fn card(service: &Service, cli: &str) -> Result<Option<Value>, String> {
        Ok(service
            .diagnose()
            .await?
            .as_array()
            .and_then(|cards| cards.iter().find(|c| c["id"] == cli))
            .cloned())
    }
    async fn execute(
        self: &Arc<Self>,
        generation: u64,
        service: Service,
        cli: &str,
        argv: Vec<String>,
    ) -> Result<(), String> {
        let code = self
            .run(
                generation,
                &service,
                commands::resolve(argv, &service.accounts.env)?,
            )
            .await?;
        if code != 0 {
            self.fail(
                generation,
                Some("comando"),
                &format!("o instalador saiu com {code}"),
            );
            return Ok(());
        }
        self.step(generation, "conferir");
        service.probe.invalidate_cli(cli);
        let card = Self::card(&service, cli).await?;
        if !card.as_ref().is_some_and(|c| c["instalado"] == true) {
            self.fail(generation,Some("conferir"),&format!("o comando terminou, mas o CLI não aparece nem no PATH deste serviço ({}) nem na pasta de config dele",service.accounts.env.base.get("PATH").map(String::as_str).unwrap_or_default()));
            return Ok(());
        }
        self.step(generation, "wrapper");
        if cfg!(windows) {
            if health::wrapper(&service.accounts.env, cli, true)["ok"] != true {
                let result = service.repair("wrapper").await.map_err(|e| e.message)?;
                self.log(generation, &format!("$ wrapper\n{result}"));
                if health::wrapper(&service.accounts.env, cli, true)["ok"] == false {
                    self.fail(
                        generation,
                        Some("wrapper"),
                        "o perfil do PowerShell continua sem o wrapper",
                    );
                    return Ok(());
                }
            } else {
                self.log(
                    generation,
                    &format!("[wrapper] {cli} já é carregado pelo perfil do PowerShell"),
                );
            }
        } else {
            match service.wrapper_command() {
                Err(error) => {
                    self.log(generation, &format!("[wrapper pulado] {error}"));
                    self.publish(
                        generation,
                        json!({"avisos":[format!("a etapa do wrapper foi pulada: {error}")]}),
                    );
                }
                Ok(args) => {
                    let code = self.run(generation, &service, args).await?;
                    if code != 0 {
                        self.fail(
                            generation,
                            Some("wrapper"),
                            &format!("o instalador dos wrappers saiu com {code}"),
                        );
                        return Ok(());
                    }
                    let item = health::wrapper(&service.accounts.env, cli, false);
                    if item["ok"] == false {
                        self.fail(
                            generation,
                            Some("wrapper"),
                            &format!(
                                "o instalador rodou, mas o wrapper continua faltando em: {}",
                                item["params"]["lista"].as_str().unwrap_or("?")
                            ),
                        );
                        return Ok(());
                    }
                }
            }
        }
        self.step(generation, "ajustes");
        let card = Self::card(&service, cli).await?.or(card).unwrap();
        let pending = card["itens"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|item| item["conserto"].as_str())
            .collect::<Vec<_>>();
        for (index, id) in pending.iter().enumerate() {
            match service.repair(id).await {
                Ok(done) => self.log(generation, &format!("$ {id}\n{done}")),
                Err(error) => {
                    let remaining = &pending[index + 1..];
                    let suffix = if remaining.is_empty() {
                        String::new()
                    } else {
                        format!("; não cheguei a rodar: {}", remaining.join(", "))
                    };
                    self.fail(
                        generation,
                        Some("ajustes"),
                        &format!("{id}: {}{suffix}", error.message),
                    );
                    return Ok(());
                }
            }
        }
        self.publish(generation, json!({"fase":"pronto","ok":true,"erro":null}));
        Ok(())
    }
}
fn split_lines(text: &str) -> Vec<&str> {
    let mut output = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((index, ch)) = chars.next() {
        if matches!(
            ch,
            '\n' | '\r'
                | '\u{b}'
                | '\u{c}'
                | '\u{1c}'
                | '\u{1d}'
                | '\u{1e}'
                | '\u{85}'
                | '\u{2028}'
                | '\u{2029}'
        ) {
            output.push(&text[start..index]);
            start = index + ch.len_utf8();
            if ch == '\r' && chars.peek().is_some_and(|(_, c)| *c == '\n') {
                let (i, c) = chars.next().unwrap();
                start = i + c.len_utf8();
            }
        }
    }
    if start < text.len() {
        output.push(&text[start..]);
    }
    output
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    #[test]
    fn harness_functional_log_is_bounded_and_old_generation_cannot_publish() {
        let installer = Installer::default();
        installer.log(0, "a\r\nb\rc\u{85}d\u{2028}");
        assert_eq!(installer.status()["log"], json!(["a", "b", "c", "d"]));
        installer.log(1, "outra rodada");
        assert_eq!(installer.status()["log"].as_array().unwrap().len(), 4);
        for _ in 0..410 {
            installer.log(0, "linha");
        }
        assert_eq!(installer.status()["log"].as_array().unwrap().len(), 400);
    }
}
