//! Uma rodada de sonho: analisadores em paralelo, um organizador, e a conferência antes de qualquer
//! máquina ser tocada. Quem grava a memória é quem chama, com o resultado conferido daqui.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Mutex;

use futures_util::stream::{self, StreamExt};

use crate::budget::{analyzer_call_cap, organizer_call_cap, MAX_PARALLEL};
use crate::memory::INDEX;
use crate::state::Suggestion;

pub const ANALYZER_MODEL: &str = "claude-haiku-5-5";
pub const ORGANIZER_MODEL: &str = "claude-sonnet-5-5";
pub const INDEX_MAX_LINES: usize = 200;
pub const INDEX_MAX_BYTES: usize = 25 * 1024;
pub const USER_MEMORY_FILE: &str = "memoria-do-usuario.md";
pub const USER_MEMORY_MAX_LINES: usize = 100;
/// Quanto o organizador pode deixar a pasta crescer além do que havia, salvo assunto novo.
pub const GROWTH: f64 = 1.3;

const ANALYZER: &str = include_str!("prompts/analyzer.md");
const ORGANIZER: &str = include_str!("prompts/organizer.md");
const USER_MEMORY: &str = include_str!("prompts/user_memory.md");

pub type BoxFut<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Clone, Debug, PartialEq)]
pub enum Auth {
    Key,
    Account(PathBuf),
}

pub struct Call<'a> {
    pub model: &'static str,
    pub prompt: String,
    pub workdir: &'a Path,
    pub budget_usd: f64,
    pub auth: Auth,
}

pub struct Reply {
    pub cost_usd: f64,
    pub text: String,
}

#[derive(Debug, PartialEq)]
pub enum ModelError {
    /// A chave respondeu "credit balance too low".
    NoCredit,
    /// A chamada bateu no teto que recebeu; o gasto até ali conta.
    Budget { cost_usd: f64 },
    /// Limite de uso da conta.
    Limited,
    Other(String),
}

pub trait Model: Send + Sync {
    fn call<'a>(&'a self, call: Call<'a>) -> BoxFut<'a, Result<Reply, ModelError>>;
}

#[derive(Debug)]
pub enum DreamError {
    Model(ModelError),
    /// O resultado não passou na conferência: nenhuma memória foi tocada.
    Rejected(String),
    Io(io::Error),
}

impl fmt::Display for DreamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Model(ModelError::NoCredit) => write!(f, "dream_no_credit"),
            Self::Model(ModelError::Budget { .. }) => write!(f, "dream_budget_exhausted"),
            Self::Model(ModelError::Limited) => write!(f, "dream_account_limited"),
            Self::Model(ModelError::Other(e)) => write!(f, "dream_model_failed: {e}"),
            Self::Rejected(why) => write!(f, "dream_rejected: {why}"),
            Self::Io(e) => write!(f, "dream_io: {e}"),
        }
    }
}

impl From<io::Error> for DreamError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

/// Arquivo de `inputs/`: uma parte de transcrição ou as notas de outra máquina.
pub struct Input {
    pub name: String,
    pub text: String,
}

/// Quem paga e quanto já foi gasto. A troca da chave para a conta acontece uma vez por rodada.
pub struct Payer<'a> {
    ledger: Mutex<Ledger>,
    on_no_credit: &'a (dyn Fn() -> Option<Auth> + Sync),
}

struct Ledger {
    auth: Auth,
    key_usd: f64,
    account_usd: f64,
}

impl Ledger {
    fn add(&mut self, auth: &Auth, cost: f64) {
        match auth {
            Auth::Key => self.key_usd += cost,
            Auth::Account(_) => self.account_usd += cost,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Spent {
    pub key_usd: f64,
    pub account_usd: f64,
    pub auth: Auth,
}

impl<'a> Payer<'a> {
    pub fn new(auth: Auth, on_no_credit: &'a (dyn Fn() -> Option<Auth> + Sync)) -> Self {
        Self { ledger: Mutex::new(Ledger { auth, key_usd: 0.0, account_usd: 0.0 }), on_no_credit }
    }

    pub fn spent(&self) -> Spent {
        let l = self.ledger.lock().unwrap();
        Spent { key_usd: l.key_usd, account_usd: l.account_usd, auth: l.auth.clone() }
    }

    async fn call(
        &self,
        model: &dyn Model,
        name: &'static str,
        prompt: &str,
        workdir: &Path,
        budget: impl Fn(f64) -> f64,
    ) -> Result<Reply, DreamError> {
        let mut switched = false;
        loop {
            let (auth, budget_usd) = {
                let l = self.ledger.lock().unwrap();
                (l.auth.clone(), budget(l.key_usd + l.account_usd))
            };
            if budget_usd <= 0.0 {
                return Err(DreamError::Model(ModelError::Budget { cost_usd: 0.0 }));
            }
            let call = Call { model: name, prompt: prompt.to_string(), workdir, budget_usd, auth: auth.clone() };
            match model.call(call).await {
                Ok(reply) => {
                    self.ledger.lock().unwrap().add(&auth, reply.cost_usd);
                    return Ok(reply);
                }
                Err(ModelError::NoCredit) if auth == Auth::Key && !switched => {
                    switched = true;
                    let mut l = self.ledger.lock().unwrap();
                    // Outra chamada pode já ter trocado; só a primeira pergunta pela conta.
                    if l.auth == Auth::Key {
                        l.auth = (self.on_no_credit)().ok_or(DreamError::Model(ModelError::NoCredit))?;
                    }
                }
                Err(ModelError::Budget { cost_usd }) => {
                    self.ledger.lock().unwrap().add(&auth, cost_usd);
                    return Err(DreamError::Model(ModelError::Budget { cost_usd }));
                }
                Err(e) => return Err(DreamError::Model(e)),
            }
        }
    }
}

pub struct Job<'a> {
    /// Pasta vazia criada por quem chama; é apagada por ele depois.
    pub workdir: &'a Path,
    pub base: &'a BTreeMap<String, String>,
    pub inputs: &'a [Input],
    pub cap_usd: f64,
    pub scrub: &'a (dyn Fn(&str) -> String + Sync),
}

#[derive(Debug)]
pub struct Outcome {
    pub files: BTreeMap<String, String>,
    pub retired: usize,
    pub organizer_reply: String,
}

pub async fn dream_project(job: &Job<'_>, payer: &Payer<'_>, model: &dyn Model) -> Result<Outcome, DreamError> {
    let mem = job.workdir.join("memory");
    let inputs = job.workdir.join("inputs");
    fs::create_dir_all(&mem)?;
    fs::create_dir_all(&inputs)?;
    for (name, content) in job.base {
        // A base pode vir de outra máquina: nome que sairia da pasta recusa a rodada.
        if !crate::memory::valid_name(name) {
            return Err(DreamError::Rejected(format!("nome inválido na base: {name}")));
        }
        fs::write(mem.join(name), content)?;
    }
    for input in job.inputs {
        fs::write(inputs.join(&input.name), &input.text)?;
    }
    let cap = job.cap_usd;
    let results: Vec<Result<Reply, DreamError>> = stream::iter(job.inputs)
        .map(|input| {
            let prompt = ANALYZER.replace("{input}", &format!("inputs/{}", input.name));
            async move { payer.call(model, ANALYZER_MODEL, &prompt, job.workdir, |spent| analyzer_call_cap(cap, spent)).await }
        })
        .buffer_unordered(MAX_PARALLEL)
        .collect()
        .await;
    if let Some(err) = results.into_iter().find_map(Result::err) {
        return Err(err);
    }
    let before = job.base.keys().filter(|n| n.as_str() != INDEX).count();
    let prompt = ORGANIZER
        .replace("{notes_before}", &before.to_string())
        .replace("{notes_cap}", &((before.max(5) as f64 * GROWTH).ceil() as usize).to_string());
    let reply = payer.call(model, ORGANIZER_MODEL, &prompt, job.workdir, |spent| organizer_call_cap(cap, spent)).await?;
    let (files, retired) = collect(job.workdir, job.scrub)?;
    Ok(Outcome { files, retired, organizer_reply: reply.text })
}

/// Confere o que os agentes deixaram. Qualquer coisa fora do formato recusa a rodada inteira.
fn collect(workdir: &Path, scrub: &dyn Fn(&str) -> String) -> Result<(BTreeMap<String, String>, usize), DreamError> {
    for entry in fs::read_dir(workdir)? {
        let name = entry?.file_name().to_string_lossy().into_owned();
        if name != "memory" && name != "inputs" {
            return Err(DreamError::Rejected(format!("arquivo fora de memory/: {name}")));
        }
    }
    let mem = workdir.join("memory");
    let mut files = BTreeMap::new();
    let mut retired = 0;
    for entry in fs::read_dir(&mem)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !entry.file_type()?.is_file() || !crate::memory::valid_name(&name) {
            return Err(DreamError::Rejected(format!("fora do formato em memory/: {name}")));
        }
        let content = fs::read_to_string(entry.path())?;
        let trimmed = content.trim();
        if trimmed.is_empty() || (trimmed.starts_with("MOVED TO") && !trimmed.contains('\n')) {
            retired += 1;
            continue;
        }
        if scrub(&content) != content {
            return Err(DreamError::Rejected(format!("segredo em {name}")));
        }
        files.insert(name, content);
    }
    let index = files.get(INDEX).ok_or_else(|| DreamError::Rejected("MEMORY.md ausente".into()))?;
    if index.lines().count() > INDEX_MAX_LINES || index.len() > INDEX_MAX_BYTES {
        return Err(DreamError::Rejected(format!("MEMORY.md com {} linhas e {} bytes", index.lines().count(), index.len())));
    }
    Ok((files, retired))
}

pub struct UserJob<'a> {
    pub workdir: &'a Path,
    /// `(projeto, conteúdo)` de cada nota `user`/`feedback`.
    pub notes: &'a [(String, String)],
    pub claude_md: &'a str,
    pub current: &'a str,
    pub cap_usd: f64,
    pub scrub: &'a (dyn Fn(&str) -> String + Sync),
}

#[derive(Debug, PartialEq)]
pub struct UserOutcome {
    pub content: String,
    pub suggestions: Vec<Suggestion>,
}

pub async fn dream_user(job: &UserJob<'_>, payer: &Payer<'_>, model: &dyn Model) -> Result<UserOutcome, DreamError> {
    let inputs = job.workdir.join("inputs");
    fs::create_dir_all(&inputs)?;
    for (n, (project, content)) in job.notes.iter().enumerate() {
        fs::write(inputs.join(format!("notes-{n}.md")), format!("PROJECT {project}\n\n{content}"))?;
    }
    fs::write(inputs.join("CLAUDE.md"), job.claude_md)?;
    fs::write(job.workdir.join(USER_MEMORY_FILE), job.current)?;
    let cap = job.cap_usd;
    payer.call(model, ORGANIZER_MODEL, USER_MEMORY, job.workdir, |spent| organizer_call_cap(cap, spent)).await?;
    let content = fs::read_to_string(job.workdir.join(USER_MEMORY_FILE))?;
    if content.lines().count() > USER_MEMORY_MAX_LINES {
        return Err(DreamError::Rejected(format!("{USER_MEMORY_FILE} com {} linhas", content.lines().count())));
    }
    if (job.scrub)(&content) != content {
        return Err(DreamError::Rejected(format!("segredo em {USER_MEMORY_FILE}")));
    }
    let suggestions = match fs::read_to_string(job.workdir.join("suggestions.json")) {
        Ok(raw) => serde_json::from_str(&raw).map_err(|e| DreamError::Rejected(format!("suggestions.json: {e}")))?,
        Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(e.into()),
    };
    Ok(UserOutcome { content, suggestions })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    type Script = dyn Fn(&Call, usize) -> Result<Reply, ModelError> + Send + Sync;

    struct Fake {
        calls: AtomicUsize,
        script: Box<Script>,
    }

    impl Model for Fake {
        fn call<'a>(&'a self, call: Call<'a>) -> BoxFut<'a, Result<Reply, ModelError>> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            let out = (self.script)(&call, n);
            Box::pin(async move { out })
        }
    }

    fn fake(script: impl Fn(&Call, usize) -> Result<Reply, ModelError> + Send + Sync + 'static) -> Fake {
        Fake { calls: AtomicUsize::new(0), script: Box::new(script) }
    }

    fn ok(cost: f64) -> Result<Reply, ModelError> {
        Ok(Reply { cost_usd: cost, text: "{}".into() })
    }

    /// Analisador grava uma nota por entrada; organizador refaz o índice com o que existir.
    fn agents(call: &Call) {
        let mem = call.workdir.join("memory");
        if call.model == ANALYZER_MODEL {
            let input = call.prompt.split("inputs/").nth(1).unwrap().split('`').next().unwrap().replace(".txt", "");
            fs::write(mem.join(format!("{input}.md")), format!("---\nname: {input}\n---\nfato de {input}\n")).unwrap();
        } else {
            let mut names: Vec<String> = fs::read_dir(&mem)
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .filter(|n| n != INDEX)
                .collect();
            names.sort();
            let index: String = names.iter().map(|n| format!("- [{n}]({n}) — nota\n")).collect();
            fs::write(mem.join(INDEX), index).unwrap();
        }
    }

    fn redact(text: &str) -> String {
        text.replace("sk-ant-", "[chave]")
    }

    fn inputs() -> Vec<Input> {
        vec![Input { name: "s1.txt".into(), text: "USER:\na".into() }, Input { name: "s2.txt".into(), text: "USER:\nb".into() }]
    }

    fn base() -> BTreeMap<String, String> {
        BTreeMap::from([("MEMORY.md".to_string(), "- [velha](velha.md) — x\n".to_string()), ("velha.md".to_string(), "fato velho".to_string())])
    }

    #[tokio::test]
    async fn normal_round_adds_notes_and_sums_cost() {
        let tmp = tempfile::tempdir().unwrap();
        let model = fake(|call, _| {
            agents(call);
            ok(0.1)
        });
        let none = || None;
        let payer = Payer::new(Auth::Key, &none);
        let (base, inputs) = (base(), inputs());
        let job = Job { workdir: tmp.path(), base: &base, inputs: &inputs, cap_usd: 15.0, scrub: &redact };
        let out = dream_project(&job, &payer, &model).await.unwrap();
        assert_eq!(out.files.keys().collect::<Vec<_>>(), ["MEMORY.md", "s1.md", "s2.md", "velha.md"]);
        let spent = payer.spent();
        assert!((spent.key_usd - 0.3).abs() < 1e-9);
        assert_eq!(spent.account_usd, 0.0);
    }

    #[tokio::test]
    async fn no_credit_moves_the_rest_of_the_round_to_the_account_once() {
        let tmp = tempfile::tempdir().unwrap();
        let model = fake(|call, _| {
            if call.auth == Auth::Key {
                return Err(ModelError::NoCredit);
            }
            agents(call);
            ok(0.1)
        });
        let asked = AtomicUsize::new(0);
        let fallback = || {
            asked.fetch_add(1, Ordering::SeqCst);
            Some(Auth::Account("/conta".into()))
        };
        let payer = Payer::new(Auth::Key, &fallback);
        let (base, inputs) = (base(), inputs());
        let job = Job { workdir: tmp.path(), base: &base, inputs: &inputs, cap_usd: 15.0, scrub: &redact };
        dream_project(&job, &payer, &model).await.unwrap();
        assert_eq!(asked.load(Ordering::SeqCst), 1);
        let spent = payer.spent();
        assert_eq!(spent.auth, Auth::Account("/conta".into()));
        assert!((spent.account_usd - 0.3).abs() < 1e-9);
    }

    #[tokio::test]
    async fn no_credit_without_an_account_fails_the_round() {
        let tmp = tempfile::tempdir().unwrap();
        let model = fake(|_, _| Err(ModelError::NoCredit));
        let none = || None;
        let payer = Payer::new(Auth::Key, &none);
        let (base, inputs) = (base(), inputs());
        let job = Job { workdir: tmp.path(), base: &base, inputs: &inputs, cap_usd: 15.0, scrub: &redact };
        assert!(matches!(dream_project(&job, &payer, &model).await, Err(DreamError::Model(ModelError::NoCredit))));
    }

    async fn rejected_by(organizer: impl Fn(&Path) + Send + Sync + 'static) -> String {
        let tmp = tempfile::tempdir().unwrap();
        let model = fake(move |call, _| {
            agents(call);
            if call.model == ORGANIZER_MODEL {
                organizer(call.workdir);
            }
            ok(0.1)
        });
        let none = || None;
        let payer = Payer::new(Auth::Key, &none);
        let (base, inputs) = (base(), inputs());
        let job = Job { workdir: tmp.path(), base: &base, inputs: &inputs, cap_usd: 15.0, scrub: &redact };
        match dream_project(&job, &payer, &model).await {
            Err(DreamError::Rejected(why)) => why,
            other => panic!("esperava recusa, veio {other:?}"),
        }
    }

    #[tokio::test]
    async fn oversized_index_is_rejected() {
        let why = rejected_by(|w| fs::write(w.join("memory").join(INDEX), "- [x](x.md) — y\n".repeat(300)).unwrap()).await;
        assert!(why.contains("MEMORY.md"), "{why}");
    }

    #[tokio::test]
    async fn secret_in_a_note_is_rejected() {
        let why = rejected_by(|w| fs::write(w.join("memory").join("deploy.md"), "token sk-ant-abc").unwrap()).await;
        assert!(why.contains("segredo"), "{why}");
    }

    #[tokio::test]
    async fn file_outside_memory_or_in_a_subfolder_is_rejected() {
        let why = rejected_by(|w| fs::write(w.join("solto.md"), "x").unwrap()).await;
        assert!(why.contains("fora de memory/"), "{why}");
        let why = rejected_by(|w| fs::create_dir(w.join("memory").join("grupo")).unwrap()).await;
        assert!(why.contains("formato"), "{why}");
    }

    #[tokio::test]
    async fn retired_notes_are_dropped() {
        let tmp = tempfile::tempdir().unwrap();
        let model = fake(|call, _| {
            agents(call);
            if call.model == ORGANIZER_MODEL {
                fs::write(call.workdir.join("memory").join("velha.md"), "MOVED TO [[s1]]\n").unwrap();
            }
            ok(0.1)
        });
        let none = || None;
        let payer = Payer::new(Auth::Key, &none);
        let (base, inputs) = (base(), inputs());
        let job = Job { workdir: tmp.path(), base: &base, inputs: &inputs, cap_usd: 15.0, scrub: &redact };
        let out = dream_project(&job, &payer, &model).await.unwrap();
        assert!(!out.files.contains_key("velha.md"));
        assert_eq!(out.retired, 1);
    }

    #[tokio::test]
    async fn user_pass_keeps_the_file_short_and_returns_suggestions() {
        let tmp = tempfile::tempdir().unwrap();
        let model = fake(|call, _| {
            assert_eq!(call.model, ORGANIZER_MODEL);
            fs::write(call.workdir.join(USER_MEMORY_FILE), "## Comunicação\n- Responder em pt-BR.\n").unwrap();
            fs::write(call.workdir.join("suggestions.json"), r#"[{"text":"usar inglês","conflicts_with":"todo texto sai em pt-BR"}]"#).unwrap();
            ok(0.2)
        });
        let none = || None;
        let payer = Payer::new(Auth::Key, &none);
        let notes = vec![("git:a/b/c".to_string(), "prefere pt-BR".to_string())];
        let job = UserJob { workdir: tmp.path(), notes: &notes, claude_md: "todo texto sai em pt-BR", current: "", cap_usd: 5.0, scrub: &redact };
        let out = dream_user(&job, &payer, &model).await.unwrap();
        assert!(out.content.contains("pt-BR"));
        assert_eq!(out.suggestions.len(), 1);
        assert!(tmp.path().join("inputs").join("notes-0.md").exists());
    }
}
