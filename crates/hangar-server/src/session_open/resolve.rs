//! `resolve_open()`: o pedido vira o spec, na ordem do `create_session` do Python: provedor padrão →
//! herança da criadora → conta Codex padrão → regra dos 95% → padrões da sessão
//! completa → validação. Os fatos entram por `OpenFacts`; nada aqui escreve em disco.
use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;
use serde_json::{Value, json};

use super::{Account, AccountSource, AgentProvider, OpenError, PermissionMode, SPEC_VERSION, ServiceTier, SessionOpenRequest,
    SessionSpec, Surface, WorktreeSpec};

/// O que decide o provedor padrão (`_session_provider_catalog` + `choose_provider`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProviderCatalog {
    /// Na ordem de preferência (`AgentProvider::ALL`).
    pub available: Vec<AgentProvider>,
    pub connected: Vec<AgentProvider>,
    pub disconnected: Vec<AgentProvider>,
    /// `last_session_provider` cru do `runtime-config.json`.
    pub remembered: Option<String>,
}

/// `headless_default` e `jev_padrao` do servidor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ServerDefaults { pub headless: bool, pub jev: bool }

/// A sessão que pediu a criação.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreatorFacts {
    pub provider: AgentProvider,
    pub headless: bool,
    /// `Ok(None)` = conta padrão; `Err` = não deu para confirmar (pane sem pid, ambiente ilegível).
    pub config_dir: Result<Option<String>, ()>,
    /// Modo fora de `plan`, cru: do sidecar ou do transcript (Claude), do sidecar (Codex).
    pub permission_mode: Option<String>,
    pub codex_account: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodexLogin { pub id: String, pub is_default: bool }

/// Como cada falha do catálogo do Codex sobe (`codex_models._call`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CodexCatalogError {
    /// A escolha não existe no catálogo.
    Invalid(String),
    /// Catálogo fora do ar ou `codex` ausente: só o Fast depende dele para abrir.
    Unavailable(String),
    /// O Codex recusou ou respondeu fora do formato.
    Refused(String),
}

pub trait OpenFacts {
    /// Contas Codex que não dá para listar são erro, nunca "nenhuma".
    fn providers(&self) -> Result<ProviderCatalog, OpenError>;
    /// `Ok(None)` = não existe; registro da criadora ilegível é erro, nunca "não encontrada".
    fn creator(&self, name: &str) -> Result<Option<CreatorFacts>, OpenError>;
    fn defaults(&self) -> ServerDefaults;
    /// Padrão de modelo, esforço e permissão marcado para provedor, motor e conta do motor.
    fn saved_defaults(&self, provider: AgentProvider, engine: &str, engine_account: &str) -> Result<Option<crate::session_defaults::Defaults>, String>;
    /// Contas Codex com login conectado.
    fn codex_logins(&self) -> Result<Vec<CodexLogin>, OpenError>;
    /// Linhas de cota das contas Claude e Codex: do cache, ou depois de uma leitura curta da rede
    /// (`network`, prazo de 5 s; estourado, o que o cache tiver).
    fn quotas(&self, network: bool) -> Vec<Value>;
    /// `limit-spent.json`: pasta de conta Claude → até quando o limite que ela bateu vale.
    fn spent_accounts(&self) -> std::collections::BTreeMap<String, f64>;
    /// A abertura seguiu sem saber a cota da conta: vai ao log e ao diário, nunca à tela.
    fn quota_unknown(&self, name: &str, account: &str);
    /// A pasta com `~` expandido, se ela existe.
    fn existing_dir(&self, cwd: &str) -> Option<String>;
    /// Caminhos de conta Claude aceitos (`list_config_dirs`), comparados como texto.
    fn config_dirs(&self) -> Result<Vec<String>, OpenError>;
    /// Nomes válidos do `engines.json`.
    fn engines(&self) -> Vec<String>;
    fn supports_fast(&self, engine: Option<&str>, model: Option<&str>) -> bool;
    /// Conta ChatGPT fixa do motor: confere conta e modelos e devolve o modelo sem o prefixo da conta.
    fn fixed_engine_model(&self, engine: &str, account: &str, model: Option<&str>) -> Result<String, OpenError>;
    /// Janela do modelo no catálogo do motor (`None` = o catálogo não traz o modelo ou o número).
    /// Catálogo que não responde é erro; a abertura segue com a janela desconhecida.
    fn engine_window(&self, engine: &str, model: &str, account: Option<&str>) -> Result<Option<u64>, OpenError>;
    /// Pasta da conta Codex pelo id (`resolve_account`).
    fn codex_home(&self, account_id: &str) -> Result<String, OpenError>;
    fn codex_choice(&self, home: Option<&str>, model: Option<&str>, effort: Option<&str>, tier: Option<ServiceTier>)
        -> Result<(), CodexCatalogError>;
    /// `permissions.defaultMode` cru da conta; `Err` = settings ilegível.
    fn account_mode(&self, config_dir: Option<&str>) -> Result<Option<String>, String>;
    /// Terminal, sidecar Codex ou sidecar sem terminal com este nome; `Err` = o multiplexador não respondeu.
    fn name_in_use(&self, name: &str) -> Result<bool, OpenError>;
    /// Grupo, `then` ou fila de uma sessão antiga com este nome que ninguém vai limpar (servidor fora).
    fn name_has_leftovers(&self, name: &str) -> Result<bool, OpenError>;
    /// Troca de agente em curso no nome (`conversation_transfer.transfer_active`).
    fn transfer_busy(&self, name: &str) -> Result<bool, OpenError>;
    /// `os.path.realpath(os.path.expanduser(path))`.
    fn real_path(&self, path: &str) -> std::path::PathBuf;
    fn new_key(&self) -> String;
    fn now(&self) -> f64;
}

/// Quem pede a abertura.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpenCaller {
    Owner,
    /// Só abre dentro da pasta dele, não herda de sessão do dono nem usa as contas e plugins do dono.
    Guest { root: std::path::PathBuf },
}

/// O spec e o que é só da abertura: avisos para a tela, a primeira mensagem e o lembrar provedor.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Resolved {
    pub spec: SessionSpec,
    pub warnings: Vec<String>,
    /// De onde veio o agente omitido no pedido: `inherited` (a criadora), `remembered` (o último do app) ou
    /// `default`; `None` = pedido explícito ou decidido por opção exclusiva.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_source: Option<&'static str>,
    pub initial_prompt: Option<String>,
    pub remember_provider: bool,
    /// Claude sem terminal que nasce no plano: o modo da conta, para onde "Implementar o plano" volta.
    pub previous_non_plan: Option<PermissionMode>,
    /// A conversa que já existe e esta abertura retoma (Arquivo, `/resume`, restore); `None` = nova.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resume: Option<String>,
    /// A mesma sessão voltando depois do reboot: o grupo, a fila, o `then` e o par do nome são dela e ficam.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub restore: bool,
}

/// `model_args.ID_OK`, com o `\Z` do Python: quebra de linha no fim é recusada.
static MODEL_OK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\A[A-Za-z0-9._:~/\[\]-]{1,128}\z").unwrap());
/// O id de modelo que pode ir à linha de comando: a regex e sem `-` na frente.
pub(crate) fn valid_model(model: &str) -> bool { MODEL_OK.is_match(model) && !model.starts_with('-') }
static CODEX_EFFORT_OK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\A[a-z]{2,32}\z").unwrap());
const EFFORT_CLAUDE: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];
const EFFORT_PI: [&str; 7] = ["off", "minimal", "low", "medium", "high", "xhigh", "max"];
/// Uso da janela mais cheia a partir do qual a conta que ninguém escolheu não recebe sessão nova.
const QUOTA_LIMIT: f64 = 95.0;
/// `tmux` troca `.` por `_`, e o Rust já recusa nome maior que isso ao reabrir.
const NAME_MAX: usize = 64;

fn truthy(value: &Option<String>) -> bool { value.as_deref().is_some_and(|v| !v.is_empty()) }

fn conflict(msg: impl Into<String>) -> OpenError { OpenError::new(409, "erro_criacao_sessao", msg) }

pub fn resolve_open(request: &SessionOpenRequest, caller: &OpenCaller, facts: &dyn OpenFacts) -> Result<Resolved, OpenError> {
    let mut used_saved_choice = false;
    match resolve_with(request, caller, facts, true, &mut used_saved_choice) {
        // Modelo ou esforço salvos que deixaram de valer (catálogo mudou) não travam toda abertura.
        Err(error) if used_saved_choice => {
            let mut resolved = resolve_with(request, caller, facts, false, &mut used_saved_choice)?;
            resolved.warnings.push(format!("o modelo ou esforço do padrão salvo não vale para esta abertura ({}); ficou sem eles", error.msg));
            Ok(resolved)
        }
        other => other,
    }
}

fn resolve_with(request: &SessionOpenRequest, caller: &OpenCaller, facts: &dyn OpenFacts, saved_choice: bool,
                used_saved_choice: &mut bool) -> Result<Resolved, OpenError> {
    let guest = matches!(caller, OpenCaller::Guest { .. });
    let mut warnings = Vec::new();
    // O omitido vem da criadora; `inherit: false` deixa tudo no padrão da conta. O convidado
    // nunca herda conta nem modo de uma sessão do dono, e nem fica sabendo dela.
    let creator = request.creator.as_deref().filter(|_| request.inherit != Some(false) && !guest);
    let creator_info = match creator { Some(name) => facts.creator(name)?, None => None };
    // Provedor omitido: opção exclusiva de um provedor decide; senão o da criadora (o par quer o mesmo
    // agente), e só sem criadora o padrão do servidor.
    let (provider, provider_source) = match &request.provider {
        Some(name) => (AgentProvider::parse(name)
            .ok_or_else(|| OpenError::new(400, "erro_provider_sessao_invalido", "provider invalido"))?, None),
        None if request.config_dir.is_some() || truthy(&request.engine) || request.subagent_model.is_some() => (AgentProvider::Claude, None),
        None if request.codex_account.is_some() => (AgentProvider::Codex, None),
        None if truthy(&request.omp_profile) => (AgentProvider::Omp, None),
        None => match &creator_info {
            Some(info) => (info.provider, Some("inherited")),
            None => {
                let catalog = facts.providers()?;
                let chosen = choose_provider(&catalog);
                let remembered = catalog.remembered.as_deref().and_then(AgentProvider::parse) == Some(chosen);
                (chosen, Some(if remembered { "remembered" } else { "default" }))
            }
        },
    };
    // O padrão salvo pelo dono preenche o que o pedido não disse e vence a criadora; a permissão
    // dela só vale quando o pedido traz `inherit: true`.
    let saved = if guest { None } else {
        match facts.saved_defaults(provider, request.engine.as_deref().unwrap_or_default(), request.engine_account.as_deref().unwrap_or_default()) {
            Ok(saved) => saved,
            Err(reason) => {
                warnings.push(format!("não consegui ler o padrão salvo da abertura ({reason}); ele não foi aplicado"));
                None
            }
        }
    };
    let saved_permission = saved.as_ref().filter(|_| request.permission_mode.is_none())
        .map(|saved| saved.permission.clone()).filter(|mode| !mode.is_empty());
    // Com `inherit: true` o modo da criadora vem primeiro e o salvo só cobre a falta dele.
    let saved_first = request.inherit != Some(true);
    // O Codex com terminal recusa modo pedido; o salvo segue como o herdado.
    let saved_codex_mode = saved_permission.as_deref().filter(|_| provider == AgentProvider::Codex).and_then(PermissionMode::codex);
    let filled;
    let request = match &saved {
        Some(saved) => {
            let mut pick = |asked: &Option<String>, value: &str| match asked.clone().filter(|v| !v.is_empty()) {
                Some(asked) => Some(asked),
                None if saved_choice && !value.is_empty() => { *used_saved_choice = true; Some(value.to_owned()) }
                None => asked.clone(),
            };
            filled = SessionOpenRequest {
                model: pick(&request.model, &saved.model),
                effort: pick(&request.effort, &saved.effort),
                permission_mode: request.permission_mode.clone()
                    .or_else(|| saved_permission.clone().filter(|_| saved_first && provider != AgentProvider::Codex)),
                ..request.clone()
            };
            &filled
        }
        None => request,
    };
    let explicit_account = request.config_dir.is_some();
    let mut config_dir = request.config_dir.clone();
    let mut codex_account = request.codex_account.clone();
    let mut headless = request.headless;
    // `default` é o nome que o Claude Code dá ao manual (`claude --permission-mode default`).
    let mut permission = request.permission_mode.clone()
        .map(|mode| if provider == AgentProvider::Claude && mode == "default" { "manual".to_owned() } else { mode });
    // Fora do `permission`: o pedido explícito num Codex com terminal continua recusado, o herdado não.
    let mut inherited_codex_mode = None;
    let mut source = if explicit_account { AccountSource::Explicit } else { AccountSource::Default };

    if let Some(creator) = creator {
        match creator_info.as_ref() {
            None => warnings.push(format!("sessão criadora '{creator}' não encontrada; nada foi herdado")),
            Some(info) => {
                // Perfil do omp já define a conta; a criadora Codex não tem conta Claude.
                if config_dir.is_none() && matches!(provider, AgentProvider::Claude | AgentProvider::Pi | AgentProvider::Omp)
                    && !truthy(&request.omp_profile) && info.provider != AgentProvider::Codex
                {
                    // Nascer na padrão calado gastaria a cota de quem ninguém escolheu.
                    config_dir = info.config_dir.clone().map_err(|()| OpenError::new(409, "erro_conta_criadora", format!(
                        "não consegui confirmar a conta da sessão '{creator}' — escolha a conta (`conta` no MCP, `--conta` no hangar-send)")))?;
                    source = AccountSource::Inherited;
                }
                // Para o Codex: a conta Codex da criadora Codex.
                if provider == AgentProvider::Codex && codex_account.is_none() && info.provider == AgentProvider::Codex {
                    codex_account = Some(info.codex_account.clone().unwrap_or_else(|| "default".into()));
                    source = AccountSource::Inherited;
                }
                if headless.is_none() && matches!(provider, AgentProvider::Claude | AgentProvider::Codex) && !request.read_only {
                    headless = Some(info.headless);
                }
                // O 1º recado de uma criadora em bypass para uma irmã em Manual fica retido no receptor.
                if permission.is_none() && provider == AgentProvider::Claude
                    && matches!(info.provider, AgentProvider::Claude | AgentProvider::Codex)
                {
                    match creator_mode(info) {
                        Some(mode) => permission = Some(mode.name().to_owned()),
                        None => warnings.push(format!("não consegui ler o modo de permissão de '{creator}'; vale o padrão da conta")),
                    }
                }
                if permission.is_none() && (saved_codex_mode.is_none() || !saved_first) && provider == AgentProvider::Codex
                    && matches!(info.provider, AgentProvider::Claude | AgentProvider::Codex)
                {
                    inherited_codex_mode = creator_codex_mode(info);
                    if inherited_codex_mode.is_none() {
                        warnings.push(format!("não consegui ler o modo de permissão de '{creator}'; vale o padrão da conta"));
                    }
                }
            }
        }
    }

    let inherited_codex_mode = if saved_first { saved_codex_mode.or(inherited_codex_mode) } else { inherited_codex_mode.or(saved_codex_mode) };
    if permission.is_none() && provider == AgentProvider::Claude {
        permission = saved_permission.filter(|_| !saved_first);
    }

    // A padrão herdada é a mesma escolha de ninguém ter pedido conta: desconectada, troca igual.
    let unchosen = codex_account.is_none() || (source == AccountSource::Inherited && codex_account.as_deref() == Some("default"));
    // As outras contas Codex são do dono: o convidado fica na padrão.
    if provider == AgentProvider::Codex && unchosen && !guest {
        let connected = facts.codex_logins()?;
        if let Some(first) = connected.first().filter(|_| !connected.iter().any(|login| login.is_default)) {
            codex_account = Some(first.id.clone());
            warnings.push(format!("A conta Codex padrão não está conectada; a sessão usa a conta {}.", first.id));
        }
    }

    // Conta a 95% ou mais: a que ninguém escolheu (herdada ou padrão) troca pela de mais folga; a
    // mandada pela tela é recusada com a sugerida, para a tela oferecer a troca.
    let explicit = if provider == AgentProvider::Codex { request.codex_account.is_some() } else { explicit_account };
    let quota_rule = match provider {
        // O convidado não recebe oferta de outra conta do dono.
        AgentProvider::Claude => !truthy(&request.engine) && !(guest && explicit),
        AgentProvider::Codex => !guest,
        _ => false,
    };
    if quota_rule {
        let name = request.name.as_deref().unwrap_or_default();
        let current = if provider == AgentProvider::Codex { codex_account.as_deref().or(Some("default")) } else { config_dir.as_deref() };
        match quota_check(provider, current, explicit, facts) {
            QuotaCheck::Room => {}
            QuotaCheck::Unknown(account) => facts.quota_unknown(name, &account),
            QuotaCheck::Exhausted { current, best } if explicit => return Err(no_quota(&current, best.as_ref())),
            QuotaCheck::Exhausted { best: None, .. } => {}
            QuotaCheck::Exhausted { current, best: Some(best) } => {
                let warning = format!("conta {} com {:.0}% de uso; a sessão nasceu em {}", current.label, current.usage.unwrap_or(100.0), best.label);
                tracing::warn!("create_session {name}: {warning}");
                if provider == AgentProvider::Codex { codex_account = Some(best.choice) } else { config_dir = Some(best.choice) }
                warnings.push(warning);
                source = AccountSource::Quota;
            }
        }
    }

    let headless = headless.unwrap_or_else(|| {
        !request.read_only && matches!(provider, AgentProvider::Claude | AgentProvider::Codex) && facts.defaults().headless
    });
    let model = validate(request, caller, provider, headless, config_dir.as_deref(), codex_account.as_deref(), permission.as_deref(), facts)?;
    let raw = match &request.name {
        Some(raw) => raw.clone(),
        None => folder_name(&request.cwd, request.take_reserved_name, facts)?,
    };
    let name = validate_name(&raw, provider, headless, request, facts)?;
    // As telas mandam o nome da pasta que elas mesmas calculam: vale o formato, não quem mandou.
    let name_auto = folder_pattern(&name, &request.cwd);

    let permission_mode = match provider {
        AgentProvider::Claude => Some(match permission.as_deref() {
            // `validate` já recusou o modo fora da lista.
            Some(mode) => PermissionMode::claude(mode).unwrap_or(PermissionMode::BypassPermissions),
            // Sem escolha nem criadora vale o modo da conta.
            None => account_mode(config_dir.as_deref(), facts)?,
        }),
        // O lançador do Codex sem modo abre em Full Access (`sem_terminal.MODO_PADRAO`).
        AgentProvider::Codex => Some(permission.as_deref().and_then(PermissionMode::codex).or(inherited_codex_mode)
            .unwrap_or(PermissionMode::FullAccess)),
        _ => None,
    };
    let cwd = facts.existing_dir(&request.cwd).unwrap_or_else(|| request.cwd.clone());
    let engine = request.engine.as_deref().filter(|e| !e.is_empty());
    let (context_window, context_window_unknown) = match (engine, model.as_deref().filter(|m| !m.is_empty())) {
        (Some(engine), Some(model)) => match facts.engine_window(engine, model, request.engine_account.as_deref()) {
            Ok(window) => (window, false),
            // Provedor fora do ar não impede abrir (escolha do Python); a janela fica
            // desconhecida no spec e é relida em cada reabertura.
            Err(error) => {
                tracing::warn!(code = error.code, "janela do motor não lida na abertura; a sessão abre sem ela");
                (None, true)
            }
        },
        _ => (None, false),
    };
    let account = match (provider, engine) {
        (AgentProvider::Codex, _) => Account::Codex { account_id: codex_account.unwrap_or_else(|| "default".into()) },
        (_, Some(engine)) => Account::Engine {
            name: engine.into(), account: request.engine_account.clone(), config_dir: config_dir.clone(), context_window,
            context_window_unknown,
        },
        (AgentProvider::Omp, _) if truthy(&request.omp_profile) => Account::Omp {
            profile: request.omp_profile.clone().unwrap_or_default(), config_dir: config_dir.clone(),
        },
        (_, None) => config_dir.clone().map_or(Account::Default, |config_dir| Account::Claude { config_dir }),
    };
    let spec = SessionSpec {
        version: SPEC_VERSION, key: facts.new_key(), name, cwd, provider,
        surface: if headless { Surface::Headless } else { Surface::Terminal },
        account, account_source: source, model, effort: request.effort.clone(), service_tier: request.service_tier,
        permission_mode, subagent_model: request.subagent_model.clone(), customizations: request.claude_customizations.clone(),
        jev: request.jev.unwrap_or_else(|| facts.defaults().jev), read_only: request.read_only,
        worktree: request.branch.clone().map(|branch| WorktreeSpec { branch, new_branch: request.new_branch, base: request.base.clone(), path: None }),
        creator: request.creator.clone(), created_at: facts.now(), conversation: None,
        name_auto: Some(name_auto), rename_suggestion: None, rename_dismissed: false,
    };
    // Nascer já no plano sem terminal deixaria a sessão sem modo de base. O ilegível vale bypass, como o
    // `modo_da_conta` do Python: o modo pedido já está escolhido, só a volta do plano depende dele.
    let previous_non_plan = (spec.surface == Surface::Headless && spec.permission_mode == Some(PermissionMode::Plan))
        .then(|| account_mode(spec.account.config_dir(), facts).unwrap_or(PermissionMode::BypassPermissions));
    // Só a abertura do dono muda o provedor lembrado.
    Ok(Resolved { spec, warnings, provider_source, initial_prompt: request.initial_prompt.clone(), remember_provider: request.remember_provider && !guest,
        previous_non_plan, resume: None, restore: false })
}

/// `session_defaults.choose_provider`.
pub fn choose_provider(catalog: &ProviderCatalog) -> AgentProvider {
    let remembered = catalog.remembered.as_deref().and_then(AgentProvider::parse)
        .filter(|p| catalog.available.contains(p) && (!catalog.disconnected.contains(p) || catalog.connected.is_empty()));
    remembered
        .or_else(|| catalog.available.iter().copied().find(|p| catalog.connected.contains(p)))
        .or_else(|| catalog.available.first().copied())
        .unwrap_or(AgentProvider::Claude)
}

/// `_creator_permission_mode` / `_codex_as_claude_mode`: o modo da criadora no nome do `--permission-mode`.
fn creator_mode(info: &CreatorFacts) -> Option<PermissionMode> {
    let raw = info.permission_mode.as_deref()?;
    if info.provider == AgentProvider::Codex {
        // Chave exata do `PERMISSION_POLICIES`, sem a tolerância de maiúscula da abertura.
        return Some(match PermissionMode::CODEX.into_iter().find(|m| m.name() == raw)? {
            PermissionMode::FullAccess => PermissionMode::BypassPermissions,
            PermissionMode::ApproveForMe => PermissionMode::AcceptEdits,
            _ => PermissionMode::Manual,
        });
    }
    PermissionMode::claude(if raw == "default" { "manual" } else { raw }).filter(|m| *m != PermissionMode::Plan)
}

/// Modo da criadora numa sessão Codex: a nova nunca é mais permissiva que ela. Codex para Codex pelo
/// nome; do Claude, o inverso do `_codex_as_claude_mode` (bypass → Full Access, acceptEdits →
/// Approve for me, manual → Ask for approval), e os modos sem imagem no Codex (auto, dontAsk, plan)
/// vão para Ask for approval, o mais restrito.
fn creator_codex_mode(info: &CreatorFacts) -> Option<PermissionMode> {
    let raw = info.permission_mode.as_deref()?;
    if info.provider == AgentProvider::Codex {
        return PermissionMode::CODEX.into_iter().find(|m| m.name() == raw);
    }
    Some(match PermissionMode::claude(if raw == "default" { "manual" } else { raw })? {
        PermissionMode::BypassPermissions => PermissionMode::FullAccess,
        PermissionMode::AcceptEdits => PermissionMode::ApproveForMe,
        _ => PermissionMode::AskForApproval,
    })
}

/// `modo_da_conta`: `default` vira `manual`; ausente ou desconhecido, o padrão do app (bypass).
/// Settings ilegível recusa: cair no modo mais permissivo pareceria escolha de quem abriu.
fn account_mode(config_dir: Option<&str>, facts: &dyn OpenFacts) -> Result<PermissionMode, OpenError> {
    match facts.account_mode(config_dir) {
        Ok(Some(mode)) => Ok(PermissionMode::claude(if mode == "default" { "manual" } else { &mode }).unwrap_or(PermissionMode::BypassPermissions)),
        Ok(None) => Ok(PermissionMode::BypassPermissions),
        Err(reason) => Err(OpenError::new(409, "erro_modo_padrao_ilegivel",
            format!("não consegui ler o modo padrão da conta ({reason}); escolha o modo de permissão"))),
    }
}

/// Uma conta Claude ou Codex na regra de cota.
#[derive(Debug)]
struct QuotaAccount {
    /// O id da credencial (`claude:<pasta>`, `codex:<pasta>`), o mesmo de `/api/credenciais`.
    id: String,
    label: String,
    usage: Option<f64>,
    /// Leitura atual (`lida`): só ela serve de sugestão; a guardada de antes do token vencer, não.
    read: bool,
    active: bool,
    /// O campo do pedido e o valor dele que abrem nesta conta.
    field: &'static str,
    choice: String,
}

#[derive(Debug)]
enum QuotaCheck { Room, Unknown(String), Exhausted { current: QuotaAccount, best: Option<QuotaAccount> } }

/// Uso conhecido: a janela mais cheia da leitura (a guardada de antes do token vencer também vale) e
/// 100% enquanto o limite que a troca automática gravou não volta.
fn quota_usage(row: Option<&Value>, spent_until: Option<f64>, now: f64) -> Option<f64> {
    let read = row.filter(|row| row["estado"] == "lida" || row["ts"].is_number())
        .and_then(|row| row["janelas"].as_array()?.iter().filter_map(|w| w["pct"].as_f64()).reduce(f64::max));
    read.into_iter().chain(spent_until.filter(|until| *until > now).map(|_| 100.0)).reduce(f64::max)
}

/// A conta atual e as outras do mesmo provedor; `None` = conta que a regra não alcança (Codex sem pasta).
fn quota_accounts(provider: AgentProvider, current: Option<&str>, rows: &[Value], spent: &std::collections::BTreeMap<String, f64>,
                  facts: &dyn OpenFacts) -> Option<(QuotaAccount, Vec<QuotaAccount>)> {
    let now = facts.now();
    let read = |row: Option<&Value>| row.is_some_and(|row| row["estado"] == "lida");
    let label = |row: Option<&Value>, fallback: &str| row.and_then(|row| row["label"].as_str()).map_or_else(|| fallback.to_owned(), str::to_owned);
    if provider == AgentProvider::Codex {
        let current = current.unwrap_or("default");
        let account = |id: &str| -> Option<QuotaAccount> {
            let home = facts.codex_home(id).ok()?;
            let row = rows.iter().find(|row| row["provedor"] == "codex"
                && row["id"].as_str().and_then(|i| i.strip_prefix("codex:")).is_some_and(|path| same_dir(path, &home)));
            Some(QuotaAccount { id: row.and_then(|r| r["id"].as_str()).map_or_else(|| format!("codex:{home}"), str::to_owned),
                label: label(row, id), usage: quota_usage(row, None, now), read: read(row), active: false,
                field: "codex_account", choice: id.to_owned() })
        };
        let others = facts.codex_logins().unwrap_or_default().iter().filter(|login| login.id != current)
            .filter_map(|login| account(&login.id)).collect();
        return Some((account(current)?, others));
    }
    let spent_until = |path: &str| spent.iter().find(|(dir, _)| same_dir(dir, path)).map(|(_, until)| *until);
    let account = |row: &Value| -> Option<QuotaAccount> {
        let path = row["id"].as_str()?.strip_prefix("claude:")?;
        Some(QuotaAccount { id: row["id"].as_str()?.to_owned(), label: label(Some(row), path), usage: quota_usage(Some(row), spent_until(path), now),
            read: read(Some(row)), active: row["ativa"] == true, field: "config_dir", choice: path.to_owned() })
    };
    let claude: Vec<QuotaAccount> = rows.iter().filter(|row| row["provedor"] == "claude").filter_map(account).collect();
    let (mine, others): (Vec<_>, Vec<_>) = claude.into_iter().partition(|a| match current {
        None => a.active,
        Some(dir) => same_dir(&a.choice, dir),
    });
    let current = mine.into_iter().next().unwrap_or_else(|| {
        // Sem linha de cota: ainda vale o limite gravado da pasta pedida.
        let dir = current.unwrap_or_default();
        let named = if dir.is_empty() { "padrão" } else { dir };
        QuotaAccount { id: format!("claude:{named}"), label: named.to_owned(),
            usage: quota_usage(None, current.and_then(spent_until), now), read: false, active: current.is_none(),
            field: "config_dir", choice: dir.to_owned() }
    });
    Some((current, others))
}

/// `cotas.conta_com_cota`: a conta a 95% ou mais e a de mais folga que a substitui (a ativa no empate).
/// A sugerida para a conta escolhida pela tela também fica abaixo dos 95%: senão a nova tentativa
/// seria recusada de novo.
fn quota_verdict(provider: AgentProvider, current: Option<&str>, explicit: bool, rows: &[Value],
                 spent: &std::collections::BTreeMap<String, f64>, facts: &dyn OpenFacts) -> QuotaCheck {
    let Some((current, others)) = quota_accounts(provider, current, rows, spent, facts) else { return QuotaCheck::Room };
    let Some(usage) = current.usage else { return QuotaCheck::Unknown(current.id) };
    if usage < QUOTA_LIMIT {
        return QuotaCheck::Room;
    }
    let best = others.into_iter().filter(|a| a.read && a.id != current.id)
        .filter_map(|a| Some((a.usage?, a)))
        .filter(|(other, _)| *other < usage && (!explicit || *other < QUOTA_LIMIT))
        .reduce(|best, next| if (-next.0, next.1.active) > (-best.0, best.1.active) { next } else { best })
        .map(|(_, a)| a);
    QuotaCheck::Exhausted { current, best }
}

/// A regra pelo cache; sem cota conhecida da conta, uma leitura curta da rede antes de desistir.
fn quota_check(provider: AgentProvider, current: Option<&str>, explicit: bool, facts: &dyn OpenFacts) -> QuotaCheck {
    let spent = if provider == AgentProvider::Claude { facts.spent_accounts() } else { Default::default() };
    match quota_verdict(provider, current, explicit, &facts.quotas(false), &spent, facts) {
        QuotaCheck::Unknown(_) => quota_verdict(provider, current, explicit, &facts.quotas(true), &spent, facts),
        known => known,
    }
}

/// 409 da conta mandada pela tela sem cota. `retry` é só o campo do pedido que a tela troca para abrir
/// na sugerida; sem sugerida ele não vem.
fn no_quota(current: &QuotaAccount, best: Option<&QuotaAccount>) -> OpenError {
    let usage = format!("{:.0}", current.usage.unwrap_or(100.0));
    let msg = match best {
        Some(best) => format!("a conta {} está com {usage}% de uso; abra a sessão na conta {}", current.label, best.label),
        None => format!("a conta {} está com {usage}% de uso e nenhuma outra conta tem cota", current.label),
    };
    let mut params = json!({"conta": current.label, "conta_id": current.id, "uso": usage,
        "sugerida": best.map_or("", |b| b.label.as_str()), "sugerida_id": best.map_or("", |b| b.id.as_str())});
    if let Some(best) = best {
        params["retry"] = json!({best.field: best.choice});
    }
    OpenError::new(409, "erro_conta_sem_cota", msg).with_params(params)
}

/// `Path(a).resolve() == Path(b).resolve()`, sem exigir que existam.
fn same_dir(a: &str, b: &str) -> bool {
    let trim = |p: &str| p.trim_end_matches(['/', '\\']).to_owned();
    trim(a) == trim(b) || crate::list::discover::real(a) == crate::list::discover::real(b)
}

/// Validação na ordem do `_create_session_owned` (o primeiro erro vence), tudo antes de qualquer
/// efeito. Devolve o modelo efetivo (a conta ChatGPT fixa tira o prefixo dela).
#[allow(clippy::too_many_arguments)]
fn validate(request: &SessionOpenRequest, caller: &OpenCaller, provider: AgentProvider, headless: bool, config_dir: Option<&str>,
            codex_account: Option<&str>, permission: Option<&str>, facts: &dyn OpenFacts) -> Result<Option<String>, OpenError> {
    let claude = provider == AgentProvider::Claude;
    if request.claude_customizations.is_some() {
        if !claude {
            return Err(OpenError::new(400, "claude_customizations_invalid", "plugins e skills por sessão só valem para Claude"));
        }
        if matches!(caller, OpenCaller::Guest { .. }) {
            return Err(OpenError::new(403, "claude_customizations_owner_only", "a seleção de plugins pertence ao dono"));
        }
    }
    if request.service_tier.is_some() && provider != AgentProvider::Codex
        && !(claude && facts.supports_fast(request.engine.as_deref(), request.model.as_deref()))
    {
        return Err(OpenError::new(400, "erro_criacao_sessao", "Fast exige Codex ou Claude com GPT no CLIProxyAPI local"));
    }
    // Antes de qualquer outra coisa da pasta: o convidado nem fica sabendo se ela existe fora da dele.
    if let OpenCaller::Guest { root } = caller && !facts.real_path(&request.cwd).starts_with(root) {
        return Err(OpenError::new(403, "erro_fora_da_pasta", "o convidado só abre sessão dentro da pasta dele"));
    }
    if facts.existing_dir(&request.cwd).is_none() {
        return Err(OpenError::new(400, "erro_cwd_inexistente", format!("a pasta {} não existe", request.cwd))
            .with_params(json!({"cwd": request.cwd})));
    }
    let mut model = request.model.clone();
    if let Some(account) = &request.engine_account {
        let Some(engine) = request.engine.as_deref().filter(|e| claude && !e.is_empty()) else {
            return Err(OpenError::new(400, "erro_cliproxy_conta", "conta ChatGPT exige Claude com motor CLIProxyAPI local"));
        };
        model = Some(facts.fixed_engine_model(engine, account, model.as_deref())?);
    }
    if codex_account.is_some() && provider != AgentProvider::Codex {
        return Err(OpenError::new(400, "codex_account_so_codex", "codex_account só vale para provider codex"));
    }
    let codex_home = match provider {
        AgentProvider::Codex => Some(facts.codex_home(codex_account.unwrap_or("default"))?),
        _ => None,
    };
    if let Some(dir) = config_dir && !facts.config_dirs()?.iter().any(|known| same_dir(known, dir)) {
        return Err(OpenError::new(400, "erro_config_dir_invalido", "config_dir invalido"));
    }
    if let Some(engine) = &request.engine {
        if !claude {
            return Err(OpenError::new(400, "erro_motor_sem_claude", "motor so vale para provider claude"));
        }
        if !facts.engines().contains(engine) {
            return Err(OpenError::new(400, "erro_motor_invalido", "motor invalido"));
        }
    }
    // O Codex sem terminal aceita o modo dele (vira sandbox e aprovação).
    if permission.is_some() && !claude && !(provider == AgentProvider::Codex && headless) {
        return Err(OpenError::new(409, "erro_permissao_so_claude", "modo de permissao so vale para claude"));
    }
    if truthy(&request.omp_profile) && provider != AgentProvider::Omp {
        return Err(OpenError::new(400, "erro_perfil_so_omp", "perfil so vale para provider omp"));
    }
    if let Some(subagent) = &request.subagent_model {
        if !claude || truthy(&request.engine) {
            return Err(OpenError::new(400, "erro_subagente_so_claude", "modelo dos subagentes so vale para claude sem motor"));
        }
        validate_choice(AgentProvider::Claude, Some(subagent), None, None)
            .map_err(|e| OpenError::plain(400, e.msg.replacen("model:", "subagent_model:", 1)))?;
    }
    validate_choice(provider, model.as_deref(), request.effort.as_deref(), permission.filter(|_| provider != AgentProvider::Codex))?;
    if provider == AgentProvider::Codex && (truthy(&request.model) || truthy(&request.effort) || request.service_tier.is_some()) {
        // O nível do Codex varia por modelo: só o catálogo cruza modelo × nível × Fast.
        // Conta nomeada (pedida, herdada ou a conectada no lugar da padrão) lê o catálogo dela.
        let home = codex_home.as_deref().filter(|_| codex_account.is_some());
        match facts.codex_choice(home, request.model.as_deref(), request.effort.as_deref(), request.service_tier) {
            Ok(()) => {}
            Err(CodexCatalogError::Invalid(e)) => return Err(OpenError::new(422, "erro_codex_escolha_invalida", e.clone()).with_params(json!({"erro": e}))),
            Err(CodexCatalogError::Unavailable(e)) if request.service_tier == Some(ServiceTier::Priority) => {
                return Err(OpenError::new(502, "erro_codex_catalogo_invalido", format!("Fast não pôde ser conferido: {e}")).with_params(json!({"erro": e})));
            }
            // Catálogo fora do ar não impede abrir: a escolha segue e o CLI decide.
            Err(CodexCatalogError::Unavailable(e)) => tracing::warn!("codex: catalogo indisponivel, escolha nao conferida: {e}"),
            Err(CodexCatalogError::Refused(e)) => return Err(OpenError::new(502, "erro_codex_catalogo_invalido", e.clone()).with_params(json!({"erro": e}))),
        }
    }
    if request.new_branch && request.branch.is_none() {
        return Err(OpenError::new(400, "erro_criacao_sessao", "branch nova sem nome"));
    }
    Ok(model)
}

/// `model_args.validar`: a regex é a barreira do shell, e cada binário tem a própria flag de esforço.
fn validate_choice(provider: AgentProvider, model: Option<&str>, effort: Option<&str>, permission: Option<&str>) -> Result<(), OpenError> {
    let name = provider.name();
    if let Some(model) = model && (!MODEL_OK.is_match(model) || model.starts_with('-')) {
        return Err(OpenError::plain(400, "model: use letras, números e . _ : / - ~ [ ] (até 128 caracteres, sem começar com -)"));
    }
    if let Some(effort) = effort {
        let levels: &[&str] = match provider {
            AgentProvider::Codex if CODEX_EFFORT_OK.is_match(effort) => &[],
            AgentProvider::Codex => return Err(OpenError::plain(400,
                "effort: use letras minúsculas (2 a 32) — os níveis do Codex variam por modelo e vêm do provedor")),
            AgentProvider::Claude => &EFFORT_CLAUDE,
            AgentProvider::Pi | AgentProvider::Omp => &EFFORT_PI,
            AgentProvider::Kimi => return Err(OpenError::plain(400, format!("provider '{name}' não aceita escolha de esforço aqui"))),
        };
        if !levels.is_empty() && !levels.contains(&effort) {
            return Err(OpenError::plain(400, format!("effort: use um de {}", levels.join(", "))));
        }
    }
    if let Some(mode) = permission && PermissionMode::claude(mode).is_none() {
        let names: Vec<_> = PermissionMode::CLAUDE.iter().map(|m| m.name()).collect();
        return Err(OpenError::new(409, "erro_permissao_invalida", format!("permission_mode: use um de {}", names.join(", "))));
    }
    Ok(())
}

/// O que o `registry.create` confere antes de qualquer efeito: nome, sem terminal, perfil do omp e
/// nome em uso. Devolve o nome sanitizado.
fn validate_name(raw: &str, provider: AgentProvider, headless: bool, request: &SessionOpenRequest, facts: &dyn OpenFacts)
    -> Result<String, OpenError> {
    let name = crate::list::discover_other::sanitize_session_name(raw);
    // Um status só para o nome inválido, com ou sem `branch`.
    if name.is_empty() {
        return Err(conflict("nome invalido"));
    }
    if name.len() > NAME_MAX {
        return Err(conflict(format!("nome invalido: use até {NAME_MAX} caracteres")));
    }
    if facts.transfer_busy(&name)? {
        return Err(OpenError::new(409, "session_transfer_busy", "A sessão está trocando de agente; tente novamente quando terminar."));
    }
    if headless {
        if !matches!(provider, AgentProvider::Claude | AgentProvider::Codex) {
            return Err(conflict("sessao sem terminal so vale para provider claude ou codex"));
        }
        // O prompt inicial é entregue em todo provedor; só leitura continua exigindo terminal.
        if request.read_only {
            return Err(conflict("sessao sem terminal nao aceita read_only"));
        }
    }
    if provider == AgentProvider::Omp && let Some(profile) = request.omp_profile.as_deref().filter(|p| !p.is_empty()) {
        // A mesma regra do omp: o valor vai para o ambiente do pane como `OMP_PROFILE`.
        let profile = profile.trim();
        if !(profile.is_empty() || profile == "default" || crate::list::discover_other::valid_omp_profile(profile)) {
            return Err(conflict("Nome de perfil OMP inválido"));
        }
    }
    if facts.name_in_use(&name)? {
        return Err(OpenError::new(409, "erro_nome_em_uso", "ja existe uma sessao com esse nome"));
    }
    if provider == AgentProvider::Codex && headless && let Some(mode) = request_permission(request)
        && PermissionMode::codex(mode).is_none()
    {
        let names: Vec<_> = PermissionMode::CODEX.iter().map(|m| m.name()).collect();
        return Err(conflict(format!("permission_mode: use um de {}", names.join(", "))));
    }
    Ok(name)
}

/// A base do nome que ninguém escolheu: a pasta saneada, ou `sessao`.
pub(crate) fn folder_base(cwd: &str) -> String {
    let folder = cwd.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next().unwrap_or_default();
    Some(crate::list::discover_other::sanitize_session_name(folder)).filter(|n| !n.is_empty()).unwrap_or_else(|| "sessao".into())
}

/// A base (ASCII, já saneada) cede espaço ao sufixo para o nome caber no teto.
pub(crate) fn fit_name(base: &str, suffix: &str) -> String {
    format!("{}{suffix}", base[..base.len().min(NAME_MAX - suffix.len())].trim_end_matches('-'))
}

/// O nome tem a forma do que a pasta dá: a base dela ou `sessao`, com ou sem `-N`.
pub(crate) fn folder_pattern(name: &str, cwd: &str) -> bool {
    let from = |base: &str| name == fit_name(base, "") || name.rsplit_once('-').is_some_and(|(_, n)|
        (1..8).contains(&n.len()) && n.bytes().all(|b| b.is_ascii_digit()) && name == fit_name(base, &format!("-{n}")));
    from(folder_base(cwd).as_str()) || from("sessao")
}

/// O nome que ninguém escolheu, pela regra do `uniqueSessionName` das telas. Com `take_reserved`, o
/// nome que só uma reserva pré-aquecida segura conta como livre: a pessoa não a vê, e a abertura a descarta.
fn folder_name(cwd: &str, take_reserved: bool, facts: &dyn OpenFacts) -> Result<String, OpenError> {
    let base = folder_base(cwd);
    // Quem está abrindo agora também ocupa.
    let fit = |suffix: &str| fit_name(&base, suffix);
    let held = |name: &str| if take_reserved { super::open::NameClaim::opening_without_reserves(name) } else { super::open::NameClaim::opening(name) };
    let mut name = fit("");
    for n in 2.. {
        if !facts.name_in_use(&name)? && !held(&name) && !facts.name_has_leftovers(&name)? { break; }
        name = fit(&format!("-{n}"));
    }
    Ok(name)
}

/// O Codex nunca herda o modo da criadora: só o pedido conta.
fn request_permission(request: &SessionOpenRequest) -> Option<&str> { request.permission_mode.as_deref() }
