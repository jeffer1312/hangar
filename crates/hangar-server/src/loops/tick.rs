//! Uma volta do loop no fim de turno: âncora do objetivo, chave mestra, trava de branch, comando de
//! conferência (fora da trava) e o desfecho — pronto, falha, estagnado, esgotado ou re-prompt.
use super::store::{int, lock, status, truthy, Loop, Store, HISTORY_MAX, NOTIFY};
use futures_util::future::BoxFuture;
use serde_json::{json, Value};
use std::io;

/// O que a volta precisa do mundo; o teste troca tudo.
pub trait Env: Send + Sync {
    fn now(&self) -> f64;
    fn automations(&self) -> bool;
    /// Branch da worktree onde o agente trabalha.
    fn branch(&self) -> Option<String>;
    /// A entrada do objetivo na fila: entregue, esperando, ou `None` se sumiu.
    fn entry_delivered(&self, entry_id: Option<String>) -> BoxFuture<'_, Option<bool>>;
    fn run_check(&self, cmd: String) -> BoxFuture<'_, (i64, String)>;
    fn last_assistant(&self) -> BoxFuture<'_, Option<String>>;
    /// Põe o texto na fila durável da sessão e drena; `Err` é o código da falha.
    fn enqueue(&self, text: String) -> BoxFuture<'_, Result<(), String>>;
    fn notify(&self, status: &str, reason: &str);
}

/// O corpo do aviso em português (`loop._body`); o push traduz pela inscrição.
pub fn body(status: &str, reason: Option<&str>) -> String {
    let r = reason.unwrap_or("None");
    match status {
        "done" => format!("loop concluído: {r}"),
        "done_claimed" => "Claude declarou pronto — confirmar?".into(),
        "stopped" => format!("loop parado: {r}"),
        "exhausted" => format!("loop esgotou as iterações: {r}"),
        "failed" => format!("loop falhou: {r}"),
        other => reason.filter(|r| !r.is_empty()).unwrap_or(other).to_owned(),
    }
}

/// Estado de atenção ou fim: grava e avisa (só os de `NOTIFY`). Quem chama segura a trava.
pub fn end(store: &Store, name: &str, status: &str, reason: &str, env: &dyn Env) -> io::Result<Option<Loop>> {
    let mut fields = Loop::new();
    fields.insert("status".into(), json!(status));
    fields.insert("ended_reason".into(), json!(reason));
    // `done_claimed` espera a confirmação: ainda não acabou.
    if status != "done_claimed" { fields.insert("ended_ts".into(), json!(env.now())); }
    let d = store.update(name, fields)?;
    if NOTIFY.contains(&status) { env.notify(status, reason); }
    Ok(d)
}

pub fn reprompt_text(d: &Loop, tail: &str, check_exit: Option<i64>) -> String {
    let goal = d.get("goal").and_then(Value::as_str).unwrap_or("");
    let middle = if truthy(d.get("check_cmd")) {
        format!("O comando de verificação falhou (exit {}). Saída (cauda):\n{tail}\n", check_exit.map_or("None".into(), |c| c.to_string()))
    } else {
        "A tarefa ainda não foi confirmada como concluída. Continue.\n".into()
    };
    format!("[loop {}/{}] Objetivo: {goal}\n{middle}Não edite arquivos de teste nem o comando de verificação; corrija o código.",
        int(d, "iter"), int(d, "max_iters"))
}

/// Conta a volta no histórico (últimas 20), grava e entrega o re-prompt. Quem chama segura a
/// trava e já conferiu que cabe mais uma volta. Re-prompt que não entra na fila falha o loop.
pub async fn reprompt(store: &Store, name: &str, mut d: Loop, tail: &str, check_exit: Option<i64>, env: &dyn Env) -> io::Result<Option<Loop>> {
    let n = int(&d, "iter") + 1;
    let mut history = d.get("history").and_then(Value::as_array).cloned().unwrap_or_default();
    history.push(json!({"n": n, "ts": env.now(), "check_exit": check_exit, "tail": tail}));
    let skip = history.len().saturating_sub(HISTORY_MAX);
    d.insert("history".into(), Value::Array(history.split_off(skip)));
    d.insert("iter".into(), json!(n));
    store.set(name, &d)?;
    if let Err(code) = env.enqueue(reprompt_text(&d, tail, check_exit)).await {
        return end(store, name, "failed", &format!("re-prompt não entrou na fila ({code})"), env);
    }
    Ok(store.get(name))
}

fn exhausted(d: &Loop) -> Option<String> {
    let max = int(d, "max_iters");
    (int(d, "iter") + 1 > max).then(|| format!("esgotou {max} iterações"))
}

fn ticking(d: &Loop) -> bool { matches!(status(d), "running" | "paused_awaiting") }

/// `Some` com o sidecar depois da volta; `None` quando não havia o que fazer.
pub async fn run_tick(store: &Store, name: &str, env: &dyn Env) -> io::Result<Option<Loop>> {
    let check_cmd = {
        let _one = lock(name).await;
        let Some(mut d) = store.get(name).filter(ticking) else { return Ok(None) };
        // A pausa da pergunta acaba no fim de turno, calada.
        if status(&d) == "paused_awaiting" {
            d.insert("status".into(), json!("running"));
            store.set(name, &d)?;
        }
        // Só conta volta depois de o objetivo ter sido digitado.
        if d.get("goal_delivered_ts").is_none_or(Value::is_null) {
            let entry = d.get("goal_entry_id").and_then(Value::as_str).map(str::to_owned);
            if env.entry_delivered(entry).await == Some(false) { return Ok(None); }
            d.insert("goal_delivered_ts".into(), json!(env.now()));
            store.set(name, &d)?;
        }
        if !env.automations() { return end(store, name, "stopped", "automations_disabled", env); }
        if let Some(br) = env.branch().filter(|b| b == "main" || b == "master") && truthy(d.get("require_branch")) {
            return end(store, name, "stopped", &format!("branch {br}"), env);
        }
        d.get("check_cmd").and_then(Value::as_str).filter(|c| !c.is_empty()).map(str::to_owned)
    };
    let (outcome, text) = match &check_cmd {
        Some(cmd) => (Some(env.run_check(cmd.clone()).await), None),
        None => (None, env.last_assistant().await),
    };
    let _one = lock(name).await;
    // Parado ou resolvido durante a conferência: não sobrescreve.
    let Some(d) = store.get(name).filter(ticking) else { return Ok(None) };
    if let Some((code, tail)) = outcome {
        if code == 0 { return end(store, name, "done", "check passou", env); }
        if code == super::check::UNRUNNABLE { return end(store, name, "failed", &format!("check_cmd não executável: {tail}"), env); }
        let last = d.get("history").and_then(Value::as_array).and_then(|h| h.last()).and_then(|e| e.get("tail"));
        if last == Some(&Value::String(tail.clone())) { return end(store, name, "stopped", "estagnado — mesmo erro 2x", env); }
        if let Some(reason) = exhausted(&d) { return end(store, name, "exhausted", &reason, env); }
        return reprompt(store, name, d, &tail, Some(code), env).await;
    }
    if text.as_deref().unwrap_or("").contains("LOOP_DONE") {
        return end(store, name, "done_claimed", "declarou pronto — confirmar?", env);
    }
    if let Some(reason) = exhausted(&d) { return end(store, name, "exhausted", &reason, env); }
    reprompt(store, name, d, "(sem check — iteração de continuação)", None, env).await
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    pub(crate) struct Fake {
        pub now: f64,
        pub automations: bool,
        pub branch: Option<String>,
        pub delivered: Option<bool>,
        pub check: (i64, String),
        pub last: Option<String>,
        pub enqueue_fails: bool,
        pub sent: Mutex<Vec<String>>,
        pub pushed: Mutex<Vec<(String, String)>>,
    }

    impl Env for Fake {
        fn now(&self) -> f64 { self.now }
        fn automations(&self) -> bool { self.automations }
        fn branch(&self) -> Option<String> { self.branch.clone() }
        fn entry_delivered(&self, _id: Option<String>) -> BoxFuture<'_, Option<bool>> { Box::pin(async move { self.delivered }) }
        fn run_check(&self, _cmd: String) -> BoxFuture<'_, (i64, String)> { Box::pin(async move { self.check.clone() }) }
        fn last_assistant(&self) -> BoxFuture<'_, Option<String>> { Box::pin(async move { self.last.clone() }) }
        fn enqueue(&self, text: String) -> BoxFuture<'_, Result<(), String>> {
            Box::pin(async move {
                if self.enqueue_fails { return Err("runtime_binding".into()); }
                self.sent.lock().unwrap().push(text);
                Ok(())
            })
        }
        fn notify(&self, status: &str, reason: &str) {
            self.pushed.lock().unwrap().push((status.into(), body(status, Some(reason))));
        }
    }

    fn fake(ctx: &Value, now: f64) -> Fake {
        let check = ctx.get("check").and_then(Value::as_array).map_or((1, "err".into()),
            |c| (c[0].as_i64().unwrap(), c[1].as_str().unwrap().to_owned()));
        Fake {
            now, check,
            automations: ctx.get("automations").and_then(Value::as_bool).unwrap_or(true),
            branch: match ctx.get("branch") { None => Some("TICKET-1".into()), Some(b) => b.as_str().map(str::to_owned) },
            delivered: match ctx.get("entry_delivered") { None => Some(true), Some(v) => v.as_bool() },
            last: ctx.get("last").and_then(Value::as_str).map(str::to_owned),
            ..Fake::default()
        }
    }

    #[tokio::test]
    async fn ticks_match_the_python_run_tick() {
        let g = super::super::tests::golden();
        let now = g["now"].as_f64().unwrap();
        for c in g["ticks"].as_array().unwrap() {
            let dir = tempfile::tempdir().unwrap();
            let store = Store::new(dir.path());
            store.set("s", c["loop"].as_object().unwrap()).unwrap();
            let env = fake(&c["ctx"], now);
            let returned = run_tick(&store, "s", &env).await.unwrap();
            let out = &c["out"];
            assert_eq!(Value::Object(store.get("s").unwrap()), out["loop"], "{}", c["label"]);
            assert_eq!(returned.is_some(), out["returned"].as_bool().unwrap(), "{}", c["label"]);
            assert_eq!(json!(*env.sent.lock().unwrap()), out["sent"], "{}", c["label"]);
            let pushed: Vec<Value> = env.pushed.lock().unwrap().iter().map(|(_, b)| json!(["s", b])).collect();
            assert_eq!(json!(pushed), out["pushed"], "{}", c["label"]);
        }
    }

    #[test]
    fn bodies_match_the_python() {
        for c in super::super::tests::golden()["bodies"].as_array().unwrap() {
            assert_eq!(body(c["status"].as_str().unwrap(), c["reason"].as_str()), c["body"].as_str().unwrap(), "{c}");
        }
    }

    /// Re-prompt que não entra na fila: o loop falha e avisa, em vez de ficar rodando calado.
    #[tokio::test]
    async fn reprompt_that_cannot_queue_fails_the_loop() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        let mut d = crate::loops::store::new_loop("g", Some("false"), 10, true, 1.0);
        d.insert("goal_delivered_ts".into(), json!(1.0));
        store.set("s", &d).unwrap();
        let env = Fake { automations: true, check: (1, "e".into()), enqueue_fails: true, ..Fake::default() };
        let after = run_tick(&store, "s", &env).await.unwrap().unwrap();
        assert_eq!(status(&after), "failed");
        assert_eq!(after["ended_reason"], "re-prompt não entrou na fila (runtime_binding)");
        assert_eq!(env.pushed.lock().unwrap().len(), 1);
    }
}
