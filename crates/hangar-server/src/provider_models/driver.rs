//! Sequências de teclado e confirmação; a posse pertence à operação inteira, inclusive ao redraw.
use super::{ModelError, pure};
use regex::Regex;
use serde_json::{Value, json};
use std::{future::Future, pin::Pin, sync::LazyLock, time::Duration};
use tokio::time::Instant;

pub type IoFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
/// Apenas a fronteira externa: pane, teclas e fonte publicada pelo agente.
pub trait Io: Send + Sync {
    fn capture(&self) -> IoFuture<'_, Result<String, ModelError>>;
    fn key<'a>(&'a self, key: &'a str, literal: bool) -> IoFuture<'a, Result<(), ModelError>>;
    fn editor(&self) -> IoFuture<'_, Option<String>>;
    fn catalog(&self) -> IoFuture<'_, Option<Value>>;
    fn wait(&self, duration: Duration) -> IoFuture<'_, ()> {
        Box::pin(tokio::time::sleep(duration))
    }
}
#[derive(Clone, Copy)]
pub struct Timing {
    pub settle: Duration,
    pub open: Duration,
    pub nav: Duration,
    pub slash: Duration,
    pub result: Duration,
    pub ready: Duration,
    pub read_back: Duration,
    pub read_interval: Duration,
}
impl Default for Timing {
    fn default() -> Self {
        Self {
            settle: Duration::from_millis(300),
            open: Duration::from_millis(700),
            nav: Duration::from_millis(120),
            slash: Duration::from_millis(300),
            result: Duration::from_secs(4),
            ready: Duration::from_secs(8),
            read_back: Duration::from_secs(2),
            read_interval: Duration::from_millis(150),
        }
    }
}

pub async fn prepare(io: &dyn Io, timing: Timing) -> Result<(), ModelError> {
    if let Ok(screen) = io.capture().await {
        let state = crate::terminal_state::analyze(&screen);
        if state.state == "awaiting_input" || state.overlay {
            return Err(ModelError::text(
                409,
                "pane com overlay aberto ou sessao morta — nada foi digitado",
            ));
        }
    }
    let until = Instant::now() + timing.ready;
    loop {
        let screen = io.capture().await?;
        let lines: Vec<_> = screen
            .trim_end_matches('\n')
            .split('\n')
            .rev()
            .take(12)
            .collect();
        if lines
            .iter()
            .any(|line| line.chars().any(|c| "─━═╰│".contains(c)))
        {
            return Ok(());
        }
        if Instant::now() >= until {
            tracing::warn!(
                code = "provider_input_ready_timeout",
                "TUI não confirmou prontidão no prazo; política anterior de envio conservada"
            );
            return Ok(());
        }
        io.wait(Duration::from_millis(200)).await;
    }
}
static NOTICE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"Subagentasyncgroupedresultintercomdeliverywasnotacknowledgedfor.*?/tmp/pi-subagents-[^']*'?\.?").unwrap()
});
pub fn occupied(screen: &str) -> bool {
    let mut lines: Vec<_> = screen.split('\n').collect();
    while lines.last().is_some_and(|s| s.trim().is_empty()) {
        lines.pop();
    }
    let rules: Vec<_> = lines
        .iter()
        .enumerate()
        .filter(|(_, s)| s.matches('─').count() >= 20)
        .map(|(i, _)| i)
        .collect();
    if rules.len() < 2 {
        return false;
    }
    let bottom = rules[rules.len() - 1];
    let top = rules[rules.len() - 2];
    if lines.len() - bottom > 8 || bottom - top > 15 {
        return false;
    }
    let content: String = lines[top + 1..bottom]
        .join("\n")
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    !NOTICE.replace_all(&content, "").is_empty()
}
pub async fn pi_commands(
    io: &dyn Io,
    commands: &[String],
    timing: Timing,
) -> Result<(), ModelError> {
    let busy = match io.editor().await {
        Some(editor) => !editor.trim().is_empty(),
        None => io.capture().await.map(|s| occupied(&s)).unwrap_or(false),
    };
    if busy {
        return Err(ModelError::text(
            409,
            "composer do pi ja tem texto — comando nao digitado (residuo/rascunho)",
        ));
    }
    for command in commands {
        io.key(command, true).await?;
        io.wait(timing.slash).await;
        io.key("Enter", false).await?;
        io.wait(timing.open).await;
    }
    Ok(())
}
pub async fn pi_read_back(
    io: &dyn Io,
    before: &Value,
    provider: Option<&str>,
    model: Option<&str>,
    effort: Option<&str>,
    timing: Timing,
) -> Result<Value, ModelError> {
    let until = Instant::now() + timing.read_back;
    let after = loop {
        let catalog = io.catalog().await;
        if catalog
            .as_ref()
            .is_some_and(|catalog| pure::confirms(catalog, provider, model, effort))
        {
            break catalog;
        }
        if Instant::now() >= until {
            break catalog;
        }
        io.wait(timing.read_interval).await;
    };
    if let Some(after) = after
        .as_ref()
        .filter(|a| pure::confirms(a, provider, model, effort))
    {
        return Ok(
            json!({"ok":true,"current":after.get("current"),"thinking":after.get("thinking"),"levels":after.get("levels").cloned().unwrap_or(json!([]))}),
        );
    }
    if after
        .as_ref()
        .is_none_or(|after| after.get("ts") == before.get("ts"))
    {
        return Err(ModelError::coded(
            409,
            "erro_sem_confirmacao_troca",
            "comandos digitados, mas o Pi nao republicou o catalogo — nao da pra confirmar a troca; veja o modelo no proprio terminal",
            json!({}),
        ));
    }
    let after = after.unwrap();
    let current = &after["current"];
    let provider = current.get("provider").cloned().unwrap_or(Value::Null);
    let id = current.get("id").cloned().unwrap_or(Value::Null);
    let thinking = after.get("thinking").cloned().unwrap_or(Value::Null);
    let show = |value: &Value| {
        value.as_str().map(str::to_owned).unwrap_or_else(|| {
            if value.is_null() {
                "None".into()
            } else {
                value.to_string()
            }
        })
    };
    Err(ModelError::coded(
        409,
        "erro_pi_recusou_troca",
        &format!(
            "o Pi recusou a troca — segue em {}/{} (raciocinio {}). Causa mais comum: sem chave configurada pro provedor pedido (o Pi avisa dentro do TUI)",
            show(&provider),
            show(&id),
            show(&thinking)
        ),
        json!({"provider":provider,"id":id,"thinking":thinking}),
    ))
}
pub async fn kimi_switch(
    io: &dyn Io,
    alias: Option<&str>,
    display: Option<&str>,
    effort: Option<&str>,
    timing: Timing,
) -> Result<Value, ModelError> {
    let first = io.capture().await?;
    let before_model = pure::parse_switched(&first);
    let before_effort = pure::parse_thinking_set(&first);
    io.key("/model", true).await?;
    io.wait(timing.slash).await;
    io.key("Enter", false).await?;
    let until = Instant::now() + timing.result;
    let mut opened = false;
    while Instant::now() < until {
        io.wait(timing.settle).await;
        if io.capture().await?.contains("Select a model") {
            opened = true;
            break;
        }
    }
    if !opened {
        io.key("Escape", false).await?;
        return Err(ModelError::text(
            409,
            "o picker do /model do Kimi nao abriu",
        ));
    }
    if let Some(alias) = alias {
        io.key(alias, true).await?;
        io.wait(timing.open).await;
    }
    if let Some(effort) = effort {
        let row = pure::parse_thinking_row(&io.capture().await?);
        let Some(row) = row else {
            io.key("Escape", false).await?;
            return Err(ModelError::text(
                409,
                "este modelo nao tem niveis de pensamento no picker",
            ));
        };
        let levels: Vec<_> = row["levels"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_lowercase)
            .collect();
        let Some(want) = levels.iter().position(|s| s == effort) else {
            io.key("Escape", false).await?;
            return Err(ModelError::text(
                409,
                format!("nivel {effort} nao aparece no picker deste modelo"),
            ));
        };
        let current = row["current"].as_str().unwrap_or("").to_lowercase();
        let position = levels
            .iter()
            .position(|s| s == &current)
            .ok_or_else(|| ModelError::text(409, "nível atual ilegível no picker"))?;
        for _ in 0..want.abs_diff(position) {
            io.key(if want > position { "Right" } else { "Left" }, false)
                .await?;
            io.wait(timing.nav).await;
        }
        io.wait(timing.settle).await;
    }
    io.key("M-s", false).await?;
    let until = Instant::now() + timing.result;
    loop {
        io.wait(timing.settle).await;
        let pane = io.capture().await?;
        let switched = pure::parse_switched(&pane);
        let thinking = pure::parse_thinking_set(&pane);
        let new_model = switched
            .as_ref()
            .filter(|v| Some(*v) != before_model.as_ref());
        let new_effort = thinking
            .as_ref()
            .filter(|v| Some(*v) != before_effort.as_ref());
        let model_ok = alias.is_none() || new_model.is_some_and(|v| v["name"].as_str() == display);
        let effort_ok = effort.is_none()
            || new_effort.is_some_and(|v| v["level"].as_str() == effort)
            || new_model.is_some_and(|v| v["level"].as_str() == effort);
        if model_ok && effort_ok {
            return Ok(
                json!({"ok":true,"result":switched.as_ref().or(thinking.as_ref()).and_then(|v|v.get("raw"))}),
            );
        }
        if let Some(new) = new_model.or(new_effort) {
            io.key("Escape", false).await?;
            return Err(ModelError::text(
                409,
                format!(
                    "o Kimi aplicou outra coisa ({})",
                    new["raw"].as_str().unwrap_or("None")
                ),
            ));
        }
        if Instant::now() >= until {
            io.key("Escape", false).await?;
            return Err(ModelError::text(
                409,
                "sem confirmacao da troca no terminal",
            ));
        }
    }
}
