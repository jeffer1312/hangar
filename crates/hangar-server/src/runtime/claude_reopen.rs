//! Reabrir o Claude pelo spec: `reopen_session(nome, SessionChange)` para troca de modo (terminal ⇄ sem
//! terminal), recarregar, bypass e troca de conta. A ordem é a da spec: monta a vida nova (comando,
//! ambiente, conta) antes de tocar em processo; encerra a antiga e confirma que ela saiu, a árvore inteira;
//! dá a partida na nova e confirma que ela está viva na conversa e no modo pedidos; grava o spec.
//! Falha antes de encerrar não muda nada; falha depois volta a sessão pelo spec antigo e o erro sai com
//! código. O modo vem do spec e do que o Rust observou da sessão, nunca de uma leitura do rodapé feita na
//! hora: sem modo conhecido, recusa.
pub(crate) mod transfer;

use super::gateway::RuntimeRegistry;
use super::protocol::RuntimeError;
use super::terminal::TerminalTarget;
use crate::list::procs::{ProcessView,SystemProcs};
use crate::session_open::claude::ClaudeContext;
use crate::session_open::terminal::{Mux, MuxOutput, PaneSpec, TerminalEnv};
use crate::session_open::{Account,ChangeReason,PermissionMode,SessionChange,SessionSpec,Surface};
use serde_json::{Value,json};
use std::path::PathBuf;
use std::time::Duration;

fn refused(code:&str,message:&str) -> RuntimeError { RuntimeError::new(code,message) }

/// As frases do `_OCUPADA` do Python, que o app já mostra.
pub fn busy_message(code:&str) -> Option<&'static str> {
    Some(match code {
        "erro_sessao_iniciando"=>"a sessão ainda está iniciando — espere ela ficar pronta",
        "erro_sessao_esperando_resposta"=>"há uma permissão ou pergunta esperando resposta",
        "erro_sessao_trabalhando"=>"a sessão está trabalhando — espere ela terminar",
        "erro_fila_pendente"=>"há mensagens na fila esperando entrega",
        _=>return None,
    })
}
fn busy(code:&'static str) -> RuntimeError { RuntimeError::new(code,busy_message(code).unwrap_or_default()) }

const EFFORTS:[&str;5] = ["low","medium","high","xhigh","max"];
/// Quanto a árvore do processo antigo tem para sair antes de a conversa ganhar outro dono.
const EXIT_WAIT:Duration = Duration::from_secs(15);
/// Quanto se espera o Claude reaberto subir. Máquina carregada leva minutos; passado isso, ele travou.
pub(crate) const STARTUP_WAIT:Duration = Duration::from_secs(300);
/// A vida nova que falhou e não saiu: nada volta por cima dela.
const NEW_ALIVE:&str = "reopen_new_alive";

/// Onde o encerramento da vida antiga parou: antes do processo sair (nada mudou), com ele vivo depois
/// de pedido o fim (não se reabre por cima), ou depois de ele sair (a sessão volta pelo spec antigo).
enum Stop { Before(RuntimeError), Alive(RuntimeError), After(RuntimeError) }

/// A vida que roda agora.
enum Life {
    Terminal { target:TerminalTarget },
    /// Sem terminal: o sidecar e, aberta aqui, a geração da entrada.
    Headless { meta:Value, open:bool },
    /// Com terminal e sem conversa provada (aberto sem id): só o pane, sem entrada aqui.
    Bare { pane_pid:i64 },
}

/// A sessão Claude como está: o spec dela (gravado ⊕ observado), a conversa, a chave, a geração da fila
/// e a base fora do plano.
struct Current { spec:SessionSpec, sid:String, generation:u64, life:Life, previous_non_plan:Option<String>, settings:Option<Value> }

pub(crate) struct EngineReopenSource { spec: SessionSpec, sid: String, generation: u64 }

impl EngineReopenSource {
    fn matches(&self, current: &Current) -> bool {
        self.sid == current.sid && self.generation == current.generation && same_engine_spec(&self.spec, &current.spec)
    }
}

fn same_engine_spec(a: &SessionSpec, b: &SessionSpec) -> bool {
    a.key == b.key && a.account == b.account && a.model == b.model && a.service_tier == b.service_tier && a.surface == b.surface
}

fn engine_source_changed() -> RuntimeError {
    refused("session_source_changed", "A conversa mudou enquanto o catálogo era conferido. Abra o seletor novamente.")
}

/// Sessão ocupada (subindo, esperando resposta, trabalhando, fila pendente) diante do `claude_current`.
#[derive(Clone,Copy,PartialEq,Eq)]
enum BusyCheck { Skip, Refuse, RefuseUnlessStalled }

/// O que a vida nova precisa, montado antes de encerrar a antiga.
enum Start { Terminal(Box<PaneSpec>,TerminalEnv), Headless(Value) }

/// A marca `asleep` do sidecar, se ele for de uma sessão adormecida.
fn asleep_of(meta:&Value) -> Option<Surface> { meta.as_object().and_then(super::gateway::asleep_surface) }

/// Onde a sessão está, pelo sidecar dela: sem ele, com terminal. A adormecida não está em nenhuma, e trocar
/// de modo nela sempre reabre.
fn surface_now(meta:Option<&Value>) -> Option<Surface> {
    match meta {
        None=>Some(Surface::Terminal),
        Some(meta) if asleep_of(meta).is_some()=>None,
        Some(_)=>Some(Surface::Headless),
    }
}

/// Motivos de desistir de adormecer: a sessão em uso, ou alguém mexendo nela.
const SLEEP_GAVE_UP:[&str;8] = ["erro_sessao_iniciando","erro_sessao_esperando_resposta","erro_sessao_trabalhando","erro_fila_pendente",
    "lifecycle_busy","lifecycle_uncertain","terminal_panel_active","session_transfer_busy"];

/// O que a reabertura devolve: o spec da vida que ficou de pé (`None` sem mudança) e o que a pessoa precisa
/// saber da troca que deu certo (motor que sumiu, registro não gravado), para a resposta mostrar.
#[derive(Debug)]
pub struct Reopened { pub spec:Option<SessionSpec>, pub changed:bool, pub warnings:Vec<String> }
impl Reopened {
    /// A resposta de sucesso da rota com os avisos em `avisos`, como a da criação.
    pub fn answer(&self,mut body:Value) -> Value {
        if !self.warnings.is_empty() {body["avisos"]=json!(self.warnings);}
        body
    }
}

/// Recusa (`stopped: false`: nada mudou) ou falha depois de encerrar a vida antiga (a mensagem diz onde a
/// sessão ficou).
#[derive(Debug)]
pub struct ReopenFailure { pub error:RuntimeError, pub stopped:bool, pub landed:Option<Surface> }
impl ReopenFailure {
    fn refused(error:RuntimeError) -> Self { Self { error,stopped:false,landed:None } }
    /// A vida antiga saiu e a sessão terminou em `landed` (`None`: sem processo).
    pub fn ended_without_terminal(&self,from_terminal:bool) -> bool { self.stopped && from_terminal && self.landed==Some(Surface::Headless) }
}

/// O que uma troca faz entre encerrar a vida antiga e dar a partida na nova (mover a conversa de conta):
/// `check` antes de qualquer efeito, `apply` com o processo antigo já fora, `undo` se a vida nova falhar.
pub trait Between: Send + Sync {
    /// O que a mudança tira do spec além do que ela mesma diz (sair do motor zera o modelo dele).
    fn adjust(&self,_next:&mut SessionSpec) {}
    fn check<'a>(&'a self,from:&'a SessionSpec,to:&'a SessionSpec,sid:&'a str) -> futures_util::future::BoxFuture<'a,Result<(),RuntimeError>>;
    fn apply<'a>(&'a self,from:&'a SessionSpec,to:&'a SessionSpec,sid:&'a str) -> futures_util::future::BoxFuture<'a,Result<(),RuntimeError>>;
    fn undo<'a>(&'a self,from:&'a SessionSpec,to:&'a SessionSpec,sid:&'a str) -> futures_util::future::BoxFuture<'a,Result<(),RuntimeError>>;
}

/// Árvore do processo com o nascimento de cada um: some quando todos saíram (ou foram reaproveitados).
fn tree(root:i64) -> Result<Vec<(i64,f64)>,RuntimeError> {
    let procs=SystemProcs::default();
    let children=procs.children(Duration::ZERO).map_err(|_|refused("lifecycle_process_read","os processos da máquina não puderam ser lidos"))?;
    Ok(crate::list::discover::descendants(root,&children).into_iter().filter_map(|pid|procs.start_time(pid).map(|birth|(pid,birth))).collect())
}

fn cleanup_uncertain(detail: &str) -> RuntimeError {
    refused(NEW_ALIVE, &format!("{detail}; nada foi reaberto por cima do Claude sem confirmar a saída"))
}

fn failed_terminal_pid(output: &MuxOutput) -> Result<Option<i64>, RuntimeError> {
    match output.code {
        Some(0) => output.stdout.trim().parse::<i64>().ok().filter(|pid| *pid > 0).map(Some)
            .ok_or_else(|| cleanup_uncertain("o multiplexador não informou o processo do terminal")),
        Some(1) if crate::list::mux::is_absence(&output.stderr) => Ok(None),
        _ => Err(cleanup_uncertain("o multiplexador não confirmou se o terminal saiu")),
    }
}

async fn wait_gone(lives:Vec<(i64,f64)>,limit:Duration) -> Result<(),RuntimeError> {
    let deadline=tokio::time::Instant::now()+limit;
    loop {
        let alive:Vec<(i64,f64)>={let pending=lives.clone();tokio::task::spawn_blocking(move||{
            let procs=SystemProcs::default();
            pending.into_iter().filter(|(pid,birth)|!super::lifecycle::process_absent(&procs,*pid,*birth).unwrap_or(false)).collect()
        }).await.unwrap_or_else(|_|lives.clone())};
        if alive.is_empty() {return Ok(());}
        if tokio::time::Instant::now()>=deadline {
            return Err(refused("reopen_process_alive","o processo antigo não saiu; para não haver dois Claude na mesma conversa, a sessão não reabriu"));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// `committed_user_lines`: os textos que chegaram ao transcript (as linhas `user` e o `enqueue` da fila
/// interna do Claude), com as variantes que a confirmação compara. Ausente ou ilegível é erro: nunca
/// "nada chegou", que reenviaria tudo.
fn committed_user_lines(path:&std::path::Path) -> std::io::Result<std::collections::BTreeSet<String>> {
    let raw=std::fs::read(path)?;
    let mut out=std::collections::BTreeSet::new();
    for line in String::from_utf8_lossy(&raw).lines() {
        if !line.contains("\"user\"") && !line.contains("\"enqueue\"") {continue;}
        let Ok(value)=serde_json::from_str::<Value>(line) else {continue};
        let text=match value["type"].as_str() {
            Some("queue-operation") if value["operation"]=="enqueue"=>value["content"].as_str().map(str::to_owned),
            Some("user")=>match &value["message"]["content"] {
                Value::String(text)=>Some(text.clone()),
                Value::Array(blocks)=>Some(blocks.iter().filter(|b|b["type"]=="text").filter_map(|b|b["text"].as_str()).collect::<Vec<_>>().join("\n")),
                _=>None,
            },
            _=>None,
        };
        if let Some(text)=text {out.extend(crate::transcript::history::chaves_de_commit(&text));}
    }
    Ok(out)
}

/// Quanto as sobras mortas à força têm para sumir.
const FORCED_WAIT:Duration = Duration::from_secs(3);

/// Espera a árvore sair; quem passa do prazo e ainda é o mesmo processo é morto à força (SIGKILL; no
/// Windows um `taskkill /F` só, sem `/T`: pid reaproveitado levaria árvore alheia) e ganha mais 3 s.
pub(super) async fn stop_tree(lives:Vec<(i64,f64)>) -> Result<(),RuntimeError> {
    // No Windows o `claude` não trata o pedido de saída: esperar o prazo inteiro só atrasa.
    let wait=if cfg!(windows) {Duration::from_millis(1500)} else {EXIT_WAIT};
    if wait_gone(lives.clone(),wait).await.is_ok() {return Ok(());}
    let left=tokio::task::spawn_blocking(move||{
        let procs=SystemProcs::default();
        let left:Vec<(i64,f64)>=lives.into_iter().filter(|(pid,birth)|!super::lifecycle::process_absent(&procs,*pid,*birth).unwrap_or(false)).collect();
        force_kill(&left);
        left
    }).await.map_err(|_|refused("reopen_process_alive","o processo antigo não pôde ser encerrado à força"))?;
    tracing::warn!(pids=?left.iter().map(|(pid,_)|*pid).collect::<Vec<_>>(),"processo antigo não saiu no prazo; morto à força");
    wait_gone(left,FORCED_WAIT).await
}

#[cfg(unix)]
fn force_kill(left:&[(i64,f64)]) {
    for (pid,_) in left {
        // SAFETY: sinal a um pid conferido pela identidade logo antes.
        if let Ok(pid)=i32::try_from(*pid) {unsafe {libc::kill(pid,libc::SIGKILL);}}
    }
}

#[cfg(windows)]
fn force_kill(left:&[(i64,f64)]) {
    if left.is_empty() {return;}
    let system=std::env::var_os("SystemRoot").map(|root|PathBuf::from(root).join("System32").join("taskkill.exe")).filter(|path|path.is_file());
    let mut command=std::process::Command::new(system.unwrap_or_else(||PathBuf::from("taskkill")));
    command.arg("/F");
    for (pid,_) in left {command.args(["/PID",&pid.to_string()]);}
    let ran=command.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status();
    if ran.is_err() {tracing::warn!(code="reopen_taskkill_missing","taskkill não rodou; as sobras do processo antigo ficaram");}
}

/// A política que a CLI aplica ao bypass (`permissions.disableBypassPermissionsMode`), lida antes de
/// encerrar a vida antiga: a conta, o projeto, a gerenciada da máquina e as escolhas da sessão. Arquivo
/// ilegível recusa.
fn bypass_policy(config:&std::path::Path,cwd:&std::path::Path,settings:Option<&Value>,env:&std::collections::BTreeMap<String,String>) -> Result<(),RuntimeError> {
    let forbids=|value:&Value|value["permissions"]["disableBypassPermissionsMode"]=="disable";
    let mut paths=vec![config.join("settings.json"),cwd.join(".claude/settings.json"),cwd.join(".claude/settings.local.json")];
    if cfg!(target_os="linux") {paths.push(PathBuf::from("/etc/claude-code/managed-settings.json"));}
    if cfg!(target_os="macos") {paths.push(PathBuf::from("/Library/Application Support/ClaudeCode/managed-settings.json"));}
    if cfg!(windows) {
        // Nome de variável no Windows não tem caixa, e o Python repassa o ambiente em maiúsculas.
        let program_files=env.iter().find(|(key,_)|key.eq_ignore_ascii_case("ProgramFiles")).map(|(_,value)|value).ok_or_else(||refused("relaunch_policy_source","a pasta de programas do Windows não é conhecida"))?;
        paths.push(PathBuf::from(program_files).join("ClaudeCode/managed-settings.json"));
    }
    for path in paths {
        let bytes=match std::fs::read(&path) {Ok(bytes)=>bytes,Err(error) if error.kind()==std::io::ErrorKind::NotFound=>continue,
            Err(_)=>return Err(refused("relaunch_policy_read","a política de permissões não pôde ser lida; nada foi reaberto"))};
        let value:Value=serde_json::from_slice(&bytes).map_err(|_|refused("relaunch_policy_read","a política de permissões não pôde ser lida; nada foi reaberto"))?;
        if forbids(&value) {return Err(refused("relaunch_policy_forbids_bypass","a política de permissões desta máquina ou conta proíbe o bypass"));}
    }
    if settings.is_some_and(forbids) {return Err(refused("relaunch_policy_forbids_bypass","as escolhas desta sessão proíbem o bypass"));}
    Ok(())
}

/// A identidade que a CLI lê do ambiente: a pasta da conta, o motor, a conta fixa e o Fast.
fn identity(env:&crate::accounts::environment::AccountEnvironment,spec:&SessionSpec)
    -> (PathBuf,Option<String>,Option<String>,Option<crate::session_open::ServiceTier>) {
    let config=spec.account.config_dir().map_or_else(||env.claude_base.clone(),PathBuf::from);
    let config=std::fs::canonicalize(&config).unwrap_or(config);
    match &spec.account {
        Account::Engine { name,account,.. }=>(config,Some(name.clone()),account.clone(),spec.service_tier),
        _=>(config,None,None,None),
    }
}

/// `default_model.anthropic`: alias, id da Anthropic (Bedrock e Vertex incluídos) ou ARN do Bedrock.
static ANTHROPIC:std::sync::LazyLock<regex::Regex>=std::sync::LazyLock::new(||regex::Regex::new(
    r"(?i)\A(?:(?:[a-z]{2,}\.)?(?:anthropic\.)?claude-[\w.:@-]+|arn:aws[\w-]*:bedrock:\S+|default|best|opus|opusplan|sonnet|haiku|fable)(?:\[1m\])?\z").unwrap());

/// O que a vida nova leva e a CLI recusaria, conferido antes de encerrar a antiga (o `_comando_terminal` e o
/// `para_headless` do Python): o motor que sumiu, o modelo malformado ou de outro provedor, o Fast sem
/// suporte e o corpo do motor ilegível. Bloqueante. Devolve os avisos para a pessoa (o motor que sumiu).
fn preflight(env:&crate::accounts::environment::AccountEnvironment,next:&mut SessionSpec,settings:Option<&Value>,
             runtime:tokio::runtime::Handle,automatic:bool) -> Result<Vec<String>,RuntimeError> {
    use crate::session_open::OpenFacts;
    let facts=crate::session_open::facts::LocalFacts::new(env.clone(),runtime);
    let mut warnings=Vec::new();
    if let Account::Engine { name,account,binding,config_dir,.. }=&mut next.account {
        // Ilegível não diz que o motor sumiu: tirar a sessão dele seria trocar a conta calado.
        let engines=crate::session_open::facts::engine_names(env)
            .map_err(|_|refused("reopen_engines_unreadable","o catálogo de motores (engines.json) não pôde ser lido; nada foi reaberto"))?;
        if engines.contains(name) {
            let route = crate::session_open::facts::session_engine_route(&crate::accounts::AccountService::new(env.clone()), name, account.as_deref(), binding.as_deref())
                .map_err(|error| refused("launch_engine_account", &error.msg))?;
            if let Some(route) = route { *account = Some(route.account); }
        } else {
            if account.is_some() || binding.is_some() {return Err(refused("reopen_engine_missing","motor da conta ChatGPT fixa indisponível; nada foi reaberto"));}
            // A escolha lida é do motor: numa conta Anthropic ela criaria uma sessão inviável.
            tracing::warn!(session=%next.name,engine=%name,"motor sumiu do catálogo; a sessão volta na conta Anthropic");
            warnings.push(format!("O motor {name} saiu do catálogo: a sessão voltou na conta Anthropic, sem o modelo, o esforço e o Fast dele."));
            next.account=config_dir.clone().map_or(Account::Default,|config_dir|Account::Claude { config_dir });
            (next.model,next.effort,next.service_tier)=(None,None,None);
        }
    }
    if let Some(model)=next.model.as_deref() && !crate::session_open::resolve::valid_model(model) {
        return Err(refused("reopen_model_invalid","model: use letras, números e . _ : / - ~ [ ] (até 128 caracteres, sem começar com -)"));
    }
    let engine=match &next.account {Account::Engine { name,.. }=>Some(name.clone()),_=>None};
    if engine.is_none() && next.model.as_deref().is_some_and(|model|!ANTHROPIC.is_match(model.trim())) {
        tracing::warn!(session=%next.name,"modelo que não é da Anthropic; a sessão volta no padrão da conta");
        next.model=None;
    }
    // Acordar, restaurar e a troca de conta do limite não são pedido da pessoa: a velocidade que perdeu o suporte
    // cai (Ultra Fast em Fast, Fast na normal) em vez de a sessão não voltar.
    if automatic {next.service_tier=crate::session_open::claude::relaunch_tier(next,&facts);}
    if let Some(tier)=next.service_tier && !facts.supports_fast(engine.as_deref(),next.model.as_deref()) {
        if tier.is_fast() {
            return Err(refused("launch_service_tier","service_tier exige Claude com motor GPT no CLIProxyAPI local"));
        }
        if !matches!(next.account, Account::Engine { binding: Some(_), .. }) { next.service_tier=None; }
    }
    // Mesma regra da criação e do lançamento sem terminal, antes de parar a sessão atual: com terminal o painel
    // abre sem passar pelo `plan_for`.
    if next.service_tier==Some(crate::session_open::ServiceTier::Ultrafast) {
        let account=match &next.account {Account::Engine { account,.. }=>account.clone(),_=>None};
        if !facts.supports_ultrafast(engine.as_deref(),next.model.as_deref(),account.as_deref()) {
            return Err(refused("launch_service_tier",crate::engines::catalog::ULTRAFAST_REFUSED));
        }
    }
    if next.service_tier.is_some() {engine_body(env,next.account.config_dir(),std::path::Path::new(&next.cwd),settings)?;}
    Ok(warnings)
}

fn validate_provider_destination(env: &crate::accounts::environment::AccountEnvironment, next: &mut SessionSpec, handle: &tokio::runtime::Handle) -> Result<crate::session_open::claude::EngineRoute, RuntimeError> {
    use crate::session_open::facts::{LocalFacts, claude_provider_route};
    let fail = |e: crate::session_open::OpenError| refused(e.code.unwrap_or("erro_troca_conta"), &e.msg);
    let Account::Engine { name, account: Some(account), binding: Some(binding), context_window, .. } = &mut next.account else {
        return Err(refused("erro_cliproxy_conta", "Destino sem conta ChatGPT vinculada"));
    };
    let model = next.model.as_deref().filter(|model| !model.is_empty())
        .ok_or_else(|| refused("erro_modelo_fora_catalogo", "Escolha um modelo da conta de destino."))?;
    let facts = LocalFacts::new(env.clone(), handle.clone());
    let service = crate::accounts::AccountService::new(env.clone());
    let route = claude_provider_route(&service, name, account, binding).map_err(fail)?;
    let (base, window) = facts.fixed_engine_selection(name, account, Some(model)).map_err(fail)?;
    *context_window = window;
    let current = claude_provider_route(&service, name, account, binding).map_err(fail)?;
    if route.prefix != current.prefix || route.base_url != current.base_url {
        return Err(refused("erro_cliproxy_conta", "O destino mudou enquanto o catálogo era conferido. Escolha novamente."));
    }
    next.model = Some(base);
    Ok(route)
}

/// O pré-voo do `engines.service_tier_settings`: o `CLAUDE_CODE_EXTRA_BODY` que o lançador do motor vai
/// ler (ambiente do servidor, escolhas da conta, do projeto e locais, e as da sessão) tem de ser um objeto.
fn engine_body(env:&crate::accounts::environment::AccountEnvironment,config_dir:Option<&str>,cwd:&std::path::Path,explicit:Option<&Value>) -> Result<(),RuntimeError> {
    let bad=|what:&str|refused("reopen_engine_body",&format!("{what}; nada foi reaberto"));
    let read=|path:&std::path::Path|->Result<Value,RuntimeError> {
        let value=match std::fs::read(path) {
            Err(error) if error.kind()==std::io::ErrorKind::NotFound=>return Ok(json!({})),
            Err(_)=>return Err(bad("settings: arquivo ilegível ou JSON inválido")),
            Ok(raw)=>serde_json::from_slice::<Value>(&raw).map_err(|_|bad("settings: arquivo ilegível ou JSON inválido"))?,
        };
        if !value.is_object() || value.get("env").is_some_and(|e|!e.is_object()) {return Err(bad("settings: esperado um objeto JSON com env como objeto"));}
        Ok(value)
    };
    let cwd=&std::fs::canonicalize(cwd).unwrap_or_else(|_|cwd.to_owned());
    let project=cwd.join(".claude");
    let mut paths=vec![config_dir.map_or_else(||env.claude_base.clone(),PathBuf::from).join("settings.json"),project.join("settings.json"),project.join("settings.local.json")];
    let root=local_settings_root(cwd,&env.home);
    if &root!=cwd {paths.push(root.join(".claude").join("settings.local.json"));}
    let mut extra=Value::from(env.base.get("CLAUDE_CODE_EXTRA_BODY").map_or("{}",String::as_str));
    for path in paths { if let Some(value)=read(&path)?["env"].get("CLAUDE_CODE_EXTRA_BODY") {extra=value.clone();} }
    if let Some(value)=explicit.and_then(|e|e["env"].get("CLAUDE_CODE_EXTRA_BODY")) {extra=value.clone();}
    let Value::String(text)=extra else {return Err(bad("CLAUDE_CODE_EXTRA_BODY: esperado texto JSON"))};
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(_))=>Ok(()),
        Ok(_)=>Err(bad("CLAUDE_CODE_EXTRA_BODY: esperado um objeto JSON")),
        Err(_)=>Err(bad("CLAUDE_CODE_EXTRA_BODY: JSON inválido")),
    }
}

/// `_local_settings_root`: o Claude lê o `settings.local.json` da raiz Git no POSIX, só com a raiz do usuário.
fn local_settings_root(cwd:&std::path::Path,home:&std::path::Path) -> PathBuf {
    #[cfg(unix)] {
        use std::os::unix::fs::MetadataExt;
        // SAFETY: geteuid lê a identidade do processo.
        let uid=unsafe {libc::geteuid()};
        let home=std::fs::canonicalize(home).unwrap_or_else(|_|home.to_owned());
        for root in cwd.ancestors() {
            if !root.join(".git").exists() {continue;}
            if root==home {return cwd.to_owned();}
            let mut entries=vec![root.to_owned(),root.join(".git")];
            if root.join(".claude").exists() {entries.push(root.join(".claude"));}
            let owned=entries.iter().all(|path|std::fs::metadata(path).is_ok_and(|m|m.uid()==uid));
            return if owned {root.to_owned()} else {cwd.to_owned()};
        }
    }
    #[cfg(not(unix))] let _=home;
    cwd.to_owned()
}

/// Spec de um Claude com terminal aberto antes do registro, lido do processo vivo: o que o `para_headless`
/// do Python lia do `/proc`. Só para essas sessões; a reabertura grava o spec dali em diante.
fn legacy_terminal_spec(target:&TerminalTarget,name:&str,cwd:String) -> Result<(SessionSpec,Option<Value>),RuntimeError> {
    let mut spec=process_spec(i64::from(target.metadata.agent_pid),&target.key,name,cwd,target.created)?;
    spec.0.conversation=Some(target.binding.conversation.clone());
    Ok(spec)
}

/// O spec do Claude que roda no processo `pid`: conta, motor, modelo, esforço, modo, Fast, Jev e só leitura.
pub(super) fn process_spec(pid:i64,key:&str,name:&str,cwd:String,created:f64) -> Result<(SessionSpec,Option<Value>),RuntimeError> {
    spec_of(&SystemProcs::default(),pid,key,name,cwd,created)
}

fn spec_of(procs:&dyn ProcessView,pid:i64,key:&str,name:&str,cwd:String,created:f64) -> Result<(SessionSpec,Option<Value>),RuntimeError> {
    // Ambiente ilegível não diz se a sessão roda protegida (só leitura) nem em que conta.
    procs.env_var(pid,"HANGAR_ORQ_READ_ONLY").map_err(|_|refused("lifecycle_process_read","o ambiente do Claude não pôde ser lido; nada foi encerrado"))?;
    let env=|var:&str|procs.env_var(pid,var).ok().flatten().map(|v|v.to_string_lossy().into_owned()).filter(|v|!v.is_empty());
    let argv=procs.argv_checked(pid).map_err(|_|refused("lifecycle_process_read","o processo do Claude não pôde ser lido"))?;
    let flag=|name:&str|argv.windows(2).find(|w|w[0]==name).map(|w|w[1].clone())
        .or_else(||argv.iter().find_map(|a|a.strip_prefix(&format!("{name}=")).map(str::to_owned)));
    let config_dir=env("CLAUDE_CONFIG_DIR");
    let account=match env("CP_ENGINE") {
        Some(engine)=>Account::Engine { name:engine,account:env("CP_ENGINE_ACCOUNT"),binding:env("CP_ENGINE_BINDING"),config_dir,
            context_window:env("CLAUDE_CODE_MAX_CONTEXT_TOKENS").and_then(|v|v.parse().ok()),context_window_unknown:false },
        None=>config_dir.map_or(Account::Default,|config_dir|Account::Claude { config_dir }),
    };
    let engine=matches!(account,Account::Engine{..});
    let settings=env("HANGAR_CLAUDE_SETTINGS").and_then(|raw|serde_json::from_str::<Value>(&raw).ok()).filter(|v|v.as_object().is_some_and(|o|!o.is_empty()));
    let tier=env("CP_ENGINE_SERVICE_TIER").and_then(|t|serde_json::from_value(Value::from(t)).ok());
    let spec=SessionSpec { version:crate::session_open::SPEC_VERSION,key:key.into(),name:name.into(),cwd,
        provider:crate::session_open::AgentProvider::Claude,surface:Surface::Terminal,account,account_source:crate::session_open::AccountSource::Explicit,
        model:flag("--model"),effort:flag("--effort"),service_tier:tier,
        permission_mode:flag("--permission-mode").as_deref().and_then(PermissionMode::claude),
        subagent_model:if engine {None} else {env("CLAUDE_CODE_SUBAGENT_MODEL")},customizations:None,
        jev:env("HANGAR_JEV").as_deref()==Some("on"),code_mode:env(crate::session_open::claude::CODE_MODE_ENV).is_some_and(|v|crate::config_sync::runtime::env_value_on(&Value::from(v))),
        read_only:env("HANGAR_ORQ_READ_ONLY").as_deref()==Some("1"),
        worktree:None,creator:None,created_at:created,conversation:None,name_auto:None,rename_suggestion:None,rename_weak_at:None,rename_dismissed:false,hooks_off:false };
    Ok((spec,settings))
}

/// A conta do Hangar em que um Claude novo vai subir se reconcilia antes, como na abertura: sem isso a cópia
/// do `settings.json` dela segue com o que o principal já mudou. Conta padrão não tem o que reconciliar.
pub(super) async fn reconcile_account(env:&crate::accounts::environment::AccountEnvironment,config_dir:Option<&str>,cwd:&str) -> Result<Vec<String>,RuntimeError> {
    let Some(dir)=config_dir.filter(|dir|!dir.is_empty()) else {return Ok(Vec::new())};
    let Some(account)=crate::session_open::claude::hangar_account(std::path::Path::new(dir)) else {return Ok(Vec::new())};
    let diary=crate::diag::diary_dir_in(env);
    let (_,result)=crate::session_open::open::reconcile_hangar_dir(env.home.clone(),Some(env.default_claude_json()),dir,&account,cwd,crate::session_open::open::RECONCILE_LIMIT,None,
        |events|{crate::diag::record(diary,events,"conta");}).await;
    result.map_err(|error|refused(error.code.unwrap_or("erro_conta_reconciliacao_falhou"),&error.msg))
}

#[derive(serde::Serialize, serde::Deserialize)]
struct ProviderSwitchRecord { generation: u64, source: SessionSpec, spec: SessionSpec }

struct SwitchMark { path: PathBuf, keep: bool }
impl SwitchMark {
    fn clear(mut self) -> std::io::Result<()> {
        self.keep = true;
        match std::fs::remove_file(&self.path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            result => result,
        }
    }

    fn write(home:&std::path::Path,next:&SessionSpec) -> Result<Option<Self>,RuntimeError> {
        // Sessão adotada do terminal não tem registro: a subida não teria o que acertar.
        if crate::session_open::record::path(home,&next.key).is_err() {return Ok(None);}
        let unmarked=|_|refused("reopen_switch_mark","a troca não pôde ser marcada; nada foi reaberto");
        crate::session_open::record::write_switching(home,next).map_err(unmarked)?;
        crate::session_open::record::switching_path(home,&next.key).map(|path|Some(SwitchMark { path, keep: false })).map_err(unmarked)
    }

    fn provider(home: &std::path::Path, current: &Current, next: &SessionSpec) -> std::io::Result<Self> {
        let path = crate::session_open::record::path(home, &next.key)?.with_extension("provider-switch");
        std::fs::create_dir_all(crate::session_open::record::dir(home))?;
        let mut source = current.spec.clone();
        source.conversation = Some(current.sid.clone());
        let record = ProviderSwitchRecord { generation: current.generation + 1, source, spec: next.clone() };
        super::queue::atomic_write(&path, &serde_json::to_vec(&record)?)?;
        Ok(Self { path, keep: true })
    }

    fn restore_origin(&self) -> std::io::Result<()> {
        let mut record: ProviderSwitchRecord = serde_json::from_slice(&super::queue::read_replaced(&self.path)?)?;
        record.spec = record.source.clone();
        super::queue::atomic_write(&self.path, &serde_json::to_vec(&record)?)
    }
}
impl Drop for SwitchMark {
    fn drop(&mut self) {
        if !self.keep && let Err(error)=std::fs::remove_file(&self.path) && error.kind()!=std::io::ErrorKind::NotFound {
            tracing::warn!(kind=?error.kind(),"marca da troca de conta ficou; a subida a resolve");
        }
    }
}

/// A troca de conta interrompida no meio (o Rust caiu entre mover a conversa e gravar a vida nova): na subida,
/// vale a conta em que a conversa está. Com ela no destino, o registro e o arquivo da sessão sem terminal passam
/// para ele; senão nada muda. Bloqueante.
pub fn settle_switches(env:&crate::accounts::environment::AccountEnvironment) {
    let dir=crate::session_open::record::dir(&env.home);
    let Ok(entries)=std::fs::read_dir(&dir) else {return};
    for path in entries.flatten().map(|e|e.path()).filter(|p|p.extension().is_some_and(|x|x=="switching" || x=="provider-switch")) {
        let provider = path.extension().is_some_and(|x|x=="provider-switch");
        let read = super::queue::read_replaced(&path).and_then(|raw| {
            if provider {
                provider_switch_recovery(env, serde_json::from_slice(&raw)?)
            } else {
                let spec: SessionSpec = serde_json::from_slice(&raw)?;
                let moved = spec.conversation.as_deref().is_some_and(|sid| super::claude_launch::transcript(env, &spec, sid).is_file());
                Ok(moved.then_some((spec, true)))
            }
        });
        match read {
            Ok(Some((to, committed)))=>{
                let terminal_switch = provider && to.surface == Surface::Terminal;
                let result = if committed {
                    let sidecar = if terminal_switch { Ok(()) } else { settle_sidecar(env, &to, provider) };
                    sidecar.and_then(|_| crate::session_open::record::write(&env.home,&to))
                        .and_then(|_| crate::session_open::record::confirm_binding(&env.home,&to))
                } else { Ok(()) };
                if let Err(error) = result {
                    tracing::warn!(session=%to.name,kind=?error.kind(),"troca interrompida não gravada; marca conservada");
                    continue;
                }
                if terminal_switch {
                    // Com o mesmo multiplexador, a restauração comum não reabre o terminal que saiu na troca.
                    let recorded = crate::session_open::fallen::remember_provider_switch(&env.home, &to.name, crate::session_open::fallen::Fallen {
                        key: to.key.clone(), provider: to.provider, conversation: to.conversation,
                        permission_mode: to.permission_mode.map(|mode| mode.name().to_owned()), cwd: Some(to.cwd),
                        died_at: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs_f64(),
                        cause: "provider_switch_interrupted".into(), orphan_pid: None, orphan_birth: None, orphan_descendants: Vec::new(), orphan_transcript: None,
                    });
                    if !recorded { continue; }
                }
            },
            Ok(_)=>{},
            Err(error)=>{
                tracing::warn!(path=%path.display(),kind=?error.kind(),"marca de troca de conta ilegível; marca conservada");
                continue;
            },
        }
        if let Err(error) = std::fs::remove_file(&path) && error.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!(kind=?error.kind(), "marca da troca recuperada não pôde ser removida");
        }
    }
}

fn provider_switch_recovery(env: &crate::accounts::environment::AccountEnvironment, record: ProviderSwitchRecord) -> std::io::Result<Option<(SessionSpec, bool)>> {
    crate::session_open::record::path(&env.home, &record.spec.key)?;
    let (state, _) = super::codex_terminal::queue_files(&env.claude_base.join(".hangar-queue"), &record.spec.key);
    let state = super::queue::read_replaced(&state)?;
    let (state, _) = super::queue::State::load(&state)?;
    if state.owner_key != record.spec.key || state.name != record.spec.name || state.runtime_state["terminal_lifecycle"]["closed"] == true {
        return Ok(None);
    }
    if state.generation == record.generation {
        Ok(Some((record.spec, true)))
    } else if state.generation.checked_add(1) == Some(record.generation) && record.source.surface == Surface::Terminal {
        Ok(Some((record.source, false)))
    } else {
        Ok(None)
    }
}

/// O sidecar sem terminal da sessão passa a dizer a conta de destino (é dele que a subida lê a conta).
fn settle_sidecar(env:&crate::accounts::environment::AccountEnvironment,to:&SessionSpec,provider:bool) -> std::io::Result<()> {
    let dir=env.home.join(".hangar").join("claude-headless");
    let Some(mut meta)=super::codex_sidecar::read(&super::codex_sidecar::path(&dir,&to.name))? else {return Ok(())};
    if meta.get("key").and_then(Value::as_str)!=Some(to.key.as_str()) {return Ok(());}
    if provider && meta.get("session_id").and_then(Value::as_str)!=to.conversation.as_deref() {
        return Err(std::io::Error::other("a conversa da troca interrompida mudou"));
    }
    let route = match &to.account {
        Account::Engine { name, account, binding, .. } => crate::session_open::facts::session_engine_route(
            &crate::accounts::AccountService::new(env.clone()), name, account.as_deref(), binding.as_deref()).ok().flatten(),
        _ => None,
    };
    let mut fresh=crate::session_open::open::headless_sidecar(to,to.conversation.as_deref().unwrap_or_default(),None,None,route.as_ref());
    if let Account::Engine { account:Some(account), .. } = &to.account && route.is_none() {
        fresh["engine_account"] = json!(account);
    }
    if provider {
        let matches_binding = ["engine", "engine_binding"].iter().all(|field| meta.get(*field).unwrap_or(&Value::Null) == &fresh[*field]);
        if let Some(cano) = meta.get("cano").filter(|value| !value.is_null()) {
            let gone = cano["pid"].as_i64().zip(cano["process_birth"].as_f64()).is_some_and(|(pid, birth)| {
                super::lifecycle::process_absent(&SystemProcs::default(), pid, birth).unwrap_or(false)
            });
            if gone {
                meta.remove("cano");
            } else if !matches_binding {
                return Err(std::io::Error::other("o processo da conta anterior não pôde ser descartado"));
            }
        }
        if !matches_binding { meta.remove("problema"); }
    }
    for field in ["config_dir","engine","engine_binding","engine_account","engine_credential_id","engine_account_base_url","model","effort","subagent_model","context_window","service_tier"] {
        match fresh.get(field).filter(|v|!v.is_null()) {Some(value)=>{meta.insert(field.into(),value.clone());},None=>{meta.remove(field);}}
    }
    super::codex_sidecar::put(&dir,&to.name,&meta)
}

impl RuntimeRegistry {
    fn claude_sidecars(&self) -> PathBuf { self.codex_environment().home.join(".hangar").join("claude-headless") }

    async fn claude_sidecar(&self,name:&str) -> Result<Option<Value>,RuntimeError> {
        let path=super::codex_sidecar::path(&self.claude_sidecars(),name);
        match tokio::task::spawn_blocking(move||super::codex_sidecar::read(&path)).await {
            Ok(Ok(meta))=>Ok(meta.map(Value::Object)),
            _=>Err(refused("claude_sidecar_read","o arquivo da sessão sem terminal não pôde ser lido")),
        }
    }

    /// O último modo fora do plano no registro sem terminal (o da parada e o da adormecida); `None` sem ele.
    pub async fn claude_previous_non_plan(&self,name:&str) -> Option<String> {
        self.claude_sidecar(name).await.ok().flatten()?.get("previous_non_plan")?.as_str().map(str::to_owned)
    }

    /// O spec que a sessão Claude `name` roda agora (gravado ⊕ observado), sem conferir se está parada.
    /// `None` = não é sessão Claude daqui.
    pub async fn claude_spec(&self,name:&str) -> Option<Result<SessionSpec,RuntimeError>> {
        match self.claude_sidecar(name).await {
            Err(error)=>return Some(Err(error)),
            Ok(Some(_))=>{},
            Ok(None)=>if let Err(error)=self.watcher_ref()?.adopt_claude(self,name).await? {return Some(Err(error));},
        }
        Some(self.claude_current(name,BusyCheck::Skip,false).await.map(|current|current.spec))
    }

    /// O que o fork do `/btw` precisa da sessão Claude com terminal `name`, nas chaves do sidecar sem
    /// terminal: a conversa de agora do pane, a pasta, a conta, o modelo e o motor.
    pub async fn claude_fork_meta(&self,name:&str) -> Result<Value,RuntimeError> {
        let current=self.claude_current(name,BusyCheck::Skip,false).await?;
        let Life::Terminal { target }=&current.life else { return Err(refused("runtime_provider","a sessão não tem terminal")) };
        let spec=&current.spec;
        // A pasta do processo vivo, não a do spec: é nela que a CLI gravou a conversa.
        let mut meta=json!({"session_id":current.sid,"cwd":spec.cwd,"config_dir":target.metadata.config_dir,"model":spec.model});
        if let Account::Engine { name,account,binding,context_window,.. }=&spec.account {
            meta["engine"]=json!(name); meta["context_window"]=json!(context_window);
            meta["engine_account"]=json!(account); meta["engine_binding"]=json!(binding);
        }
        Ok(meta)
    }

    /// Fecha com Esc o menu do limite do Claude Code com terminal, se ele está no rodapé do pane. `Ok(false)`: sem
    /// terminal (o menu é da TUI) ou sem o menu.
    pub async fn dismiss_claude_limit_menu(&self,name:&str) -> Result<bool,RuntimeError> {
        use crate::mods::click::{PaneOp,PaneReply};
        let Some((_,handle))=self.claude_terminal_entry(name).await else { return Ok(false) };
        let PaneReply::Screen(screen)=handle.pane(PaneOp::Screen,std::time::Instant::now()+std::time::Duration::from_secs(5)).await? else { return Ok(false) };
        if !crate::session_write::limit_switch::limit_menu_open(&screen) { return Ok(false); }
        let reply=handle.control(crate::mods::state::random_hex(16),"navigation_key".into(),json!({"key":"Escape"})).await?;
        if reply.disposition!=crate::runtime::protocol::Disposition::Accepted { return Err(refused("limit_menu_escape","o Esc não chegou ao menu do limite")); }
        Ok(true)
    }

    /// O modo que o Rust confirmou por último na conversa (memória dele, sem leitura do rodapé).
    pub fn observed_claude_mode(&self,conversation:&str) -> Option<String> { self.permission_view(conversation).map(|view|view.current) }

    /// O modelo que a sessão Claude `name` usa agora (a vista sem terminal ou a linha de status do terminal);
    /// `None` = o do spec, sem nada mais novo.
    pub async fn claude_model_in_use(&self,name:&str) -> Option<String> {
        if let Some((_,view))=self.claude_headless_view(name).await {return view["model"].as_str().filter(|m|!m.is_empty()).map(str::to_owned);}
        let (target,_)=self.claude_terminal_entry(name).await?;
        super::lifecycle::terminal_choices(&target).ok().flatten()?.model.filter(|m|!m.is_empty())
    }

    /// A sessão Claude `name` está sem terminal (tem sidecar)? `None` com o sidecar ilegível.
    pub async fn claude_headless(&self,name:&str) -> Option<bool> { self.claude_sidecar(name).await.ok().map(|meta|meta.is_some()) }

    /// Reabre a sessão Claude `name` com a mudança. `None` = não é sessão Claude daqui (nem sidecar nem pane
    /// com uma conversa Claude), e a rota segue o caminho de sempre. Troca de modo para o modo em que a
    /// sessão já está não toca em nada (`changed: false`).
    pub async fn reopen_claude_session(&self,name:&str,change:SessionChange,between:Option<&dyn Between>) -> Option<Result<Reopened,ReopenFailure>> {
        let mut warnings=Vec::new();
        let surface=match self.claude_sidecar(name).await {
            Err(error)=>return Some(Err(ReopenFailure::refused(error))),
            Ok(Some(meta))=>surface_now(Some(&meta)),
            Ok(None)=>{
                let watcher=self.watcher_ref()?;
                let to_terminal=change.reason==crate::session_open::ChangeReason::ModeSwitch && change.surface==Some(Surface::Terminal);
                // Sem sidecar a sessão já é com terminal: ir a ele não depende da adoção, que o Claude recém-nascido
                // no pane (outra troca em curso) ainda não prova sem o registro nativo.
                if let Err(error)=watcher.adopt_claude(self,name).await? {
                    if !to_terminal {return Some(Err(ReopenFailure::refused(error)));}
                    warnings.push(format!("o Claude do terminal ainda não foi adotado: {} ({})",error.message,error.code));
                }
                Some(Surface::Terminal)
            },
        };
        if change.reason==ChangeReason::ModeSwitch && surface.is_some() && change.surface==surface {
            return Some(Ok(Reopened { spec:None,changed:false,warnings }));
        }
        Some(self.reopen_by_spec(name,change,between,None).await)
    }

    /// `/resume`: a sessão Claude `name` volta na conversa `sid`, que já existe, pelo mesmo caminho das outras
    /// reaberturas: a vida nova é montada antes, a antiga sai inteira antes de a nova subir e a
    /// falha volta pelo spec de antes. O pane sem conversa provada também. `None` = não é Claude daqui.
    pub async fn resume_claude_session(&self,name:&str,sid:&str) -> Option<Result<Reopened,ReopenFailure>> {
        if let Err(error)=self.claude_resume_spec(name).await? {return Some(Err(ReopenFailure::refused(error)));}
        let change=SessionChange { reason:crate::session_open::ChangeReason::Resume,surface:None,account:None,permission_mode:None,
            model:None,effort:None,service_tier:None };
        Some(self.reopen_by_spec(name,change,None,Some(sid)).await)
    }

    /// O spec da sessão Claude `name` para retomar outra conversa: sem terminal, adotada ou com o pane sem
    /// conversa provada (o caso do `/resume`). `None` = não é Claude daqui.
    pub async fn claude_resume_spec(&self,name:&str) -> Option<Result<SessionSpec,RuntimeError>> {
        match self.claude_sidecar(name).await {
            Err(error)=>return Some(Err(error)),
            Ok(Some(_))=>return Some(self.claude_current(name,BusyCheck::Skip,false).await.map(|current|current.spec)),
            Ok(None)=>{},
        }
        let watcher=self.watcher_ref()?;
        match watcher.adopt_claude(self,name).await {
            Some(Ok(()))=>Some(self.claude_current(name,BusyCheck::Skip,false).await.map(|current|current.spec)),
            // Sem conversa provada é o caso do `/resume`: a sessão aberta sem id.
            Some(Err(error)) if error.code!="claude_terminal_conversation_unproved"=>Some(Err(error)),
            _=>match watcher.bare_claude(name).await {
                Ok(Some(_))=>Some(self.bare_current(name).await.map(|current|current.spec)),
                Ok(None)=>None,
                Err(error)=>Some(Err(error)),
            },
        }
    }

    /// A vigia de adormecer pede: a sessão Claude com terminal `name`, parada há `after` e sem nada em aberto,
    /// tem o Claude e o terminal fechados e fica no sidecar sem terminal marcado `asleep`, sem processo; o spec
    /// gravado segue com terminal. Tudo é conferido de novo com a porta fechada. `Ok(false)`: desistiu.
    pub async fn sleep_claude_terminal(&self,name:&str,after:Duration) -> Result<bool,RuntimeError> {
        if self.waking_key(name).is_some() || self.claude_sidecar(name).await?.is_some() {return Ok(false);}
        let watcher=self.watcher_ref().ok_or_else(||refused("runtime_binding","sem a vigia do terminal"))?;
        match watcher.adopt_claude(self,name).await {
            Some(Ok(()))=>{},
            None=>return Ok(false),
            Some(Err(error))=>return Err(error),
        }
        let Ok(_ingress)=super::lifecycle::ClosedIngress::acquire_within(self.ingress(),name,Duration::from_secs(1)).await else {return Ok(false)};
        let home=self.codex_environment().home;
        // A porta fechada não exclui a troca de agente nem o acordar: os dois são conferidos aqui.
        if self.handoff_held(name) || super::codex_relaunch::transfer_open(&home,name,None,None) || self.waking_key(name).is_some() || self.claude_sidecar(name).await?.is_some() {
            return Ok(false);
        }
        // Pergunta, fila, entrega incerta, turno e painel: a guarda da reabertura.
        let current=match self.claude_current(name,BusyCheck::Refuse,false).await {
            Ok(current)=>current,
            Err(error) if SLEEP_GAVE_UP.contains(&error.code.as_str())=>return Ok(false),
            Err(error)=>return Err(error),
        };
        let Life::Terminal { target }=&current.life else {return Ok(false)};
        // Só leitura não roda sem terminal, e o sidecar dela seria o da estacionada à espera do terminal protegido.
        if current.spec.read_only {return Ok(false);}
        let (transcript,agent,mux)=(target.transcript.clone(),i64::from(target.metadata.agent_pid),target.binding.mux_argv.first().cloned());
        // Parada pela conversa: a linha da lista não acompanha a última atividade.
        let idle=tokio::task::spawn_blocking(move||crate::list::reply::conversation_idle(crate::transcript::Provider::Claude,&transcript)).await.ok().flatten();
        let Some(idle)=idle.filter(|idle|*idle>=after) else {return Ok(false)};
        // Filhos ilegíveis não dizem que nenhum comando roda.
        let quiet=tokio::task::spawn_blocking(move||{
            let procs=SystemProcs::default();
            procs.children(Duration::ZERO).is_ok() && crate::state::shells::shells_of(agent,&procs).is_empty()
        }).await.unwrap_or(false);
        if !quiet || self.panel_open(name) {return Ok(false);}
        // Um terminal de verdade olhando a sessão fecharia com ela; os clientes de controle do Hangar não
        // contam (`mods_clients`). Sem resposta, também desiste.
        let Some((_,handle))=self.claude_terminal_entry(name).await else {return Ok(false)};
        let clients=handle.pane(crate::mods::click::PaneOp::Clients,std::time::Instant::now()+Duration::from_secs(5)).await;
        if !matches!(clients,Ok(crate::mods::click::PaneReply::Clients(0))) {return Ok(false);}
        let change=SessionChange { reason:ChangeReason::Sleep,surface:Some(Surface::Headless),account:None,permission_mode:None,model:None,effort:None,service_tier:None };
        let mut next=current.spec.with(&change);
        next.conversation=Some(current.sid.clone());
        let previous=current.previous_non_plan.as_deref().and_then(PermissionMode::claude);
        let mut meta=crate::session_open::open::headless_sidecar(&next,&current.sid,previous,current.settings.as_ref(),None);
        // A conta ChatGPT fixa do motor: sem rota calculada aqui, é o sidecar que a guarda até o acordar.
        if let Account::Engine { account:Some(account),.. }=&next.account {meta["engine_account"]=json!(account);}
        let since=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0,|d|d.as_secs_f64());
        meta["asleep"]=json!({"wake_surface":"terminal","since":since});
        // Fora da fotografia antes de fechar: um tique no meio a daria por sumida, e depois de um reboot ela
        // acorda pela mensagem, não pelo restore. Se a sessão ficar, ela volta à fotografia em até dois tiques.
        {let owned=name.to_owned();let _=tokio::task::spawn_blocking(move||crate::session_open::restore::forget(&home,&owned)).await;}
        match self.stop_claude(name,&current,Surface::Headless).await {
            Ok(())=>{},
            // Rolando o histórico do pane: alguém está olhando.
            Err(Stop::Before(error)) if error.code=="lifecycle_panel"=>return Ok(false),
            Err(Stop::Before(error))=>return Err(error),
            Err(Stop::Alive(error))=>{
                super::actor::mark_live_error(&self.live_channel(name),&error.code,&error.message);
                return Err(error);
            },
            Err(Stop::After(error))=>return Err(self.restore_claude(name,&current,error).await.error),
        }
        if let Some(program)=&mux {
            // O shell do painel (`term-<nome>`) sai junto, só com a marca de que é nosso.
            let shell=crate::state::capture::MuxProcess::new(program,crate::state::capture::TIMEOUT);
            let hidden=format!("term-{name}");
            if shell.is_hidden(&hidden).await && shell.kill_session(&hidden).await!=Some(true) {
                tracing::warn!(session=%name,"shell escondido do Claude adormecido não fechou");
            }
        }
        let (dir,owned,meta)=(self.claude_sidecars(),name.to_owned(),meta.as_object().cloned().unwrap_or_default());
        let written=tokio::task::spawn_blocking(move||super::codex_sidecar::put(&dir,&owned,&meta)).await;
        if !matches!(written,Ok(Ok(()))) {
            // Sem o arquivo a sessão sumiria da lista sem processo: volta como estava.
            return Err(self.restore_claude(name,&current,refused("lifecycle_sidecar_io","o arquivo da sessão adormecida não foi gravado")).await.error);
        }
        tracing::info!(session=%name,idle_s=idle.as_secs(),"sessão com terminal adormecida");
        let event=json!({"evento":"sessao.adormecida","nivel":"ok","sessao":name,"provider":"claude","parada_s":idle.as_secs()});
        crate::diag::record(crate::diag::diary_dir_in(&self.codex_environment()),vec![event],"adormecer");
        Ok(true)
    }

    /// Acorda a sessão Claude adormecida `name` na superfície da marca, pelo spec, como o estacionamento.
    /// `Ok(false)`: a marca já tinha saído.
    pub(super) async fn wake_claude(&self,name:&str,surface:Surface) -> Result<bool,RuntimeError> {
        let change=SessionChange { reason:ChangeReason::ParkWake,surface:Some(surface),account:None,permission_mode:None,model:None,effort:None,service_tier:None };
        self.reopen_by_spec(name,change,None,None).await.map(|reopened|reopened.changed).map_err(|failure|failure.error)
    }

    async fn reopen_by_spec(&self,name:&str,change:SessionChange,between:Option<&dyn Between>,resume:Option<&str>) -> Result<Reopened,ReopenFailure> {
        self.reopen_checked(name, change, between, resume, None, None).await
    }

    pub(crate) async fn reopen_claude_provider(&self, name: &str, source: &hangar_api::session::ClaudeProviderSource, change: SessionChange) -> Option<Result<Reopened, ReopenFailure>> {
        if let Err(error) = self.claude_spec(name).await? { return Some(Err(ReopenFailure::refused(error))); }
        Some(self.reopen_checked(name, change, None, None, Some(source), None).await)
    }

    pub(crate) async fn claude_engine_source(&self, name: &str, expected: &SessionSpec) -> Result<EngineReopenSource, ReopenFailure> {
        let current = self.claude_current(name, BusyCheck::Skip, false).await.map_err(ReopenFailure::refused)?;
        if !same_engine_spec(expected, &current.spec) { return Err(ReopenFailure::refused(engine_source_changed())); }
        Ok(EngineReopenSource { spec: current.spec, sid: current.sid, generation: current.generation })
    }

    pub(crate) async fn reopen_claude_engine(&self, name: &str, source: &EngineReopenSource, change: SessionChange) -> Result<Reopened, ReopenFailure> {
        self.reopen_checked(name, change, None, None, None, Some(source)).await
    }

    async fn reopen_checked(&self,name:&str,change:SessionChange,between:Option<&dyn Between>,resume:Option<&str>,provider_source:Option<&hangar_api::session::ClaudeProviderSource>,engine_source:Option<&EngineReopenSource>) -> Result<Reopened,ReopenFailure> {
        let transfer=|| {
            // A troca de agente à espera também conta: a vida nova do Claude levaria a fila que ela segurou para o Codex.
            let open=self.handoff_held(name) || super::codex_relaunch::transfer_open(&self.codex_environment().home,name,None,None);
            if open {Err(ReopenFailure::refused(RuntimeError::new("session_transfer_busy","a sessão está trocando de agente")))} else {Ok(())}
        };
        transfer()?;
        let _ingress = if provider_source.is_some() {
            super::lifecycle::ClosedIngress::acquire_provider_switch(self.ingress(), name).await
        } else {
            super::lifecycle::ClosedIngress::acquire_unless_transfer(self.ingress(), name).await
        }.map_err(ReopenFailure::refused)?;
        // A transferência que começou enquanto esta esperava a porta.
        transfer()?;
        // Conferido de novo sob a porta: outra troca pode ter terminado enquanto esta esperava.
        let meta=self.claude_sidecar(name).await.map_err(ReopenFailure::refused)?;
        // Acordar a sessão que outra reabertura já acordou seria um segundo Claude na mesma conversa.
        if change.reason==ChangeReason::ParkWake && meta.as_ref().and_then(asleep_of).is_none() {
            return Ok(Reopened { spec:None,changed:false,warnings:Vec::new() });
        }
        let now=surface_now(meta.as_ref());
        if change.reason==ChangeReason::ModeSwitch && now.is_some() && change.surface==now {
            return Ok(Reopened { spec:None,changed:false,warnings:Vec::new() });
        }
        // A porta fechada não exclui outro fechador: quem reabre a adormecida, ou chega com o acordar em curso,
        // reserva a vaga de acordar, e o `ParkWake` só entra pela tarefa que já a tem.
        let marked=meta.as_ref().filter(|meta|asleep_of(meta).is_some()).and_then(|meta|meta["key"].as_str()).map(str::to_owned);
        let _slot=match (change.reason,marked) {
            (ChangeReason::ParkWake,_) if self.waking_key(name).is_some()=>None,
            (ChangeReason::ParkWake,_)=>return Err(ReopenFailure::refused(busy("erro_sessao_iniciando"))),
            (_,Some(key))=>Some(self.reserve_waking(name,&key).ok_or_else(||ReopenFailure::refused(busy("erro_sessao_iniciando")))?),
            (_,None) if self.waking_key(name).is_some()=>return Err(ReopenFailure::refused(busy("erro_sessao_iniciando"))),
            (_,None)=>None,
        };
        // O acordar vem de uma mensagem: a fila esperando é justamente o que a vida nova entrega.
        let check=match change.reason {
            ChangeReason::Reload=>BusyCheck::RefuseUnlessStalled,
            ChangeReason::ParkWake=>BusyCheck::Skip,
            _=>BusyCheck::Refuse,
        };
        let mut current=self.claude_current(name,check,resume.is_some()).await.map_err(ReopenFailure::refused)?;
        if let Some(source) = provider_source {
            self.check_provider_source(&current, source, meta.as_ref()).map_err(ReopenFailure::refused)?;
        }
        if engine_source.is_some_and(|source| !source.matches(&current)) {
            return Err(ReopenFailure::refused(engine_source_changed()));
        }
        // A vida nova só entrega o que tem até 15 min antes de nascer: o texto guardado no sono vem com ela.
        if let Some(meta)=meta.as_ref().filter(|meta|asleep_of(meta).is_some()) && let Some(key)=meta["key"].as_str() {
            self.restamp_asleep(key,meta["asleep"]["since"].as_f64().unwrap_or(0.0),false).await.map_err(ReopenFailure::refused)?;
        }
        let mut archived_binding = None;
        // Outra conversa leva as escolhas e a identidade registradas para ela.
        if let Some(sid)=resume.filter(|sid|*sid!=current.sid) {
            let (home,sid)=(self.codex_environment().home,sid.to_owned());
            let (settings, archived)=tokio::task::spawn_blocking(move||(
                crate::claude_customizations::stored(&home,&sid),
                crate::session_open::record::archived_claude(&home,&sid),
            )).await.map_err(|_|ReopenFailure::refused(refused("claude_customizations_read_failed","as escolhas da conversa não puderam ser lidas")))?;
            current.settings=settings.map_err(|(code,detail)|ReopenFailure::refused(refused(code,detail)))?;
            if let Some(SessionSpec { account: Account::Engine { binding, .. }, .. }) = archived { archived_binding = binding; }
        }
        let sid=resume.unwrap_or(&current.sid).to_owned();
        let mut next=current.spec.with(&change);
        next.conversation=Some(sid.clone());
        if provider_source.is_some() {
            let Account::Engine { config_dir, .. } = &mut next.account else {
                return Err(ReopenFailure::refused(refused("erro_cliproxy_conta", "Destino sem conta ChatGPT vinculada")));
            };
            *config_dir = current.spec.account.config_dir().map(str::to_owned);
            next.subagent_model = None;
            next.effort = None;
        }
        if let Some(between)=between {between.adjust(&mut next);}
        if let Some(expected) = archived_binding.as_deref()
            && !matches!(&next.account, Account::Engine { binding: Some(binding), .. } if binding == expected) {
            return Err(ReopenFailure::refused(refused("reopen_engine_account_mismatch", "A conversa de destino está vinculada a outra conta ChatGPT. Retome pelo Arquivo.")));
        }
        let (mut warnings, provider_route);
        let automatic=matches!(change.reason,ChangeReason::RebootRestore|ChangeReason::ParkWake|ChangeReason::LimitSwitch|ChangeReason::Sleep);
        let previous = (sid != current.sid).then(|| {
            let mut record = current.spec.clone();
            record.conversation = Some(current.sid.clone());
            record
        });
        {let (env,mut spec,settings,handle)=(self.codex_environment(),next,current.settings.clone(),tokio::runtime::Handle::current());
            let provider_switch = provider_source.is_some();
            (next,warnings,provider_route)=tokio::task::spawn_blocking(move|| {
                let provider_route = provider_switch.then(|| validate_provider_destination(&env, &mut spec, &handle)).transpose()?;
                let warnings = preflight(&env,&mut spec,settings.as_ref(),handle,automatic)?;
                if let Some(previous) = &previous {
                    crate::session_open::record::confirm_binding(&env.home, previous)
                        .map_err(|_| refused("reopen_engine_binding_write", "Não consegui guardar a conta da conversa atual no Arquivo; nada foi reaberto."))?;
                }
                Ok((spec,warnings,provider_route))
            }).await.map_err(|_|ReopenFailure::refused(refused("reopen_preflight","a subida nova não pôde ser conferida")))?.map_err(ReopenFailure::refused)?;}
        // O único aviso do pré-voo é o motor que sumiu.
        if !warnings.is_empty() {self.diag.report("runtime.reopen_engine_gone",name,"reopen_engine_gone","o motor saiu do catálogo e a sessão volta na conta Anthropic");}
        warnings.extend(reconcile_account(&self.codex_environment(),next.account.config_dir(),&next.cwd).await.map_err(ReopenFailure::refused)?);
        if let Some(between)=between {between.check(&current.spec,&next,&current.sid).await.map_err(ReopenFailure::refused)?;}
        let start=self.prepare_start(name,&next,&current,&sid,provider_route.as_ref()).await.map_err(ReopenFailure::refused)?;
        if let Some(source) = provider_source {
            let last = self.claude_current(name, BusyCheck::Refuse, false).await.map_err(ReopenFailure::refused)?;
            if self.panel_open(name) { return Err(ReopenFailure::refused(busy("terminal_panel_active"))); }
            self.check_provider_source(&last, source, meta.as_ref()).map_err(ReopenFailure::refused)?;
            if last.generation != current.generation || last.sid != current.sid || last.spec.account != current.spec.account {
                return Err(ReopenFailure::refused(refused("session_source_changed", "A conversa mudou enquanto o destino era conferido.")));
            }
        }
        if let Some(source) = engine_source {
            let last = self.claude_current(name, BusyCheck::Refuse, false).await.map_err(ReopenFailure::refused)?;
            if !source.matches(&last) { return Err(ReopenFailure::refused(engine_source_changed())); }
        }
        // Quando o transcript é movido, a recuperação pode seguir a pasta que o recebeu.
        let _switching = if between.is_some() {
            SwitchMark::write(&self.codex_environment().home, &next).map_err(ReopenFailure::refused)?
        } else { None };
        let mut provider_mark = if provider_source.is_some() {
            // A geração avançada prova a saída da origem; sem ela a marca só permite recuperar a origem.
            Some(SwitchMark::provider(&self.codex_environment().home, &current, &next).map_err(|error|
                ReopenFailure::refused(refused("reopen_switch_mark", &format!("a troca não pôde ser registrada ({error})"))))?)
        } else { None };
        match self.stop_claude(name,&current,next.surface).await {
            Ok(())=>{},
            Err(Stop::Before(error))=>{
                if let Some(mark) = provider_mark.as_mut() { mark.keep = false; }
                return Err(ReopenFailure::refused(error));
            },
            Err(Stop::Alive(error))=>{
                if let Some(mark) = provider_mark.as_mut() { mark.keep = false; }
                super::actor::mark_live_error(&self.live_channel(name),&error.code,&error.message);
                return Err(ReopenFailure { error,stopped:true,landed:None });
            },
            Err(Stop::After(error))=>return Err(self.restore_before_provider_switch(name, &current, provider_mark, error).await),
        }
        // Sem terminal, o processo antigo saiu sem gravar o que recebeu: a vida nova responde.
        if resume.is_none() && matches!(current.life,Life::Headless{..}) {
            let transcript=super::claude_launch::transcript(&self.codex_environment(),&current.spec,&current.sid);
            if let Err(error)=self.requeue_unanswered(&current.spec.key,transcript).await {
                tracing::warn!(session=%name,code=%error.code,"mensagens sem confirmação ficaram como estavam");
            }
        }
        if let Some(between)=between && let Err(error)=between.apply(&current.spec,&next,&current.sid).await {
            return Err(self.restore_claude(name,&current,error).await);
        }
        match self.start_claude(name,&next,start,&sid).await {
            Ok(())=>{
                let mut record_failed = false;
                if crate::session_open::record::path(&self.codex_environment().home,&next.key).is_ok() && let Err(error)=crate::session_open::record::write(&self.codex_environment().home,&next) {
                    record_failed = true;
                    // A sessão nova está de pé, mas o restore e a próxima troca leriam o registro velho.
                    tracing::warn!(session=%name,kind=?error.kind(),"spec da reabertura não gravado");
                    self.diag.report("runtime.reopen_spec_unwritten",name,"reopen_spec_unwritten","a sessão reabriu e o registro dela não foi gravado");
                    warnings.push("A sessão reabriu, mas o registro dela não foi gravado: o restore e a próxima reabertura podem voltar como antes.".into());
                }
                if let Err(error) = crate::session_open::record::confirm_binding(&self.codex_environment().home, &next) {
                    record_failed = true;
                    tracing::warn!(session=%name,kind=?error.kind(),"vínculo da conversa reaberta não gravado no Arquivo");
                    warnings.push("A sessão reabriu, mas o vínculo da conversa não foi gravado no Arquivo.".into());
                }
                if provider_source.is_some() && (record_failed || provider_mark.take().is_some_and(|mark| mark.clear().is_err())) {
                    return Err(ReopenFailure { error: refused("reopen_engine_binding_write", "O Claude já reabriu no destino, mas não consegui finalizar o registro da troca. Confira a conta da conversa antes de tentar novamente."), stopped: true, landed: Some(next.surface) });
                }
                Ok(Reopened { spec:Some(next),changed:true,warnings })
            },
            // A vida nova que não saiu não divide a conversa com a de antes restaurada.
            Err(error) if error.code==NEW_ALIVE=>{
                super::actor::mark_live_error(&self.live_channel(name),&error.code,&error.message);
                Err(ReopenFailure { error,stopped:true,landed:None })
            },
            Err(error)=>{
                // A conversa que não voltou para a conta de antes não é reaberta lá: seria outra conversa.
                if let Some(between)=between && let Err(undo)=between.undo(&current.spec,&next,&current.sid).await {
                    let error=RuntimeError::new(&error.code,&format!("{}; {}",error.message,undo.message));
                    super::actor::mark_live_error(&self.live_channel(name),&error.code,&error.message);
                    return Err(ReopenFailure { error,stopped:true,landed:None });
                }
                Err(self.restore_before_provider_switch(name, &current, provider_mark, error).await)
            },
        }
    }

    fn check_provider_source(&self, current: &Current, source: &hangar_api::session::ClaudeProviderSource, meta: Option<&Value>) -> Result<(), RuntimeError> {
        let changed = || refused("session_source_changed", "A conversa ou a conta de origem mudou. Abra o seletor novamente.");
        if current.spec.read_only || meta.and_then(asleep_of).is_some() || crate::session_open::record::path(&self.codex_environment().home, &current.spec.key).is_err() {
            return Err(refused("erro_troca_conta", "Só é possível trocar o provedor de uma conversa Claude aberta e editável."));
        }
        let (engine, account, binding) = match &current.spec.account {
            Account::Engine { name, account, binding, .. } => (Some(name.as_str()), account.as_deref(), binding.as_deref()),
            Account::Claude { .. } | Account::Default => (None, None, None),
            _ => return Err(changed()),
        };
        if source.headless != matches!(&current.life, Life::Headless { .. }) || engine != source.engine.as_deref()
            || account != source.engine_account.as_deref() || binding != source.engine_binding.as_deref() {
            return Err(changed());
        }
        let (transcript, terminal_life) = match &current.life {
            Life::Terminal { target } => (PathBuf::from(&target.transcript), Some(format!("t:{}", target.binding.created))),
            Life::Headless { .. } => (super::claude_launch::transcript(&self.codex_environment(), &current.spec, &current.sid), None),
            Life::Bare { .. } => return Err(changed()),
        };
        if source.lifecycle_id != format!("k:{}", current.spec.key) && terminal_life.as_deref() != Some(source.lifecycle_id.as_str()) {
            return Err(changed());
        }
        let expected = std::path::Path::new(&source.jsonl);
        if !expected.is_absolute() || crate::list::links::resolve_lenient(expected) != crate::list::links::resolve_lenient(&transcript) {
            return Err(changed());
        }
        Ok(())
    }

    /// A sessão, o spec dela e se ela está parada. Ocupada recusa com o motivo, antes de qualquer efeito.
    async fn claude_current(&self,name:&str,check:BusyCheck,bare:bool) -> Result<Current,RuntimeError> {
        let (env,checks)=(self.codex_environment(),check!=BusyCheck::Skip);
        if let Some(meta)=self.claude_sidecar(name).await? {
            let mut spec=crate::session_open::record::claude_live(&env.home,&meta,name).map_err(|e|refused("launch_meta",&format!("registro da sessão ilegível: {e}")))?;
            // A adormecida que era com terminal volta com ele: `/conta` e `/recarregar` nela reabrem o terminal.
            if let Some(surface)=asleep_of(&meta) {spec.surface=surface;}
            let sid=meta["session_id"].as_str().filter(|s|!s.is_empty()).ok_or_else(||refused("lifecycle_conversation","sessão sem conversa"))?.to_owned();
            let open=self.claude_headless_view(name).await;
            let (generation,view)=match open {
                Some((generation,view))=>(generation,Some(view)),
                None=>(self.queue_generation(&spec.key).await?,None),
            };
            // Reabrir com a entrega parada (aviso do próprio ator) é a saída da mensagem presa: o turno, a
            // subida e o cartão congelados na queda (inalcançáveis com o cano surdo) saem com a vida velha, e a
            // fila vai para a nova, que entrega o que estiver esperando.
            let stalled=check==BusyCheck::RefuseUnlessStalled && view.as_ref().is_some_and(|view|view["public_state"]["problema"]==super::actor::DELIVERY_STALLED);
            let checks_busy=checks && !stalled;
            if let Some(view)=&view {
                if checks_busy && view["alive"]==true && view["initialized"]!=true {return Err(busy("erro_sessao_iniciando"));}
                if checks_busy && (!view["question"].is_null() || view["pending"].as_array().is_some_and(|p|!p.is_empty())) {return Err(busy("erro_sessao_esperando_resposta"));}
                if checks_busy && view["in_progress"]==true {return Err(busy("erro_sessao_trabalhando"));}
                // O modo e as escolhas que a CLI mudou sozinha valem; com motor, o do sidecar.
                if let Some(mode)=view["permission_mode"].as_str().and_then(PermissionMode::claude) {spec.permission_mode=Some(mode);}
                if !matches!(spec.account,Account::Engine{..}) {
                    if let Some(model)=view["model"].as_str().filter(|m|!m.is_empty()) {spec.model=Some(model.into());}
                    if let Some(effort)=view["effort"].as_str().filter(|e|EFFORTS.contains(e)) {spec.effort=Some(effort.into());}
                }
            }
            // Na troca por limite ou de agente a fila foi segurada para ir com a conversa ao destino novo.
            if checks_busy && !self.queue_held(name) && self.queue_pending(&spec.key).await {return Err(busy("erro_fila_pendente"));}
            if spec.permission_mode.is_none() {
                spec.permission_mode=Some(super::claude_launch::account_mode(&env,spec.account.config_dir())?);
            }
            let previous_non_plan=meta["previous_non_plan"].as_str().map(str::to_owned);
            let settings=meta.get("claude_settings").filter(|s|s.as_object().is_some_and(|o|!o.is_empty())).cloned();
            return Ok(Current { spec,sid,generation,life:Life::Headless { meta,open:view.is_some() },previous_non_plan,settings });
        }
        let Some((target,handle))=self.claude_terminal_entry(name).await else {
            if bare {return self.bare_current(name).await;}
            return Err(refused("runtime_binding","a sessão não está aberta aqui"));
        };
        let cwd={let procs=SystemProcs::default();procs.cwd(i64::from(target.metadata.agent_pid)).map(|p|p.to_string_lossy().into_owned())};
        let recorded=if crate::session_open::record::path(&env.home,&target.key).is_ok() {
            crate::session_open::record::read(&env.home,&target.key).map_err(|e|refused("session_spec_read",&format!("registro da sessão ilegível: {e}")))?
        } else {None};
        let (mut spec,settings)=match recorded {
            Some(spec)=>{
                let settings=SystemProcs::default().env_var(i64::from(target.metadata.agent_pid),"HANGAR_CLAUDE_SETTINGS").ok().flatten()
                    .and_then(|raw|serde_json::from_str::<Value>(&raw.to_string_lossy()).ok()).filter(|v|v.as_object().is_some_and(|o|!o.is_empty()));
                (spec,settings)
            },
            None=>legacy_terminal_spec(&target,name,cwd.clone().ok_or_else(||refused("lifecycle_process_read","a pasta do Claude não pôde ser lida"))?)?,
        };
        spec.name=name.into();
        spec.surface=Surface::Terminal;
        if !matches!(spec.account,Account::Engine{..}) && let Ok(Some(choices))=super::lifecycle::terminal_choices(&target) {
            if let Some(model)=choices.model.filter(|m|!m.is_empty()) {spec.model=Some(model);}
            // `ultracode` só existe no `/effort` em voo: a linha de comando não o aceita.
            if let Some(effort)=choices.effort.filter(|e|EFFORTS.contains(&e.as_str())) {spec.effort=Some(effort);}
        }
        let observed=self.permission_view(&target.binding.conversation);
        if let Some(mode)=observed.as_ref().and_then(|view|PermissionMode::claude(&view.current)) {spec.permission_mode=Some(mode);}
        if checks && spec.permission_mode.is_none() {
            return Err(refused("permission_mode_unknown","o modo de permissão da sessão não é conhecido; nada foi reaberto"));
        }
        // Parada, sem pergunta, sem fila e sem entrega incerta: os motivos do `_motivo_ocupada`.
        if checks {handle.control(format!("guard:reopen:{}",crate::mods::state::random_hex(8)),"lifecycle_guard".into(),json!({"queued_ok":self.queue_held(name)})).await?;}
        Ok(Current { sid:target.binding.conversation.clone(),generation:target.generation,previous_non_plan:observed.map(|v|v.previous_non_plan),
            settings,spec,life:Life::Terminal { target } })
    }

    /// O pane Claude sem conversa provada: o spec gravado pela chave dele, senão o lido do processo; o modo
    /// desconhecido é o da conta, que é onde a CLI aberta sem `--permission-mode` estava. A conversa de agora
    /// não existe para a reabertura: a volta, se a nova falhar, é numa conversa nova na mesma sessão.
    async fn bare_current(&self,name:&str) -> Result<Current,RuntimeError> {
        let env=self.codex_environment();
        let watcher=self.watcher_ref().ok_or_else(||refused("runtime_binding","sem a vigia do terminal"))?;
        let bare=watcher.bare_claude(name).await?.ok_or_else(||refused("runtime_binding","a sessão não roda um Claude"))?;
        let recorded=if crate::session_open::record::path(&env.home,&bare.key).is_ok() {
            crate::session_open::record::read(&env.home,&bare.key).map_err(|e|refused("session_spec_read",&format!("registro da sessão ilegível: {e}")))?
        } else {None};
        let settings_of=|pid:i64|SystemProcs::default().env_var(pid,"HANGAR_CLAUDE_SETTINGS").ok().flatten()
            .and_then(|raw|serde_json::from_str::<Value>(&raw.to_string_lossy()).ok()).filter(|v|v.as_object().is_some_and(|o|!o.is_empty()));
        let (mut spec,settings)=match recorded {
            Some(spec)=>(spec,settings_of(bare.agent)),
            None=>process_spec(bare.agent,&bare.key,name,bare.cwd.clone(),bare.created)?,
        };
        spec.name=name.into();
        spec.surface=Surface::Terminal;
        if spec.permission_mode.is_none() {spec.permission_mode=Some(super::claude_launch::account_mode(&env,spec.account.config_dir())?);}
        let generation=self.queue_generation(&spec.key).await?;
        let sid=crate::session_open::open::uuid4(&crate::mods::state::random_hex(16));
        Ok(Current { spec,sid,generation,life:Life::Bare { pane_pid:bare.pane_pid },previous_non_plan:None,settings })
    }

    /// O comando da vida nova, com tudo que pode recusar antes de encerrar a antiga.
    async fn prepare_start(&self,name:&str,next:&SessionSpec,current:&Current,sid:&str,expected_provider:Option<&crate::session_open::claude::EngineRoute>) -> Result<Start,RuntimeError> {
        let env=self.codex_environment();
        let mode=next.permission_mode.ok_or_else(||refused("permission_mode_unknown","o modo de permissão da sessão não é conhecido; nada foi reaberto"))?;
        if mode==PermissionMode::BypassPermissions {
            let config=next.account.config_dir().map_or_else(||env.claude_base.clone(),PathBuf::from);
            bypass_policy(&config,std::path::Path::new(&next.cwd),current.settings.as_ref(),&env.base)?;
        }
        // A base do plano: a guardada; sem ela, a do transcript da conversa e, por último, o modo da conta.
        let previous_non_plan=if mode==PermissionMode::Plan {
            let remembered=current.previous_non_plan.as_deref().and_then(PermissionMode::claude);
            let transcript=||[&current.spec,next].into_iter().map(|spec|super::claude_launch::transcript(&env,spec,sid)).find_map(|path|
                crate::terminal_facts::transcript_mode(&path,&std::sync::Mutex::new(Default::default())).ok().flatten()
                    .as_deref().and_then(PermissionMode::claude));
            match remembered.or_else(transcript) {
                Some(mode)=>Some(mode),
                None=>Some(super::claude_launch::account_mode(&env,current.spec.account.config_dir())?),
            }
        } else {None};
        let route=match &next.account {
            Account::Engine { name:engine,account,binding,.. }=>crate::session_open::facts::session_engine_route(
                &crate::accounts::AccountService::new(env.clone()),engine,account.as_deref(),binding.as_deref()).map_err(|e|refused("launch_engine_account",&e.msg))?,
            _=>None,
        };
        if expected_provider.is_some() && route.as_ref() != expected_provider {
            return Err(refused("erro_cliproxy_conta", "O provedor mudou depois da conferência do catálogo. Escolha novamente."));
        }
        if matches!(next.account,Account::Engine{..}) {
            if super::process::resolve_program("hangar-engine",&env.base).is_none() {
                return Err(refused("launch_engine_missing","hangar-engine não está no PATH deste servidor; sem ele a sessão com motor morre ao nascer"));
            }
        }
        match next.surface {
            Surface::Headless=>{
                if next.read_only {return Err(refused("headless_read_only",super::claude_launch::READ_ONLY_MSG));}
                let mut meta=crate::session_open::open::headless_sidecar(next,sid,previous_non_plan,current.settings.as_ref(),route.as_ref());
                if let Life::Headless { meta:old,.. }=&current.life {
                    // O que só o sidecar guarda (o `created`, o `problema` da conta) segue com a sessão.
                    // O aviso da conta anterior (limite batido, sem login) não vale noutra conta.
                    let fields:&[&str]=if next.account==current.spec.account {&["created","problema"]} else {&["created"]};
                    for field in fields { if let Some(value)=old.get(*field) {meta[*field]=value.clone();} }
                }
                // Confere a subida inteira (conta, motor, modo, Fast) antes de encerrar a vida antiga.
                let (bridge,owned,handle)=(self.policy_plugin_bridge(),name.to_owned(),tokio::runtime::Handle::current());
                let (check,env)=(meta.clone(),env.clone());
                let checked=tokio::task::spawn_blocking(move||super::claude_launch::prepare(&env,bridge.as_ref(),&check,&owned,handle)).await
                    .map_err(|_|refused("relaunch_recipe","a subida nova não pôde ser preparada"))?;
                checked.map(|_|Start::Headless(meta))
            },
            Surface::Terminal=>{
                let read_only=if next.read_only {
                    let run=|argv:Vec<String>,timeout:Duration|->futures_util::future::BoxFuture<'_,crate::session_open::terminal::MuxOutput> {
                        let base=env.base.clone();Box::pin(async move {crate::session_open::server::run_program(&base,argv,timeout).await})
                    };
                    Some(crate::session_open::open::read_only_envelope(&env.base,&env.home,&next.cwd,next.account.config_dir(),&run).await
                        .map_err(|e|refused("reopen_read_only",&e.msg))?)
                } else {None};
                let watcher=self.watcher_ref().ok_or_else(||refused("runtime_binding","sem a vigia do terminal"))?;
                let mut terminal={let (home,base)=(env.home.clone(),env.base.clone());tokio::task::spawn_blocking(move||TerminalEnv::local(&home,&base)).await
                    .map_err(|_|refused("reopen_terminal_env","o ambiente do terminal não pôde ser lido"))?};
                terminal.program=watcher.mux_program();
                // A pasta não confiada prende o terminal novo no "trust this folder?", e a reabertura sairia
                // pela confirmação, com a causa errada.
                crate::session_open::claude::pretrust_async(crate::session_open::claude::claude_json(&terminal,&env.home,next.account.config_dir()),next.cwd.clone(),terminal.windows).await
                    .map_err(|e|refused("reopen_pretrust_failed",&format!("a pasta não pôde ser confiada na conta ({e}); nada foi reaberto")))?;
                let runtime=super::codex_launch::runtime_config(&env);
                let roots=crate::session_open::claude::plugin_roots(&env.home);
                let bridge=self.policy_plugin_bridge();
                let key=crate::mods::state::random_hex(16);
                // A troca de conta só leva a conversa depois de encerrar: a de agora diz se ela já existe.
                let resume=super::claude_launch::transcript(&env,next,sid).exists() || super::claude_launch::transcript(&env,&current.spec,sid).exists();
                let settings_file=match current.settings.as_ref().filter(|s|terminal.windows && s.as_object().is_some_and(|o|!o.is_empty())) {
                    Some(settings)=>Some(crate::session_open::claude::settings_file(&env.home,settings)
                        .map_err(|e|refused("reopen_settings_file",&format!("as escolhas da conversa não foram gravadas ({e}); nada foi reaberto")))?),
                    None=>None,
                };
                let ctx=ClaudeContext { plugin_roots:&roots,bridge:bridge.as_ref(),bridge_key:&key,runtime_config:&runtime,server_env:&env.base,
                    settings:current.settings.as_ref(),settings_file:settings_file.as_deref(),read_only:read_only.as_deref(),windows:terminal.windows,initial_prompt:None,engine_route:route.as_ref(),
                    resume,allow_bypass:previous_non_plan==Some(PermissionMode::BypassPermissions) };
                Ok(Start::Terminal(Box::new(crate::session_open::claude::pane(next,sid,&ctx)),terminal))
            },
        }
    }

    /// Encerra a vida que roda e confirma a saída da árvore inteira; depois avança a geração da fila.
    async fn stop_claude(&self,name:&str,current:&Current,to:Surface) -> Result<(),Stop> {
        let key=current.spec.key.clone();
        let before=Stop::Before;
        match &current.life {
            Life::Terminal { target }=>{
                let io=crate::terminal_input::ProcessIo::default();
                let pane=super::lifecycle::pane(&io,&target.binding).await.map_err(before)?;
                if pane.copy {return Err(Stop::Before(refused("lifecycle_panel","o terminal está com um painel aberto")));}
                let lives=tree(pane.pid).map_err(before)?;
                let (program,prefix)=target.binding.mux_argv.split_first().ok_or_else(||Stop::Before(refused("lifecycle_mux","sem o multiplexador")))?;
                self.close_claude_entry(&key,current.generation).await.map_err(before)?;
                let session=crate::session_open::terminal::target(target.binding.windows,name);
                let mut command=crate::terminal_input::child_command(program);
                command.args(prefix).args(["kill-session","-t",&session]).stdin(std::process::Stdio::null()).kill_on_drop(true);
                let killed=tokio::time::timeout(Duration::from_secs(5),command.output()).await.ok().and_then(Result::ok).is_some_and(|o|o.status.success());
                if !killed {return Err(Stop::Before(refused("reopen_kill_mux","o terminal não fechou; a sessão não reabriu")));}
                stop_tree(lives).await.map_err(Stop::Alive)?;
            },
            Life::Bare { pane_pid }=>{
                let lives=tree(*pane_pid).map_err(before)?;
                let watcher=self.watcher_ref().ok_or_else(||Stop::Before(refused("runtime_binding","sem a vigia do terminal")))?;
                let session=crate::session_open::terminal::target(cfg!(windows),name);
                if watcher.mux_text(&["kill-session","-t",&session]).await.is_none() {
                    return Err(Stop::Before(refused("reopen_kill_mux","o terminal não fechou; a sessão não reabriu")));
                }
                stop_tree(lives).await.map_err(Stop::Alive)?;
            },
            Life::Headless { meta,open }=>{
                let cano=meta.get("cano").filter(|c|c.is_object()).and_then(|c|serde_json::from_value::<super::process::Cano>(c.clone()).ok());
                let lives=match &cano {Some(cano)=>tree(i64::from(cano.pid)).map_err(before)?,None=>Vec::new()};
                if *open {self.close_claude_entry(&key,current.generation).await.map_err(before)?;}
                if let Some(cano)=cano {
                    // `kill` que falha pode ter sinalizado o grupo: a árvore que sobrou decide.
                    if let Err(error)=super::process::kill(&cano,&key,&self.claude_sidecars()).await {
                        stop_tree(lives).await.map_err(|_|Stop::Alive(RuntimeError::new(error.code(),"o processo antigo não foi encerrado")))?;
                    } else {
                        stop_tree(lives).await.map_err(Stop::Alive)?;
                    }
                }
                // O sidecar sai: nada sobe o processo sem terminal enquanto o pane nasce. Sem terminal de
                // novo, a vida nova grava o dela.
                if to==Surface::Terminal {self.remove_claude_sidecar(name).await.map_err(Stop::After)?;}
            },
        }
        self.advance_generation(name,&key,current.generation).await.map_err(Stop::After)
    }

    /// Dá a partida na vida nova e confirma: o Claude na conversa e no modo pedidos.
    async fn start_claude(&self,name:&str,next:&SessionSpec,start:Start,sid:&str) -> Result<(),RuntimeError> {
        let env=self.codex_environment();
        let mode=next.permission_mode.map(PermissionMode::name).unwrap_or_default();
        match start {
            Start::Terminal(pane,terminal)=>{
                let watcher=self.watcher_ref().ok_or_else(||refused("runtime_binding","sem a vigia do terminal"))?;
                if let Err(error)=crate::session_open::terminal::spawn(&watcher.spawn_mux(),&terminal,&pane).await {
                    self.drop_failed_terminal(name).await?;
                    return Err(refused("reopen_terminal_spawn",&format!("o terminal não abriu: {}",error.stderr)));
                }
                let io=crate::terminal_input::ProcessIo::default();
                let deadline=tokio::time::Instant::now()+STARTUP_WAIT;
                loop {
                    if let Some(Ok(()))=watcher.adopt_claude(self,name).await
                        && let Some((target,_))=self.claude_terminal_entry(name).await
                        && target.binding.conversation==sid
                        && super::lifecycle::permission_mode(&io,&target.binding).await.is_ok_and(|shown|shown==mode) {
                        let pid=i64::from(target.metadata.agent_pid);
                        let running=process_spec(pid,&next.key,name,next.cwd.clone(),0.0).map(|(spec,_)|identity(&env,&spec));
                        if running.as_ref().is_ok_and(|running|*running==identity(&env,next)) {return Ok(());}
                        if running.is_ok() {
                            self.drop_failed_terminal(name).await?;
                            return Err(refused("reopen_identity","o Claude reabriu com uma identidade diferente da escolhida (conta, motor ou Fast)"));
                        }
                    }
                    if tokio::time::Instant::now()>=deadline {
                        self.drop_failed_terminal(name).await?;
                        return Err(refused("relaunch_confirmation","o Claude não voltou no terminal na conversa e no modo pedidos em 5 minutos"));
                    }
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
            },
            Start::Headless(meta)=>{
                let dir=self.claude_sidecars();
                {let (dir,owned,meta)=(dir.clone(),name.to_owned(),meta.as_object().cloned().unwrap_or_default());
                    tokio::task::spawn_blocking(move||super::codex_sidecar::put(&dir,&owned,&meta)).await
                        .map_err(|_|refused("lifecycle_sidecar_io","o arquivo da sessão não foi gravado"))?
                        .map_err(|_|refused("lifecycle_sidecar_io","o arquivo da sessão não foi gravado"))?;}
                let opened=self.open_claude_headless(name,true).await.unwrap_or_else(||Err(refused("runtime_binding","a sessão sem terminal sumiu")));
                if let Err(error)=opened {
                    self.drop_failed_headless(name,&next.key).await?;
                    return Err(error);
                }
                let deadline=tokio::time::Instant::now()+STARTUP_WAIT;
                let why=loop {
                    if let Some((_,view))=self.claude_headless_view(name).await {
                        if view["alive"]==false {break "o Claude sem terminal saiu antes de subir";}
                        if view["initialized"]==true && view["conversation"]==sid {
                            if view["permission_mode"]==mode {return Ok(());}
                            break "o Claude sem terminal subiu num modo diferente do pedido";
                        }
                    }
                    if tokio::time::Instant::now()>=deadline {break "o Claude sem terminal não subiu em 5 minutos; travou";}
                    tokio::time::sleep(Duration::from_millis(50)).await;
                };
                self.drop_failed_headless(name,&next.key).await?;
                Err(refused("relaunch_initialize",why))
            },
        }
    }

    async fn restore_before_provider_switch(&self, name: &str, current: &Current, mark: Option<SwitchMark>, error: RuntimeError) -> ReopenFailure {
        if let Some(mark) = mark.as_ref() && let Err(undo) = mark.restore_origin() {
            let message = format!("{}; o registro da troca não pôde ser desfeito ({undo}); a origem não foi reaberta", error.message);
            super::actor::mark_live_error(&self.live_channel(name), &error.code, &message);
            return ReopenFailure { error: refused(&error.code, &message), stopped: true, landed: None };
        }
        let restored = self.restore_claude(name, current, error).await;
        if restored.landed.is_some() && let Some(mark) = mark && let Err(error) = mark.clear() {
            tracing::warn!(session=%name,kind=?error.kind(),"marca da troca restaurada não pôde ser removida");
        }
        restored
    }

    /// Falhou depois de encerrar: a sessão volta pelo spec de antes; sem terminal, se nem o terminal de antes
    /// voltar. O erro diz onde ela ficou.
    async fn restore_claude(&self,name:&str,current:&Current,error:RuntimeError) -> ReopenFailure {
        // Só leitura estacionada não roda sem terminal: volta a esperar o terminal protegido, com o arquivo dela.
        if let (true,Life::Headless { meta,.. })=(current.spec.read_only,&current.life) {
            let (dir,owned,meta)=(self.claude_sidecars(),name.to_owned(),meta.as_object().cloned().unwrap_or_default());
            let parked=tokio::task::spawn_blocking(move||super::codex_sidecar::put(&dir,&owned,&meta)).await;
            let message=format!("o terminal protegido não voltou ({}); a sessão read-only ficou parada até reabrir no terminal",error.message);
            if !matches!(parked,Ok(Ok(_))) {
                let message=format!("{message}; o arquivo da sessão não foi regravado");
                super::actor::mark_live_error(&self.live_channel(name),&error.code,&message);
                return ReopenFailure { error:RuntimeError::new("erro_troca_conta_parada",&message),stopped:true,landed:None };
            }
            return ReopenFailure { error:RuntimeError::new("erro_troca_conta_parada",&message),stopped:true,landed:Some(Surface::Headless) };
        }
        // A adormecida que não acordou volta a dormir com o arquivo dela, marca incluída: nada sobe sem terminal,
        // e a próxima mensagem tenta de novo.
        if let Life::Headless { meta,.. }=&current.life && asleep_of(meta).is_some() {
            let (dir,owned,meta)=(self.claude_sidecars(),name.to_owned(),meta.as_object().cloned().unwrap_or_default());
            let asleep=tokio::task::spawn_blocking(move||super::codex_sidecar::put(&dir,&owned,&meta)).await;
            let message=if matches!(asleep,Ok(Ok(()))) {format!("{}; a sessão continua adormecida",error.message)}
                else {format!("{}; a sessão ficou sem processo e o arquivo dela não foi regravado",error.message)};
            return ReopenFailure { error:RuntimeError::new(&error.code,&message),stopped:true,landed:None };
        }
        let mut tries=vec![current.spec.clone()];
        if current.spec.surface==Surface::Terminal && !current.spec.read_only {
            let mut headless=current.spec.clone();headless.surface=Surface::Headless;tries.push(headless);
        }
        for spec in tries {
            // O sidecar da tentativa sem terminal faria a adoção do pane abrir o processo sem terminal.
            if spec.surface==Surface::Terminal && let Err(error)=self.remove_claude_sidecar(name).await {
                tracing::warn!(session=%name,code=%error.code,"sidecar da tentativa que falhou ficou");
                continue;
            }
            let back=match self.prepare_start(name,&spec,current,&current.sid,None).await {
                Ok(start)=>self.start_claude(name,&spec,start,&current.sid).await,
                Err(error)=>Err(error),
            };
            match back {
                Ok(())=>{
                    let place=if spec.surface==current.spec.surface {"a sessão voltou como estava"} else {"a sessão seguiu sem terminal"};
                    return ReopenFailure { error:RuntimeError::new(&error.code,&format!("{}; {place}",error.message)),stopped:true,landed:Some(spec.surface) };
                },
                Err(restore) if restore.code == NEW_ALIVE => {
                    let message = format!("{}; {}", error.message, restore.message);
                    super::actor::mark_live_error(&self.live_channel(name), &restore.code, &message);
                    return ReopenFailure { error: refused(&restore.code, &message), stopped:true, landed:None };
                },
                Err(restore)=>tracing::warn!(session=%name,code=%restore.code,"a sessão não voltou pelo spec de antes"),
            }
        }
        let message=format!("{}; a sessão não voltou e ficou sem processo",error.message);
        super::actor::mark_live_error(&self.live_channel(name),&error.code,&message);
        ReopenFailure { error:RuntimeError::new(&error.code,&message),stopped:true,landed:None }
    }

    async fn remove_claude_sidecar(&self,name:&str) -> Result<(),RuntimeError> {
        let (dir,owned)=(self.claude_sidecars(),name.to_owned());
        tokio::task::spawn_blocking(move||super::codex_sidecar::locked(&dir,&owned,|path|match std::fs::remove_file(path) {
            Err(error) if error.kind()!=std::io::ErrorKind::NotFound=>Err(error),_=>Ok(()) }))
            .await.map_err(|_|refused("lifecycle_sidecar_io","o arquivo da sessão não pôde ser apagado"))?
            .map_err(|_|refused("lifecycle_sidecar_io","o arquivo da sessão não pôde ser apagado"))
    }

    /// Fecha o terminal da tentativa que falhou e espera a árvore dele sair; ficou vivo = `NEW_ALIVE`.
    async fn drop_failed_terminal(&self,name:&str) -> Result<(),RuntimeError> {
        if let Some((target,_))=self.claude_terminal_entry(name).await {let _=self.close_claude_entry(&target.key,target.generation).await;}
        let watcher=self.watcher_ref().ok_or_else(||refused(NEW_ALIVE,"sem a vigia do terminal para fechar a tentativa"))?;
        let session=crate::session_open::terminal::target(cfg!(windows),name);
        let mux = watcher.spawn_mux();
        let program = watcher.mux_program();
        let pane = mux.run(vec![program.clone(), "display-message".into(), "-p".into(), "-t".into(), session.clone(), "#{pane_pid}".into()]).await;
        let Some(pid) = failed_terminal_pid(&pane)? else { return Ok(()); };
        let lives = tree(pid).map_err(|error| cleanup_uncertain(&error.message))?;
        let killed = mux.run(vec![program, "kill-session".into(), "-t".into(), session]).await;
        if killed.code != Some(0) && !(killed.code == Some(1) && crate::list::mux::is_absence(&killed.stderr)) {
            return Err(cleanup_uncertain("o multiplexador não confirmou o fechamento do terminal"));
        }
        wait_gone(lives,EXIT_WAIT).await.map_err(|error| cleanup_uncertain(&error.message))
    }

    async fn drop_failed_headless(&self,name:&str,key:&str) -> Result<(),RuntimeError> {
        if let Some((generation,_))=self.claude_headless_view(name).await {let _=self.close_claude_entry(key,generation).await;}
        let meta = self.claude_sidecar(name).await.map_err(|error| cleanup_uncertain(&error.message))?;
        let Some(cano) = meta.as_ref().and_then(|meta| meta.get("cano")).filter(|cano| !cano.is_null()) else { return Ok(()); };
        let cano = serde_json::from_value::<super::process::Cano>(cano.clone())
            .map_err(|_| cleanup_uncertain("o processo do Claude sem terminal não pôde ser identificado"))?;
        let lives = tree(i64::from(cano.pid)).map_err(|error| cleanup_uncertain(&error.message))?;
        if let Err(error)=super::process::kill(&cano,key,&self.claude_sidecars()).await {
            tracing::warn!(session=%name,code=error.code(),"cano da reabertura que falhou não foi encerrado");
        }
        wait_gone(lives,EXIT_WAIT).await.map_err(|_|refused(NEW_ALIVE,"o Claude novo não saiu; a sessão não volta por cima dele para não haver dois na mesma conversa"))
    }

    /// A geração seguinte na fila: a vida nova não casa com recibos e interações da antiga.
    async fn advance_generation(&self,name:&str,key:&str,generation:u64) -> Result<(),RuntimeError> {
        let queue=self.codex_environment().claude_base.join(".hangar-queue");
        let (state_path,lease_path)=super::codex_terminal::queue_files(&queue,key);
        let (key,name)=(key.to_owned(),name.to_owned());
        tokio::task::spawn_blocking(move||{
            let _lease=super::queue::acquire_lease_within(&lease_path,Duration::from_secs(3)).map_err(|_|refused("queue_lease","a fila da sessão está presa"))?;
            let mut store=super::queue::Store::open(&state_path,&queue,super::queue::State::new(&key,generation,&name,Vec::new())).map_err(|_|refused("queue_io","fila da sessão inacessível"))?;
            if store.state().generation!=generation {return Err(refused("runtime_generation","a vida da sessão mudou durante a reabertura"));}
            let clock=super::protocol::ClockSample {monotonic_s:0.0,epoch_s:std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0,|d|d.as_secs_f64())};
            store.exec(generation,&format!("claude-reopen:{}",crate::mods::state::random_hex(8)),clock,
                super::queue::Action::AdvanceLifecycle {generation:generation+1,state:json!({})}).map_err(|_|refused("queue_io","fila da sessão inacessível"))?;
            Ok(())
        }).await.map_err(|_|refused("queue_job","a fila da sessão não respondeu"))?
    }

    /// `_requeue_unanswered`: a entrega que o processo antigo nunca gravou volta como linha nova (a antiga,
    /// com a operação já aceita, devolveria a resposta guardada sem enviar) e a antiga é confirmada; o que já
    /// está no transcript só é confirmado. Transcript ilegível não muda nada.
    async fn requeue_unanswered(&self,key:&str,transcript:PathBuf) -> Result<usize,RuntimeError> {
        let queue=self.codex_environment().claude_base.join(".hangar-queue");
        let (state_path,lease_path)=super::codex_terminal::queue_files(&queue,key);
        let key=key.to_owned();
        tokio::task::spawn_blocking(move||{
            let committed=committed_user_lines(&transcript).map_err(|_|refused("reopen_transcript_read","o transcript da conversa não pôde ser lido"))?;
            let _lease=super::queue::acquire_lease_within(&lease_path,Duration::from_secs(3)).map_err(|_|refused("queue_lease","a fila da sessão está presa"))?;
            let mut store=match super::queue::Store::open(&state_path,&queue,super::queue::State::new(&key,1,"",Vec::new())) {
                Ok(store)=>store,
                Err(_)=>return Err(refused("queue_io","fila da sessão inacessível")),
            };
            let generation=store.state().generation;
            let rows:Vec<Value>=store.state().rows.iter().filter(|r|r["delivered"]==true && r["confirmed"]!=true && r["desistiu"]!=true && r["papel"]!="assistant").cloned().collect();
            let clock=||super::protocol::ClockSample {monotonic_s:0.0,epoch_s:std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0,|d|d.as_secs_f64())};
            let mut requeued=0;
            for row in rows {
                let (text,id)=(row["text"].as_str().unwrap_or("").trim().to_owned(),row["id"].as_str().unwrap_or("").to_owned());
                if !text.is_empty() && !committed.contains(&text) {
                    let entry=crate::mods::state::random_hex(16);
                    let pre_transcript=row["pre_transcript"]==true;
                    // A "Na conversa" volta marcada: segue entrando no turno, sem esperar o fim dele no painel.
                    let action=if row["conversation"]==true {super::queue::Action::AppendConversation { text,pre_transcript,entry_id:Some(entry.clone()) }}
                        else {super::queue::Action::Append { text,delivered:false,ts:None,pre_transcript,entry_id:Some(entry.clone()) }};
                    store.exec(generation,&format!("requeue:{entry}"),clock(),action).map_err(|_|refused("queue_io","fila da sessão inacessível"))?;
                    requeued+=1;
                }
                store.exec(generation,&format!("requeue-confirm:{}",crate::mods::state::random_hex(8)),clock(),super::queue::Action::Confirm { entry_ids:vec![id] })
                    .map_err(|_|refused("queue_io","fila da sessão inacessível"))?;
            }
            if requeued>0 {tracing::info!(requeued,"mensagens sem confirmação voltaram à fila");}
            Ok(requeued)
        }).await.map_err(|_|refused("queue_job","a fila da sessão não respondeu"))?
    }

    async fn queue_generation(&self,key:&str) -> Result<u64,RuntimeError> {
        let (state,_)=super::codex_terminal::queue_files(&self.codex_environment().claude_base.join(".hangar-queue"),key);
        tokio::task::spawn_blocking(move||match super::queue::read_replaced(&state) {
            Err(error) if error.kind()==std::io::ErrorKind::NotFound=>Ok(1),
            Err(_)=>Err(refused("queue_io","fila da sessão inacessível")),
            Ok(raw)=>serde_json::from_slice::<Value>(&raw).ok().and_then(|s|s["generation"].as_u64()).ok_or_else(||refused("queue_io","fila da sessão ilegível")),
        }).await.map_err(|_|refused("queue_job","a fila da sessão não respondeu"))?
    }

    /// Só a linha não entregue segura a troca (`_motivo_ocupada`).
    async fn queue_pending(&self,key:&str) -> bool {
        let (state,_)=super::codex_terminal::queue_files(&self.codex_environment().claude_base.join(".hangar-queue"),key);
        tokio::task::spawn_blocking(move||match super::queue::read_replaced(&state) {
            Err(_)=>false,
            Ok(raw)=>serde_json::from_slice::<Value>(&raw).is_ok_and(|s|s["rows"].as_array().is_some_and(|rows|rows.iter().any(|r|r["delivered"]==false))),
        }).await.unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_terminal_cleanup_requires_proof_of_absence() {
        let absent = MuxOutput { code: Some(1), stderr: "no server running on /tmp/tmux-test/default".into(), ..Default::default() };
        assert!(failed_terminal_pid(&absent).unwrap().is_none());
        let pane = MuxOutput { code: Some(0), stdout: "1234\n".into(), ..Default::default() };
        assert_eq!(failed_terminal_pid(&pane).unwrap(), Some(1234));
        for output in [MuxOutput::default(), MuxOutput { code: Some(1), stderr: "unknown command".into(), ..Default::default() },
            MuxOutput { code: Some(0), ..Default::default() }] {
            assert_eq!(failed_terminal_pid(&output).unwrap_err().code, NEW_ALIVE);
        }
    }

    /// Sessão adotada do terminal (`terminal_<hex>`) não tem registro: a troca de conta segue sem marca;
    /// a do lançador fica marcada até a vida nova ser gravada.
    #[test]
    fn only_a_recorded_session_is_marked_while_switching_accounts() {
        let home = tempfile::tempdir().unwrap();
        let spec = |key:&str| SessionSpec { version:crate::session_open::SPEC_VERSION,key:key.into(),name:"s".into(),cwd:"/w".into(),
            provider:crate::session_open::AgentProvider::Claude,surface:Surface::Terminal,account:Account::Default,
            account_source:crate::session_open::AccountSource::Explicit,model:None,effort:None,service_tier:None,permission_mode:None,
            subagent_model:None,customizations:None,jev:false,code_mode:false,read_only:false,worktree:None,creator:None,created_at:0.0,conversation:None,
            name_auto:None,rename_suggestion:None,rename_weak_at:None,rename_dismissed:false,hooks_off:false };
        let adopted = format!("terminal_{}", "e6".repeat(32));
        assert!(SwitchMark::write(home.path(),&spec(&adopted)).unwrap().is_none());
        let key = "0123456789abcdef0123456789abcdef";
        let mark = SwitchMark::write(home.path(),&spec(key)).unwrap().expect("sessão do lançador marcada");
        let path = crate::session_open::record::switching_path(home.path(),key).unwrap();
        assert!(path.is_file());
        drop(mark);
        assert!(!path.exists());
    }

    /// O Python repassa o ambiente com o nome em maiúsculas (`PROGRAMFILES`); no Windows o nome não tem caixa.
    #[cfg(windows)]
    #[test]
    fn the_managed_policy_is_found_whatever_the_case_of_the_variable() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("ClaudeCode")).unwrap();
        std::fs::write(dir.path().join("ClaudeCode/managed-settings.json"),
            r#"{"permissions":{"disableBypassPermissionsMode":"disable"}}"#).unwrap();
        let env = std::collections::BTreeMap::from([("PROGRAMFILES".to_owned(), dir.path().to_string_lossy().into_owned())]);
        let error = bypass_policy(&dir.path().join("conta"), &dir.path().join("cwd"), None, &env).unwrap_err();
        assert_eq!(error.code, "relaunch_policy_forbids_bypass");
    }
    /// Sem sidecar a sessão é com terminal; com ele, sem; adormecida não está em nenhuma, e só a marca
    /// `asleep` de superfície conhecida conta.
    #[test]
    fn an_asleep_session_is_on_no_surface_until_it_wakes() {
        assert_eq!(surface_now(None),Some(Surface::Terminal));
        assert_eq!(surface_now(Some(&json!({"key":"k"}))),Some(Surface::Headless));
        let asleep=json!({"key":"k","asleep":{"wake_surface":"terminal","since":1.0}});
        assert_eq!((surface_now(Some(&asleep)),asleep_of(&asleep)),(None,Some(Surface::Terminal)));
        let crooked=json!({"key":"k","asleep":{"wake_surface":"tela"}});
        assert_eq!((surface_now(Some(&crooked)),asleep_of(&crooked)),(Some(Surface::Headless),None));
    }
    use crate::list::procs::ChildrenMap;
    use std::ffi::OsString;
    use std::sync::Arc;

    /// Um Claude cujo argv se lê e o ambiente não (outro dono, `/proc` protegido).
    struct Sealed;
    impl ProcessView for Sealed {
        fn children(&self,_:Duration) -> std::io::Result<Arc<ChildrenMap>> { Ok(Arc::default()) }
        fn argv_checked(&self,_:i64) -> std::io::Result<Vec<String>> { Ok(vec!["claude".into(),"--permission-mode".into(),"manual".into()]) }
        fn cwd(&self,_:i64) -> Option<PathBuf> { None }
        fn env_var(&self,_:i64,_:&str) -> std::io::Result<Option<OsString>> { Err(std::io::ErrorKind::PermissionDenied.into()) }
        fn start_time(&self,_:i64) -> Option<f64> { None }
    }

    /// Sem ler o ambiente não se sabe se a sessão roda protegida (só leitura) nem em que conta:
    /// recusa, como o Python, em vez de reabrir fora da proteção.
    #[test]
    fn an_unreadable_environment_refuses_instead_of_guessing_not_read_only() {
        let error=spec_of(&Sealed,7,"k","s1","/w".into(),0.0).unwrap_err();
        assert_eq!(error.code,"lifecycle_process_read");
    }
}
