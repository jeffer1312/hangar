//! Barra lateral como a do web (Sidebar.svelte + sessionListModel): filtro, agrupar por projeto, ordem por nome,
//! prévia da última resposta ao parar o mouse e o menu da sessão (Renomear, Silenciar, Copiar cwd, Abrir no editor,
//! Fechar). Toda resposta volta amarrada à sessão capturada no gesto (máquina + nome) e ao número do pedido: resposta
//! velha não mexe no que a pessoa fez depois. Cada gesto vai à máquina da linha, como o `withServer` do web.
use super::*;
use gpui_kit::base::AccordionTrigger;
use gpui_kit::component::{WindowExt, dialog, menu::{DropdownMenu, PopupMenu}, notification::NotificationType};
use super::machines::enter_to_focused;
use super::activity::web;
use std::{cell::RefCell, rc::Rc};

/// O filtro aparece com mais de 6 sessões, contadas antes de filtrar (`FILTER_FROM` do web).
const FILTER_FROM: usize = 6;
const PREVIEW_DELAY: Duration = Duration::from_millis(400);
const PREVIEW_TTL: Duration = Duration::from_secs(30);
// Cauda de 8 e não 3: corridas de ferramentas empurram a última resposta para trás (mesmo número do web).
const PREVIEW_TAIL: usize = 8;
const PRESS: Duration = Duration::from_millis(500);
pub(super) const PREVIEW_W: f32 = 380.;
pub(super) const PREVIEW_H: f32 = 220.;
const NO_CWD: &str = "no-cwd";

/// Sessão de uma linha: a máquina (endereço normalizado, `servers::norm`) e o nome, que sozinho se repete entre máquinas.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct Target { pub(super) server: String, pub(super) name: String }

/// O que a segunda linha da sessão diz: a última resposta, a pergunta em aberto ou o que o agente está fazendo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Sub { Reply, Question, Working }

type LastSubs = HashMap<Target, (Option<String>, (String, Sub))>;

/// ↑↓ numa lista: dão a volta nas pontas; sem item atual, o sentido escolhe a ponta.
pub(super) fn wrap_step(current: Option<usize>, count: usize, step: isize) -> Option<usize> {
    if count == 0 { return None; }
    Some(match current {
        Some(i) => (i as isize + step).rem_euclid(count as isize) as usize,
        None if step > 0 => 0,
        None => count - 1,
    })
}

// A linha só troca quando há texto novo: no vão entre estados (enviou e o turno ainda não pegou, parou e a resposta
// ainda não chegou) ela segura a anterior, e o card nunca encolhe para crescer de novo logo depois. Só a conversa
// trocada (`/clear`, outro transcript) a descarta; transcript ainda desconhecido não troca nada.
fn kept_sub(last: &mut LastSubs, target: &Target, jsonl: Option<&str>, fresh: Option<(String, Sub)>) -> Option<(String, Sub)> {
    if jsonl.is_some() && last.get(target).is_some_and(|(seen, _)| seen.is_some() && seen.as_deref() != jsonl) {
        last.remove(target);
    }
    match fresh {
        Some(sub) => {
            let seen = jsonl.map(str::to_owned).or_else(|| last.get(target).and_then(|(seen, _)| seen.clone()));
            last.insert(target.clone(), (seen, sub.clone()));
            Some(sub)
        }
        None => last.get(target).map(|(_, sub)| sub.clone()),
    }
}

impl Target {
    pub(super) fn new(server: &str, name: &str) -> Self { Self { server: server.to_owned(), name: name.to_owned() } }
    /// Id estável dos elementos da linha: o nome sozinho colidiria entre máquinas.
    pub(super) fn id(&self) -> String { format!("{}::{}", self.server, self.name) }
}

/// O servidor confere as guardas novamente; a tela só oferece uma origem identificável e parada.
pub(super) fn transfer_source(session: &SessionInfo) -> Option<(&str, &str)> {
    if session.provider != "claude" || session.engine.as_deref().is_some_and(|e| !e.is_empty()) || session.uses_engine_account()
        || session.read_only() || session.tracked == Some(false) || !matches!(session.state.as_str(), "idle" | "dead")
        || !session.conta.as_deref().is_some_and(|c| c.starts_with("claude:"))
        || session.pending_questions != 0 || session.question.is_some() || session.options.as_ref().is_some_and(|o| !o.is_empty())
        || session.transfer_phase.as_deref().is_some_and(|p| !matches!(p, "rejected" | "rolled_back")) { return None; }
    let life = session.lifecycle_id.as_deref().filter(|s| s.len() > 2 && (s.starts_with("k:") || s.starts_with("t:")))?;
    let jsonl = session.jsonl.as_deref().filter(|s| !s.is_empty())?;
    Some((life, jsonl))
}

/// Estado de silenciar da sessão do menu aberto, lido de `GET /api/push/settings`.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Mute { Loading, Known(bool), Failed(String) }

/// Branches do repositório da sessão do menu aberto, lidas de `GET …/branches` ao abrir o menu.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Branches { Loading, Known(BranchList), Failed(String) }

#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize)]
pub(super) struct BranchList { #[serde(default)] branches: Vec<String>, current: Option<String>, #[serde(default)] dirty: bool }

/// Contas Claude para onde a conversa pode ir, sem a da sessão, de `GET …/conta` ao abrir o menu.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Accounts { Loading, Known(Vec<AccountTarget>, Option<String>), Failed(String) }

/// `pct` é a janela de cota mais cheia (None = sem leitura); `full` é conta perto demais do limite para continuar nela.
#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
pub(super) struct AccountTarget {
    pub(super) path: String,
    pub(super) label: String,
    pub(super) pct: Option<f64>,
    #[serde(default)] pub(super) low: bool,
    #[serde(default)] pub(super) full: bool,
    pub(super) engine_account: Option<String>,
}

struct MenuRead { target: Target, seq: u64, mute: Mute, branches: Option<Branches>, accounts: Option<Accounts> }

#[derive(Clone, Debug)]
/// O renomear leva o número do pedido: resposta de um diálogo cancelado não fecha a tentativa seguinte na mesma sessão.
pub(super) enum Write { Rename(String, u64), Mute(bool), Editor, Delete, Mode(bool), Account { path: String, label: String, engine_account: Option<String> } }

/// Gravações de git e o remover vínculo: o resultado é a notificação da sessão, já em texto.
enum GitWrite { Pull, Checkout(String), StashCheckout(String), Unlink }

pub(super) enum SidebarReply {
    Preview(u64, Target, Result<String, Failure>),
    MuteRead(u64, Result<Value, Failure>),
    BranchRead(u64, Result<Value, Failure>),
    AccountsRead(u64, Result<(Vec<AccountTarget>, Option<String>), Failure>),
    Wrote(Target, Write, Result<Value, Failure>),
    /// Resultado de uma gravação de git: nível e texto da notificação daquela sessão.
    Note(Target, NotificationType, String),
    Chained(Target, u64, String, Result<Value, Failure>),
    NotSaved(String),
    /// Resposta de agrupar, sair do grupo ou sugerir a tarefa, amarrada ao número do pedido.
    Group(u64, super::grouping::GroupReply),
    /// Resposta de um pedido do painel do grupo.
    Sheet(super::group_sheet::SheetReply),
}

/// Uma notificação por sessão: o "git pull…" dá lugar ao resultado, como o `flash` único do web.
struct GitNote;

pub(super) fn git_note(name: &str, kind: NotificationType, text: String) -> Notification {
    // O título diz de qual sessão é: o gesto pode ter sido numa linha que não é a aberta.
    Notification::new().title(name.to_owned()).message(text).with_type(kind).id1::<GitNote>(SharedString::from(name.to_owned()))
}

/// Git no menu só com pasta num repositório: o backend manda `branch` nulo fora de um (como o web, SCM:162).
pub(super) fn has_git(s: &SessionInfo) -> bool { s.cwd.as_deref().is_some_and(|c| !c.is_empty()) && s.branch.is_some() }

fn account_body(path: &str, engine_account: Option<&str>) -> Value {
    match engine_account { Some(account) => json!({"engine_account": account}), None => json!({"config_dir": path}) }
}

fn merge_account_targets(mut legacy: Vec<AccountTarget>, proxy: Result<Option<Vec<AccountTarget>>, String>, local_proxy: bool)
    -> Result<(Vec<AccountTarget>, Option<String>), Failure> {
    match proxy {
        Ok(Some(mut targets)) => { legacy.append(&mut targets); Ok((legacy, None)) }
        Ok(None) => Ok((Vec::new(), None)),
        Err(error) if local_proxy => Ok((legacy, Some(error))),
        Err(error) => Err(Failure::local(error)),
    }
}

async fn read_account_targets(api: &Api, session: &SessionInfo) -> Result<(Vec<AccountTarget>, Option<String>), Failure> {
    let legacy = api.read(&session.name, &["conta"], &[], 15).await?;
    let targets: Vec<AccountTarget> = serde_json::from_value(legacy).map_err(|_| Failure::local(tr("invalid_response")))?;
    if session.engine.as_deref().is_some_and(|engine| !engine.is_empty()) || session.uses_engine_account() {
        let (credentials, engines) = tokio::join!(api.server_read(&["credenciais"], &[], 30), api.server_read(&["engines"], &[], 15));
        let engines = match engines {
            Ok(engines) => engines,
            Err(error) if session.uses_engine_account() => return Ok((targets, Some(Hangar::fetch_failure(&error)))),
            Err(error) => return Err(error),
        };
        let local_proxy = engines.get("motores").and_then(|m| m.get(session.engine.as_deref().unwrap_or("")))
            .and_then(|m| m.get("cliproxy_accounts")).is_some_and(Value::is_array);
        let proxy = credentials.map_err(|e| Hangar::fetch_failure(&e))
            .and_then(|credentials| super::accounts::session_proxy_targets(session, credentials, engines));
        return merge_account_targets(targets, proxy, local_proxy);
    }
    Ok((targets, None))
}

fn first_line(value: &Value) -> Option<String> {
    value.get("output").and_then(Value::as_str).and_then(|o| o.trim().lines().next()).filter(|l| !l.is_empty()).map(str::to_owned)
}

async fn git_result(api: &Api, name: &str, what: GitWrite) -> (NotificationType, String) {
    let failed = |key: &str, error: &Failure| (NotificationType::Error, tr(key).replace("{n}", &Hangar::fetch_failure(error)));
    let checkout = async |branch: &str, done: String| match api.act(name, &["checkout"], Some(json!({"branch": branch})), false, 60).await {
        Ok(_) => (NotificationType::Success, done),
        Err(error) => failed("sidebar_checkout_failed", &error),
    };
    match what {
        // O backend responde 200 com `ok: false` quando o git recusa: aí é aviso com a saída, não sucesso.
        GitWrite::Pull => match api.act(name, &["git"], Some(json!({"action": "pull"})), false, 120).await {
            Ok(value) if value.get("ok").and_then(Value::as_bool) != Some(true) => (NotificationType::Warning,
                tr("sidebar_pull_failed").replace("{n}", &first_line(&value).unwrap_or_else(|| tr("sidebar_git_refused")))),
            Ok(value) => (NotificationType::Success, first_line(&value).unwrap_or_else(|| tr("sidebar_pull_ok"))),
            Err(error) => failed("sidebar_pull_failed", &error),
        },
        GitWrite::Checkout(branch) => checkout(&branch, tr("sidebar_switched").replace("{n}", &branch)).await,
        // Stash recusado para aqui: trocar de branch sem ter guardado levaria as mudanças junto.
        GitWrite::StashCheckout(branch) => match api.act(name, &["git"], Some(json!({"action": "stash"})), false, 60).await {
            Ok(value) if value.get("ok").and_then(Value::as_bool) != Some(true) => (NotificationType::Error, tr("sidebar_checkout_failed")
                .replace("{n}", &value.get("output").and_then(Value::as_str).map(str::trim).filter(|o| !o.is_empty()).map(str::to_owned)
                    .unwrap_or_else(|| tr("sidebar_stash_failed")))),
            Ok(_) => checkout(&branch, tr("sidebar_switched_stashed").replace("{n}", &branch)).await,
            Err(error) => failed("sidebar_checkout_failed", &error),
        },
        GitWrite::Unlink => match api.act(name, &["then"], None, true, 30).await {
            Ok(_) => (NotificationType::Success, tr("sidebar_unlinked")),
            Err(error) => failed("sidebar_unlink_failed", &error),
        },
    }
}

/// Encadear: o alvo escolhido no submenu (da mesma máquina da origem) e o prompt, num diálogo curto.
pub(super) struct Chain { from: Target, target: String, input: Entity<InputState>, status: Rc<RefCell<Pending>>, _events: Subscription }

/// Leva a resposta de volta à janela, amarrada à conexão do pedido.
pub(super) struct Tell(async_channel::Sender<Envelope>, u64);

impl Tell {
    pub(super) async fn send(&self, reply: SidebarReply) {
        let _ = self.0.send(Envelope { connection: self.1, selection: None, payload: Payload::Sidebar(reply) }).await;
    }
}

/// Nome em edição: na própria linha (barra lateral) ou num diálogo (abas, como o web com a barra recolhida).
pub(super) struct Edit {
    pub(super) target: Target, pub(super) input: Entity<InputState>, inline: bool, status: Rc<RefCell<Pending>>, _events: Subscription,
}

/// Diálogo de renomear: o pedido em voo e a falha dele seguram o resultado junto do campo; só fecha com o renomear confirmado,
/// como o web. Fora do `Hangar` porque o `open_dialog` chama o construtor na hora, com o `Hangar` em atualização.
#[derive(Default)]
struct Pending { sent: Option<u64>, error: Option<String> }

pub(super) struct Sidebar {
    pub(super) filter: Entity<InputState>,
    collapsed: HashSet<String>,
    /// Fechadas agora: somem da lista na hora e voltam se o servidor recusar.
    deleting: HashSet<Target>,
    pub(super) editing: Option<Edit>,
    renaming: HashSet<Target>,
    /// Aberta e renomeada: o nome novo abre assim que aparecer na lista.
    follow: Option<Target>,
    /// A aberta sumiu da lista com o renomear em voo: a resposta decide se ela reabre pelo nome novo.
    lost: Option<Target>,
    /// Trocando de conta: a sessão fecha e volta com o mesmo nome, então a aberta fica na tela até a resposta (e até a
    /// lista trazê-la de volta, que pode chegar depois). Instantes do envio e da resposta.
    pub(super) moving: HashMap<Target, (std::time::Instant, Option<std::time::Instant>)>,
    menu: Option<MenuRead>,
    menu_seq: u64,
    rename_seq: u64,
    pub(super) chain: Option<Chain>,
    chain_seq: u64,
    /// Renomeada pelo diálogo antes de a aba nova existir: o foco vai a ela quando a lista trouxer o nome.
    focus_tab: Option<Target>,
    /// Foco das linhas das outras máquinas; as da ativa usam o `tab_focus`, que as abas também usam.
    remote_focus: HashMap<Target, FocusHandle>,
    /// Número do último clique em cabeçalho de grupo: gravação de um clique anterior que termine depois não volta o arquivo.
    collapse_gen: u64,
    /// Linha cujo ⋯ está com o menu aberto: o botão fica na tela enquanto o ponteiro anda pelo menu.
    pub(super) button_menu: Option<Target>,
    pub(super) hover: Option<Target>,
    hover_seq: u64,
    pointer_y: f32,
    pub(super) preview: Option<(Target, Entity<TextViewState>, f32)>,
    cache: HashMap<Target, (String, Instant)>,
    /// Última segunda linha mostrada de cada sessão, com o transcript dela: vale enquanto a nova vier vazia.
    last_sub: std::cell::RefCell<LastSubs>,
    press_seq: u64,
    long_pressed: bool,
    /// A troca entre a lista e o trilho em andamento: quando começou e se vai para o trilho.
    rail_anim: Option<(Instant, bool)>,
    /// Arrasto da borda em curso: onde o ponteiro desceu e a largura naquele instante.
    resize: Option<(f32, f32)>,
    /// Arrastar sessão sobre sessão e o diálogo de agrupar/sair que ele abre.
    pub(super) grouping: super::grouping::Grouping,
}

impl Sidebar {
    pub(super) fn new(window: &mut Window, cx: &mut Context<Hangar>) -> Self {
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder(tr("sidebar_filter")).clean_on_escape());
        cx.subscribe(&filter, |_, _, _: &InputEvent, cx| cx.notify()).detach();
        Self { filter, collapsed: load_collapsed(), deleting: HashSet::new(), editing: None, renaming: HashSet::new(), follow: None, lost: None, moving: HashMap::new(),
            menu: None, menu_seq: 0, rename_seq: 0, chain: None, chain_seq: 0, focus_tab: None, remote_focus: HashMap::new(), collapse_gen: 0, button_menu: None, hover: None, hover_seq: 0, pointer_y: 0., preview: None, cache: HashMap::new(), last_sub: Default::default(), press_seq: 0,
            long_pressed: false, rail_anim: None, resize: None, grouping: Default::default() }
    }

    /// Troca de servidor: o que é da conexão anterior sai; filtro e grupos recolhidos são deste computador e ficam.
    pub(super) fn reset_server(&mut self) {
        self.deleting.clear();
        self.renaming.clear();
        (self.editing, self.follow, self.lost, self.menu, self.button_menu, self.hover, self.preview) = (None, None, None, None, None, None, None);
        (self.focus_tab, self.chain) = (None, None);
        self.grouping.reset();
        self.chain_seq += 1;
        self.cache.clear();
        self.last_sub.borrow_mut().clear();
        self.menu_seq += 1;
        self.hover_seq += 1;
        self.press_seq += 1;
    }

    /// A segunda linha da sessão. Vazia, fica a última mostrada da mesma conversa: o rótulo do que o agente faz some e
    /// volta entre uma etapa e outra, e a linha indo junto muda a altura do card a cada vez.
    pub(super) fn keep_sub(&self, target: &Target, jsonl: Option<&str>, fresh: Option<(String, Sub)>) -> Option<(String, Sub)> {
        kept_sub(&mut self.last_sub.borrow_mut(), target, jsonl, fresh)
    }

    fn menu_for(&self, target: &Target) -> Option<Mute> {
        self.menu.as_ref().filter(|m| m.target == *target).map(|m| m.mute.clone())
    }

    fn branches_for(&self, target: &Target) -> Option<Branches> {
        self.menu.as_ref().filter(|m| m.target == *target).and_then(|m| m.branches.clone())
    }

    fn accounts_for(&self, target: &Target) -> Option<Accounts> {
        self.menu.as_ref().filter(|m| m.target == *target).and_then(|m| m.accounts.clone())
    }

    /// Fechada agora na máquina `server`: fora da lista até o servidor responder.
    pub(super) fn is_hidden(&self, server: &str, name: &str) -> bool { self.deleting.iter().any(|t| t.server == server && t.name == name) }

    /// Os nomes escondidos da máquina `server`, no formato que o `layout` lê.
    pub(super) fn hidden_on(&self, server: &str) -> HashSet<String> {
        self.deleting.iter().filter(|t| t.server == server).map(|t| t.name.clone()).collect()
    }

    pub(super) fn is_collapsed(&self, key: &str) -> bool { self.collapsed.contains(key) }
}

pub(super) struct Group<'a> { pub(super) key: String, pub(super) label: String, pub(super) sessions: Vec<&'a SessionInfo> }

/// O que a barra mostra, na ordem: "Aguardando você" no topo (nos dois modos, decisão do árbitro) e depois um grupo só
/// ("Sessões") ou um por projeto. Nenhuma sessão aparece duas vezes.
pub(super) struct Layout<'a> {
    pub(super) waiting: Vec<&'a SessionInfo>,
    pub(super) groups: Vec<Group<'a>>,
    pub(super) by_project: bool,
    /// Sessões antes do filtro: é o que decide se o filtro aparece.
    pub(super) total: usize,
    pub(super) filtering: bool,
}

impl Layout<'_> {
    pub(super) fn show_filter(&self) -> bool { self.total > FILTER_FROM }
    pub(super) fn filter_empty(&self) -> bool { self.filtering && self.waiting.is_empty() && self.groups.is_empty() }
}

/// `projectKey` do web: o cwd inteiro sem a barra final; sem cwd, uma chave que nunca é um caminho.
pub(super) fn project_key(cwd: Option<&str>) -> String {
    match cwd.filter(|c| !c.is_empty()) {
        Some(cwd) => { let trimmed = cwd.trim_end_matches('/'); if trimmed.is_empty() { "/".into() } else { trimmed.into() } }
        None => NO_CWD.into(),
    }
}

pub(super) fn project_label(cwd: Option<&str>) -> String {
    // `\` também separa: pasta de servidor Windows.
    cwd.map(crate::composer::basename).filter(|b| !b.is_empty() && !b.chars().all(|c| c == '/' || c == '\\'))
        .map(str::to_owned).unwrap_or_else(|| tr("sidebar_no_project"))
}

// ponytail: `localeCompare` sem tabela de colação — minúsculas com os acentos do português dobrados na letra-base, empate pelo
// texto cru. Outras escritas ordenam por ponto de código; uma tabela (icu_collator) resolveria se isso importar.
fn text_key(text: &str) -> (String, String) { (sort_key(text), text.to_owned()) }

fn sort_key(text: &str) -> String {
    text.to_lowercase().chars().map(|c| match c {
        'á' | 'à' | 'â' | 'ã' | 'ä' => 'a', 'é' | 'è' | 'ê' | 'ë' => 'e', 'í' | 'ì' | 'î' | 'ï' => 'i',
        'ó' | 'ò' | 'ô' | 'õ' | 'ö' => 'o', 'ú' | 'ù' | 'û' | 'ü' => 'u', 'ç' => 'c', 'ñ' => 'n', c => c,
    }).collect()
}

pub(super) fn layout<'a>(sessions: &'a [SessionInfo], query: &str, by_project: bool, hidden: &HashSet<String>) -> Layout<'a> {
    let live: Vec<&SessionInfo> = sessions.iter().filter(|s| !hidden.contains(&s.name)).collect();
    let total = live.len();
    // Filtro escondido não filtra: com a lista curta de novo, o texto que ficou no campo não esconde ninguém.
    let q = if total > FILTER_FROM { query.trim().to_lowercase() } else { String::new() };
    let matches = |s: &SessionInfo| q.is_empty() || s.name.to_lowercase().contains(&q)
        || s.cwd.as_deref().unwrap_or("").to_lowercase().contains(&q)
        || (by_project && project_label(s.cwd.as_deref()).to_lowercase().contains(&q));
    let mut shown: Vec<&SessionInfo> = live.into_iter().filter(|s| matches(s)).collect();
    // Chave de ordem montada uma vez por sessão, não duas por comparação.
    shown.sort_by_cached_key(|s| text_key(&s.name));
    let (waiting, rest): (Vec<&SessionInfo>, Vec<&SessionInfo>) = shown.into_iter().partition(|s| s.state == "awaiting_input");
    let groups = if by_project {
        let mut by_key: Vec<Group> = Vec::new();
        for s in rest {
            let key = project_key(s.cwd.as_deref());
            match by_key.iter_mut().find(|g| g.key == key) {
                Some(g) => g.sessions.push(s),
                None => by_key.push(Group { label: project_label(s.cwd.as_deref()), key, sessions: vec![s] }),
            }
        }
        by_key.sort_by_cached_key(|g| text_key(&g.label));
        by_key
    } else if rest.is_empty() { Vec::new() } else { vec![Group { key: "*".into(), label: tr("sessions"), sessions: rest }] };
    Layout { waiting, groups, by_project, total, filtering: !q.is_empty() }
}

fn collapsed_path() -> Option<PathBuf> {
    Some(crate::appearance::dir()?.join("sidebar-collapsed.json"))
}

/// Arquivo ausente é a primeira abertura; ilegível vai ao log em vez de reabrir os grupos sem explicação.
fn load_collapsed() -> HashSet<String> {
    let Some(path) = collapsed_path() else { return HashSet::new() };
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return HashSet::new(),
        Err(e) => { eprintln!("grupos recolhidos ilegíveis em {}: {e}", path.display()); return HashSet::new(); }
    };
    serde_json::from_slice(&bytes).unwrap_or_else(|e| { eprintln!("grupos recolhidos inválidos em {}: {e}", path.display()); HashSet::new() })
}

/// Bloqueante. Só a geração mais nova grava, trocando o arquivo inteiro (tmp + rename), como `appearance::save`.
fn save_collapsed(path: Option<PathBuf>, generation: u64, saved: &HashSet<String>) -> std::io::Result<()> {
    static WRITTEN: std::sync::Mutex<u64> = std::sync::Mutex::new(0);
    let mut written = WRITTEN.lock().unwrap_or_else(|e| e.into_inner());
    if generation <= *written { return Ok(()); }
    let path = path.ok_or_else(|| std::io::Error::other("sem pasta de configuração"))?;
    std::fs::create_dir_all(path.parent().expect("arquivo dentro de hangar-native"))?;
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_vec(saved).map_err(std::io::Error::other)?)?;
    std::fs::rename(&tmp, &path)?;
    *written = generation;
    Ok(())
}

impl Hangar {
    pub(super) fn sidebar_tell(&self) -> Tell { Tell(self.tx.clone(), self.connection) }

    pub(super) fn by_project() -> bool { appearance::get().sidebar_group == appearance::SidebarGroup::Project }

    pub(super) fn sidebar_layout(&self, cx: &App) -> Layout<'_> {
        layout(&self.sessions, &self.sidebar.filter.read(cx).value(), Self::by_project(), &self.sidebar.hidden_on(&self.active_key()))
    }

    /// A lista de outra máquina como a barra a mostra: grupos de projeto com a chave dela, para recolher um não recolher o
    /// de mesmo caminho na outra.
    pub(super) fn remote_layout<'a>(&self, key: &str, sessions: &'a [SessionInfo], cx: &App) -> Layout<'a> {
        let mut l = layout(sessions, &self.sidebar.filter.read(cx).value(), Self::by_project(), &self.sidebar.hidden_on(key));
        for group in &mut l.groups { group.key = format!("{key}::{}", group.key); }
        l
    }

    /// A sessão da linha, lida da lista viva da máquina dela.
    pub(super) fn target_session(&self, target: &Target) -> Option<&SessionInfo> {
        self.sessions_of(&target.server).iter().find(|s| s.name == target.name)
    }

    /// A aberta, com a máquina dela.
    pub(super) fn selected_target(&self) -> Option<Target> {
        self.selected.as_ref().map(|s| Target::new(&self.open_server(), &s.name))
    }

    /// Linha de um convite: parar de acompanhar no lugar de fechar, e só o que é da própria sessão.
    pub(super) fn invite_target(&self, target: &Target) -> bool { self.server_entry(&target.server).is_some_and(|s| s.invite) }

    /// Foco da linha: da ativa é o da aba; das outras máquinas, o da própria linha.
    pub(super) fn row_focus(&self, target: &Target) -> Option<&FocusHandle> {
        if self.is_active_key(&target.server) { self.tab_focus.get(&target.name) } else { self.sidebar.remote_focus.get(target) }
    }

    /// Abre a sessão da linha na máquina dela, sem trocar o servidor ativo. `None`: fora da lista ou sem conexão (já avisada).
    pub(super) fn select_target(&mut self, target: &Target, window: &mut Window, cx: &mut Context<Self>) -> Option<SessionInfo> {
        let session = self.target_session(target)?.clone();
        self.select_on(&target.server, session.clone(), window, cx).then_some(session)
    }

    /// O clique na linha: abre e leva o foco ao compositor.
    pub(super) fn open_target(&mut self, target: &Target, window: &mut Window, cx: &mut Context<Self>) {
        self.hide_preview();
        if let Some(session) = self.select_target(target, window, cx) { self.focus_composer_for(&session, window, cx); }
    }

    /// Ordem em que o Ctrl+↓/↑ anda: a das linhas à vista na barra, máquina por máquina (pula filtrada e recolhida), ou a
    /// das abas.
    pub(super) fn visible_order(&self, cx: &App) -> Vec<Target> {
        let active = self.active_key();
        if appearance::get().navigation.tabs() {
            return self.sessions.iter().filter(|s| !self.sidebar.is_hidden(&active, &s.name)).map(|s| Target::new(&active, &s.name)).collect();
        }
        // Na ordem dos blocos de grupo, sem os membros de um bloco recolhido.
        let shown = |key: &str, remote: Option<&str>, l: &Layout| -> Vec<Target> {
            let rows = |list: &[&SessionInfo]| super::grouping::cluster(list).into_iter().filter_map(|row| match row {
                super::grouping::ListRow::Session(s) if !self.pair_collapsed(s, remote) => Some(Target::new(key, &s.name)),
                _ => None,
            }).collect::<Vec<_>>();
            rows(&l.waiting).into_iter()
                .chain(l.groups.iter().filter(|g| !(l.by_project && self.sidebar.collapsed.contains(&g.key))).flat_map(|g| rows(&g.sessions)))
                .collect()
        };
        if !self.multi_server() { return shown(&active, None, &self.sidebar_layout(cx)); }
        let mut order = Vec::new();
        for entry in self.servers.iter().filter(|s| !s.disabled) {
            let key = servers::norm(&entry.address);
            if self.sidebar.collapsed.contains(&format!("server:{key}")) { continue; }
            if key == active { order.extend(shown(&key, None, &self.sidebar_layout(cx))); }
            else if let Some(list) = self.remote.get(&key) { order.extend(shown(&key, Some(&key), &self.remote_layout(&key, &list.sessions, cx))); }
        }
        order
    }

    /// Atalhos que agem na sessão aberta ficam parados com diálogo, Configurações ou nome em edição. Com o nome em edição a
    /// tecla é do campo: trocar ou fechar a sessão deixaria o campo aberto numa linha que não é a aberta.
    fn session_keys_blocked(&self, window: &mut Window, cx: &mut App) -> bool {
        window.has_active_dialog(cx) || self.connection_dialog || (self.settings.is_some() && !self.settings_live())
            || self.sidebar.editing.is_some()
    }

    pub(super) fn step_session(&mut self, step: isize, window: &mut Window, cx: &mut Context<Self>) {
        if self.session_keys_blocked(window, cx) { return; }
        let order = self.visible_order(cx);
        if order.is_empty() { return; }
        let current = self.selected_target().and_then(|t| order.iter().position(|o| *o == t));
        let Some(next) = wrap_step(current, order.len(), step) else { return };
        self.select_target(&order[next], window, cx);
    }

    /// O "Fechar" do menu da linha: no convite, parar de acompanhar (nunca o servidor ativo); senão, a confirmação.
    fn close_target(&mut self, target: Target, window: &mut Window, cx: &mut Context<Self>) {
        if self.invite_target(&target) { self.stop_following(&target.server, window, cx) } else { self.confirm_delete(target, window, cx) }
    }

    /// Ctrl+W: o "Fechar" do menu para a sessão aberta.
    pub(super) fn close_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.session_keys_blocked(window, cx) { return; }
        let Some(target) = self.selected_target() else { return };
        // Orquestrador e sessão do par não têm "Fechar" no menu.
        if self.target_session(&target).is_none_or(|s| s.orq() || s.read_only()) { return; }
        self.close_target(target, window, cx);
    }

    /// F2: o mesmo Renomear do menu da linha, na sessão aberta.
    pub(super) fn rename_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.session_keys_blocked(window, cx) { return; }
        let Some(target) = self.selected_target() else { return };
        self.start_session_rename(target, window, cx);
    }

    pub(super) fn set_group(&mut self, project: bool, cx: &mut Context<Self>) {
        let group = if project { appearance::SidebarGroup::Project } else { appearance::SidebarGroup::None };
        self.apply_appearance(appearance::Appearance { sidebar_group: group, ..appearance::get() }, true, cx);
    }

    pub(super) fn toggle_group(&mut self, key: String, cx: &mut Context<Self>) {
        if !self.sidebar.collapsed.remove(&key) { self.sidebar.collapsed.insert(key); }
        self.sidebar.collapse_gen += 1;
        let (generation, saved, tell) = (self.sidebar.collapse_gen, self.sidebar.collapsed.clone(), self.sidebar_tell());
        // Disco fora da thread da janela; a falha aparece (na próxima abertura o grupo voltaria aberto sem explicação).
        self.runtime.spawn(async move {
            let result = tokio::task::spawn_blocking(move || save_collapsed(collapsed_path(), generation, &saved))
                .await.map_err(|e| e.to_string()).and_then(|r| r.map_err(|e| e.to_string()));
            if let Err(error) = result { tell.send(SidebarReply::NotSaved(error)).await; }
        });
        cx.notify();
    }

    /// Uma lista nova chegou (a ativa ou a de outra máquina): fechadas que saíram dela deixam de ser escondidas; renomeada
    /// aberta é reaberta pelo nome novo.
    pub(super) fn sidebar_sessions_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let alive = |this: &Self, t: &Target| this.target_session(t).is_some();
        let deleting = std::mem::take(&mut self.sidebar.deleting);
        self.sidebar.deleting = deleting.into_iter().filter(|t| alive(self, t)).collect();
        // Cada linha das outras máquinas guarda o próprio foco pela vida da sessão, como as da ativa.
        let rows: HashSet<Target> = self.remote.iter()
            .flat_map(|(key, list)| list.sessions.iter().map(move |s| Target::new(key, &s.name))).collect();
        self.sidebar.remote_focus.retain(|t, _| rows.contains(t));
        for t in rows { self.sidebar.remote_focus.entry(t).or_insert_with(|| cx.focus_handle().tab_stop(true)); }
        self.refresh_group_ask();
        self.refresh_group_sheet(window, cx);
        self.external_pairs_changed(cx);
        if self.sidebar.hover.as_ref().is_some_and(|t| !alive(self, t)) { self.hide_preview(); }
        // Só se o foco ainda está onde o renomear o deixou: gesto novo nesse meio-tempo vence.
        if let Some(new) = self.sidebar.focus_tab.clone().filter(|t| alive(self, t)) {
            self.sidebar.focus_tab = None;
            if self.root_focus.is_focused(window) && let Some(focus) = self.row_focus(&new).cloned() { focus.focus(window, cx); }
        }
        // Só deixa de seguir depois de abrir: sem conexão, a próxima lista tenta de novo.
        if let Some(new) = self.sidebar.follow.clone().filter(|t| alive(self, t)) {
            if self.selected.is_some() { self.sidebar.follow = None; }
            else if self.select_target(&new, window, cx).is_some() {
                self.sidebar.follow = None;
                self.error = None;
            }
        }
    }

    /// A sessão aberta sumiu da lista: se está sendo renomeada, a resposta do renomear decide; não é "sessão encerrada".
    pub(super) fn lost_while_renaming(&mut self, target: &Target) -> bool {
        let renaming = self.sidebar.renaming.contains(target);
        if renaming { self.sidebar.lost = Some(target.clone()); }
        renaming
    }

    // ── Prévia ──

    pub(super) fn row_hover(&mut self, target: Target, hovered: bool, cx: &mut Context<Self>) {
        if !hovered {
            if self.sidebar.hover.as_ref() == Some(&target) { self.hide_preview(); cx.notify(); }
            self.sidebar.press_seq += 1;
            return;
        }
        self.hide_preview();
        self.sidebar.hover = Some(target.clone());
        let seq = self.sidebar.hover_seq;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(PREVIEW_DELAY).await;
            let _ = this.update(cx, |this, cx| this.preview_due(seq, target, cx));
        }).detach();
        cx.notify();
    }

    pub(super) fn row_pointer(&mut self, y: f32) { self.sidebar.pointer_y = y; }

    pub(super) fn hide_preview(&mut self) {
        self.sidebar.hover_seq += 1;
        (self.sidebar.hover, self.sidebar.preview) = (None, None);
    }

    fn preview_due(&mut self, seq: u64, target: Target, cx: &mut Context<Self>) {
        if seq != self.sidebar.hover_seq { return; }
        if let Some((text, _)) = self.sidebar.cache.get(&target).filter(|(_, at)| at.elapsed() < PREVIEW_TTL).cloned() {
            self.show_preview(target, text, cx);
            return;
        }
        let Some(api) = self.machine_api(&target.server) else { return };
        let tell = self.sidebar_tell();
        self.runtime.spawn(async move {
            let result = api.history(&target.name, PREVIEW_TAIL, None).await.map(|h| h.events.unwrap_or_default().into_iter().rev()
                .find(|e| e.kind == "assistant_msg" && e.text.as_deref().is_some_and(|t| !t.is_empty()))
                .and_then(|e| e.text).unwrap_or_default());
            tell.send(SidebarReply::Preview(seq, target, result)).await;
        });
    }

    fn show_preview(&mut self, target: Target, text: String, cx: &mut Context<Self>) {
        // Texto vazio não abre, como o web.
        if text.trim().is_empty() { return; }
        let view = cx.new(|cx| TextViewState::markdown(&safe_markdown(&text), cx));
        self.sidebar.preview = Some((target, view, self.sidebar.pointer_y));
        cx.notify();
    }

    // ── Pressionar para renomear ──

    pub(super) fn row_press(&mut self, target: Target, window: &mut Window, cx: &mut Context<Self>) {
        self.sidebar.press_seq += 1;
        self.sidebar.long_pressed = false;
        let seq = self.sidebar.press_seq;
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(PRESS).await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.sidebar.press_seq != seq { return; }
                this.sidebar.long_pressed = true;
                this.start_session_rename(target, window, cx);
            });
        }).detach();
    }

    pub(super) fn row_release(&mut self) { self.sidebar.press_seq += 1; }

    /// O clique que termina um pressionar longo não abre a sessão.
    pub(super) fn take_long_press(&mut self) -> bool { std::mem::take(&mut self.sidebar.long_pressed) }

    // ── Menu ──

    /// Abrir o menu lê o silenciar daquela sessão; resposta de um menu anterior é descartada pelo número.
    pub(super) fn start_menu(&mut self, target: Target, cx: &mut Context<Self>) {
        self.hide_preview();
        self.sidebar.press_seq += 1;
        self.sidebar.menu_seq += 1;
        let seq = self.sidebar.menu_seq;
        // Branches só onde o menu mostra o git (pasta num repositório); como o silenciar, lidas a cada abertura do menu.
        // A sessão da outra pessoa num par não tem esses itens: o servidor dela recusaria as leituras.
        let read_only = self.target_session(&target).is_some_and(SessionInfo::read_only);
        let git = !read_only && self.target_session(&target).is_some_and(has_git);
        // Convite não tem Silenciar: as preferências de aviso são do servidor inteiro, fora do convite (web: `if (invite) return`).
        let mute = !read_only && !self.invite_target(&target);
        let session = self.target_session(&target).cloned();
        let moves = mute && session.as_ref().is_some_and(|s| s.provider == "claude"
            && (s.conta.as_deref().is_some_and(|c| c.starts_with("claude:")) || s.uses_engine_account()
                || s.engine.as_deref().is_some_and(|engine| !engine.is_empty())));
        let Some(api) = self.machine_api(&target.server) else {
            let failed = self.machine_error(&target.server);
            self.sidebar.menu = Some(MenuRead { target, seq, mute: Mute::Failed(failed.clone()), branches: git.then_some(Branches::Failed(failed.clone())),
                accounts: moves.then_some(Accounts::Failed(failed)) });
            return;
        };
        self.sidebar.menu = Some(MenuRead { target: target.clone(), seq, mute: Mute::Loading, branches: git.then_some(Branches::Loading),
            accounts: moves.then_some(Accounts::Loading) });
        if moves && let Some(session) = session {
            let (api, tell) = (api.clone(), self.sidebar_tell());
            self.runtime.spawn(async move { tell.send(SidebarReply::AccountsRead(seq, read_account_targets(&api, &session).await)).await; });
        }
        if git {
            let (api, tell, name) = (api.clone(), self.sidebar_tell(), target.name.clone());
            self.runtime.spawn(async move { tell.send(SidebarReply::BranchRead(seq, api.read(&name, &["branches"], &[], 30).await)).await; });
        }
        if mute {
            let tell = self.sidebar_tell();
            self.runtime.spawn(async move { tell.send(SidebarReply::MuteRead(seq, api.server_read(&["push", "settings"], &[], 15).await)).await; });
        }
        cx.notify();
    }

    pub(super) fn button_menu(&mut self, target: Target, open: bool, cx: &mut Context<Self>) {
        if open { self.start_menu(target.clone(), cx); self.sidebar.button_menu = Some(target); }
        else if self.sidebar.button_menu.as_ref() == Some(&target) { self.sidebar.button_menu = None; cx.notify(); }
    }

    pub(super) fn start_session_rename(&mut self, target: Target, window: &mut Window, cx: &mut Context<Self>) {
        if !self.target_session(&target).is_some_and(|s| !s.orq()) { return; }
        // No trilho a linha não tem onde mostrar o campo: vai ao diálogo, como o web com a barra recolhida.
        let inline = appearance::get().navigation == appearance::Navigation::Sidebar && !self.rail();
        let name = target.name.clone();
        let input = cx.new(|cx| InputState::new(window, cx).default_value(name.clone()));
        let old = target.clone();
        let events = cx.subscribe_in(&input, window, move |this, _, event: &InputEvent, window, cx| match event {
            InputEvent::PressEnter { .. } => this.commit_session_rename(window, cx),
            // Sair do campo salva, como o blur do web; no diálogo quem decide são os botões.
            InputEvent::Blur if this.sidebar.editing.as_ref().is_some_and(|e| e.inline && e.target == old) => this.commit_session_rename(window, cx),
            // O diálogo é desenhado com a janela: refazê-la liga e desliga o Renomear enquanto se digita.
            InputEvent::Change => cx.notify(),
            _ => {}
        });
        // Foco e seleção no quadro seguinte: o campo ainda não está na tela, e o diálogo toma o foco ao abrir.
        let field = input.clone();
        cx.defer_in(window, move |_, window, cx| field.update(cx, |state, cx| { state.focus(window, cx); state.select_all(window, cx); }));
        self.hide_preview();
        let status = Rc::new(RefCell::new(Pending::default()));
        self.sidebar.editing = Some(Edit { target: target.clone(), input: input.clone(), inline, status: status.clone(), _events: events });
        if !inline {
            // O diálogo devolve ao fechar o foco de quando abriu: o do menu morre com ele, então a aba de origem vem antes.
            self.focus_origin(&target, window, cx);
            let weak = cx.entity().downgrade();
            let owner = target;
            window.open_dialog(cx, move |dialog, window, cx| {
                let value = input.read(cx).value().trim().to_owned();
                let (busy, error) = { let s = status.borrow(); (s.sent.is_some(), s.error.clone()) };
                let (commit, cancel, close, owner) = (weak.clone(), weak.clone(), weak.clone(), owner.clone());
                // Vazio ou igual ao atual não renomeia, e gravando não manda de novo: o botão fica desligado, como o do web.
                let rename = Button::new("rename-ok").primary().label(tr("sidebar_rename")).disabled(busy || value.is_empty() || value == owner.name)
                    .on_click(move |_, window, cx| {
                    let _ = commit.update(cx, |this, cx| this.commit_session_rename(window, cx));
                });
                let forget = move |weak: &WeakEntity<Hangar>, owner: &Target, cx: &mut App| { let _ = weak.update(cx, |this, cx| {
                    if this.sidebar.editing.as_ref().is_some_and(|e| e.target == *owner) { this.sidebar.editing = None; cx.notify(); }
                }); };
                let cancel_owner = owner.clone();
                // Borda vermelha liga o erro ao campo, como o `aria-invalid` do web; o anel de foco do kit a cobriria, então sai
                // enquanto o erro está à vista (o cursor segue mostrando o foco).
                let field = Input::new(&input).aria_label(tr("sidebar_new_name"))
                    .when(error.is_some(), |el| el.focus_bordered(false).border_color(theme::danger()));
                // Curto como as confirmações: no meio da janela, pela altura de título, campo e botões.
                popup::dialog(dialog).w(px(420.)).margin_top(popup::centered_top(window.viewport_size().height, px(170.)))
                    .title(tr("sidebar_rename_title")).child(field)
                    .when_some(error, |dialog, error| dialog.child(div().id("rename-error").role(Role::Alert).mt(px(6.)).text_sm().text_color(theme::danger()).child(error)))
                    .footer(div().flex().justify_end().gap_2()
                        .child(Button::new("rename-cancel").label(tr("cancel")).on_click(move |_, window, cx| {
                            forget(&cancel, &cancel_owner, cx);
                            window.close_dialog(cx);
                        }))
                        .child(rename))
                    .on_ok(enter_to_focused)
                    .on_close(move |_, _, cx| forget(&close, &owner, cx))
            });
        }
        cx.notify();
    }

    pub(super) fn cancel_session_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sidebar.editing.take().is_some() { self.root_focus.focus(window, cx); cx.notify(); }
    }

    fn commit_session_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(edit) = self.sidebar.editing.as_ref() else { return };
        let new = edit.input.read(cx).value().trim().to_owned();
        let unchanged = new.is_empty() || new == edit.target.name;
        let old = edit.target.clone();
        let offline = self.machine_api(&old.server).is_none().then(|| self.machine_error(&old.server));
        let Some(edit) = self.sidebar.editing.as_mut() else { return };
        if edit.inline {
            // Na linha o campo fecha na hora e a falha vai à notificação, como o `saveEdit` do web.
            self.sidebar.editing = None;
            self.root_focus.focus(window, cx);
            cx.notify();
            if unchanged { return; }
            if let Some(reason) = offline { window.push_notification(Notification::error(tr("sidebar_rename_failed").replace("{n}", &reason)), cx); return; }
        } else {
            // No diálogo, vazio ou igual não faz nada (o web só envia válido), e gravando não manda de novo. Ele fica aberto
            // com o botão ocupado até a resposta, que fecha ou mostra o motivo junto do campo.
            let mut status = edit.status.borrow_mut();
            if unchanged || status.sent.is_some() { return; }
            if let Some(reason) = offline {
                *status = Pending { sent: None, error: Some(tr("sidebar_rename_failed").replace("{n}", &reason)) };
                drop(status);
                cx.notify();
                return;
            }
            *status = Pending { sent: Some(self.sidebar.rename_seq + 1), error: None };
        }
        self.sidebar.rename_seq += 1;
        self.sidebar.renaming.insert(old.clone());
        self.write(old, Write::Rename(new, self.sidebar.rename_seq), cx);
    }

    /// Grava na máquina da linha; sem a conexão dela a falha volta pelo mesmo caminho da resposta, nunca calada.
    /// Fechar: a linha some na hora e volta se o servidor recusar (`Wrote`).
    pub(super) fn delete_target(&mut self, target: Target, cx: &mut Context<Self>) {
        self.sidebar.deleting.insert(target.clone());
        self.write(target, Write::Delete, cx);
    }

    fn write(&mut self, target: Target, what: Write, cx: &mut Context<Self>) {
        let api = self.machine_api(&target.server).ok_or_else(|| self.machine_error(&target.server));
        let tell = self.sidebar_tell();
        let editor = matches!(what, Write::Editor).then(|| self.local_editor(api.clone().ok(), &target.name)).flatten();
        self.runtime.spawn(async move {
            let api = match api {
                Ok(api) => api,
                Err(reason) => { tell.send(SidebarReply::Wrote(target, what, Err(Failure::local(reason)))).await; return; }
            };
            let name = &target.name;
            let result = match &what {
                Write::Mute(muted) => api.server_send(reqwest::Method::POST, &["push", "mute"], Some(json!({"session": name, "muted": muted})), 15).await,
                Write::Editor => match editor { Some(open) => open.await, None => api.act(name, &["open-editor"], None, false, 15).await },
                Write::Delete => api.act(name, &[], None, true, 30).await,
                Write::Rename(new, _) => api.act(name, &["rename"], Some(json!({"new": new})), false, 30).await,
                Write::Mode(terminal) => api.act(name, &["modo-execucao"], Some(json!({"terminal": terminal})), false, 60).await,
                Write::Account { path, engine_account, .. } => api.act(name, &["conta"], Some(account_body(path, engine_account.as_deref())), false, 120).await,
            };
            tell.send(SidebarReply::Wrote(target, what, result)).await;
        });
        cx.notify();
    }

    // ── Git e encadear ──

    /// "Git": a aba Git do painel da direita (o diálogo quando o painel não cabe), o mesmo caminho da faixa do compositor.
    fn open_git(&mut self, target: Target, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_target().as_ref() != Some(&target) && self.select_target(&target, window, cx).is_none() { return; }
        self.open_git_panel(window, cx);
    }

    /// Terminal ⇄ sem terminal na mesma conversa: reinicia o processo da sessão, então pergunta antes, como o web.
    pub(super) fn confirm_mode(&mut self, target: Target, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.target_session(&target) else { return };
        let terminal = session.headless;
        let label = web(if terminal { "modo_abrir_no_terminal" } else { "modo_continuar_sem_terminal" });
        let message = web(if terminal { "modo_confirmar_terminal_msg" } else { "modo_confirmar_sem_terminal_msg" });
        self.focus_origin(&target, window, cx);
        let this = cx.entity().downgrade();
        chrome::confirm_alert(window, cx, label.clone(), message, label, ButtonVariant::Primary,
            move |_, cx| { let _ = this.update(cx, |this, cx| this.write(target.clone(), Write::Mode(terminal), cx)); true });
    }

    /// A mesma conversa noutra conta: reinicia o processo da sessão, então pergunta antes, como o modo.
    /// `warn`: % da conta que está acabando; a confirmação diz isso antes de quem escolhe aceitar.
    pub(super) fn confirm_account(&mut self, target: Target, path: String, label: String, warn: Option<f64>, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm_account_choice(target, path, None, label, warn, window, cx);
    }

    pub(super) fn confirm_engine_account(&mut self, target: Target, account: String, label: String, warn: Option<f64>, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm_account_choice(target, String::new(), Some(account), label, warn, window, cx);
    }

    fn confirm_account_choice(&mut self, target: Target, path: String, engine_account: Option<String>, label: String,
        warn: Option<f64>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.target_session(&target).filter(|s| s.provider == "claude" && !s.read_only() && s.state == "idle") else { return; };
        if self.sidebar.moving.contains_key(&target) { return; }
        let returning = engine_account.is_none() && session.engine.as_deref().is_some_and(|e| !e.is_empty());
        let life = session.lifecycle_id.clone();
        let jsonl = session.jsonl.clone();
        self.focus_origin(&target, window, cx);
        let this = cx.entity().downgrade();
        let title = tr(if returning { "sidebar_return_claude_title" } else if engine_account.is_some() { "sidebar_proxy_title" } else { "sidebar_same_title" }).replace("{n}", &label);
        let message = tr(if returning { "sidebar_return_claude_msg" } else if engine_account.is_some() { "sidebar_proxy_msg" } else { "sidebar_same_msg" }).replace("{n}", &label);
        let message = match warn {
            Some(pct) => format!("{}\n\n{message}", tr("sidebar_same_low").replace("{n}", &label).replace("{pct}", &format!("{pct:.0}"))),
            None => message,
        };
        chrome::confirm_alert(window, cx, title, message, tr("sidebar_same_ok"), ButtonVariant::Primary,
            move |window, cx| {
                let _ = this.update(cx, |this, cx| {
                    if !this.target_session(&target).is_some_and(|s| s.provider == "claude" && !s.read_only() && s.state == "idle"
                        && s.lifecycle_id == life && s.jsonl == jsonl) || this.sidebar.moving.contains_key(&target) {
                        window.push_notification(Notification::warning(tr("sidebar_account_changed")), cx);
                        return;
                    }
                    window.push_notification(Notification::info(tr("sidebar_same_moving").replace("{n}", &label)), cx);
                    this.sidebar.moving.insert(target.clone(), (std::time::Instant::now(), None));
                    this.write(target.clone(), Write::Account { path: path.clone(), label: label.clone(), engine_account: engine_account.clone() }, cx)
                });
                true
            });
    }

    fn git_write(&mut self, target: Target, what: GitWrite, window: &mut Window, cx: &mut Context<Self>) {
        let name = target.name.clone();
        let Some(api) = self.machine_api(&target.server) else {
            window.push_notification(git_note(&name, NotificationType::Error, self.machine_error(&target.server)), cx);
            return;
        };
        let waiting = match &what {
            GitWrite::Pull => Some(tr("sidebar_pulling")),
            GitWrite::Checkout(branch) => Some(tr("sidebar_checking_out").replace("{n}", branch)),
            GitWrite::StashCheckout(_) => Some(tr("sidebar_stashing")),
            GitWrite::Unlink => None,
        };
        if let Some(text) = waiting { window.push_notification(git_note(&name, NotificationType::Info, text), cx); }
        let tell = self.sidebar_tell();
        self.runtime.spawn(async move {
            let (kind, text) = git_result(&api, &name, what).await;
            tell.send(SidebarReply::Note(target, kind, text)).await;
        });
    }

    /// Branch com a árvore suja pergunta antes, como o web; limpa troca direto.
    fn pick_branch(&mut self, target: Target, branch: String, dirty: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !dirty { self.git_write(target, GitWrite::Checkout(branch), window, cx); return; }
        self.focus_origin(&target, window, cx);
        let weak = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            let run = |id: &'static str, label: String, stash: bool| {
                let (weak, target, branch) = (weak.clone(), target.clone(), branch.clone());
                Button::new(id).label(label).when(stash, |b| b.primary()).on_click(move |_, window, cx| {
                    window.close_dialog(cx);
                    let what = if stash { GitWrite::StashCheckout(branch.clone()) } else { GitWrite::Checkout(branch.clone()) };
                    let _ = weak.update(cx, |this, cx| this.git_write(target.clone(), what, window, cx));
                })
            };
            let line = |label: String, text: String| div().child(div().font_weight(FontWeight::SEMIBOLD).child(label))
                .child(div().text_color(theme::muted()).child(text));
            popup::dialog(dialog).w(px(460.)).title(tr("sidebar_dirty_title"))
                .child(div().flex().flex_col().gap(px(10.)).text_sm()
                    .child(div().font_family(theme::MONO).child(format!("→ {branch}")))
                    .child(div().child(tr("sidebar_dirty_body")))
                    .child(line(tr("sidebar_stash_and_switch"), tr("sidebar_stash_help")))
                    .child(line(tr("sidebar_switch_anyway"), tr("sidebar_carry_help"))))
                .footer(div().flex().justify_end().gap_2()
                    .child(Button::new("dirty-cancel").label(tr("cancel")).on_click(|_, window, cx| window.close_dialog(cx)))
                    .child(run("dirty-anyway", tr("sidebar_switch_anyway"), false))
                    .child(run("dirty-stash", tr("sidebar_stash_and_switch"), true)))
                .on_ok(enter_to_focused)
        });
    }

    fn start_chain(&mut self, from: Target, target: String, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(tr("sidebar_chain_prompt")));
        let events = cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| match event {
            InputEvent::PressEnter { .. } => this.commit_chain(window, cx),
            // O diálogo é desenhado com a janela: refazê-la liga e desliga o Salvar enquanto se digita.
            InputEvent::Change => cx.notify(),
            _ => {}
        });
        let field = input.clone();
        cx.defer_in(window, move |_, window, cx| field.update(cx, |state, cx| state.focus(window, cx)));
        let status = Rc::new(RefCell::new(Pending::default()));
        self.sidebar.chain = Some(Chain { from: from.clone(), target: target.clone(), input: input.clone(), status: status.clone(), _events: events });
        self.focus_origin(&from, window, cx);
        let weak = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, cx| {
            let empty = input.read(cx).value().trim().is_empty();
            let (busy, error) = { let s = status.borrow(); (s.sent.is_some(), s.error.clone()) };
            let (save, close, owner) = (weak.clone(), weak.clone(), status.clone());
            let field = Input::new(&input).aria_label(tr("sidebar_chain_prompt_aria"))
                .when(error.is_some(), |el| el.focus_bordered(false).border_color(theme::danger()));
            popup::dialog(dialog).w(px(460.)).title(tr("sidebar_chain_title").replace("{n}", &target)).child(field)
                .when_some(error, |dialog, error| dialog.child(div().id("chain-error").role(Role::Alert).mt(px(6.)).text_sm().text_color(theme::danger()).child(error)))
                .footer(div().flex().justify_end().gap_2()
                    .child(Button::new("chain-cancel").label(tr("cancel")).on_click(|_, window, cx| window.close_dialog(cx)))
                    .child(Button::new("chain-save").primary().label(tr("sidebar_save")).loading(busy).disabled(busy || empty)
                        .on_click(move |_, window, cx| { let _ = save.update(cx, |this, cx| this.commit_chain(window, cx)); })))
                .on_ok(enter_to_focused)
                // Fechar sem esperar a resposta a esquece: ela vai à notificação e não mexe num diálogo aberto depois.
                .on_close(move |_, _, cx| { let _ = close.update(cx, |this, _| {
                    if this.sidebar.chain.as_ref().is_some_and(|c| Rc::ptr_eq(&c.status, &owner)) { this.sidebar.chain = None; }
                }); })
        });
        cx.notify();
    }

    fn commit_chain(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(chain) = self.sidebar.chain.as_ref() else { return };
        let text = chain.input.read(cx).value().trim().to_owned();
        if text.is_empty() || chain.status.borrow().sent.is_some() { return; }
        let Some(api) = self.machine_api(&chain.from.server) else {
            *chain.status.borrow_mut() = Pending { sent: None, error: Some(tr("sidebar_chain_failed").replace("{n}", &self.machine_error(&chain.from.server))) };
            cx.notify();
            return;
        };
        self.sidebar.chain_seq += 1;
        let seq = self.sidebar.chain_seq;
        *chain.status.borrow_mut() = Pending { sent: Some(seq), error: None };
        let (from, target, tell) = (chain.from.clone(), chain.target.clone(), self.sidebar_tell());
        self.runtime.spawn(async move {
            let result = api.server_send(reqwest::Method::PUT, &["sessions", &from.name, "then"], Some(json!({"target": target, "text": text})), 30).await;
            tell.send(SidebarReply::Chained(from, seq, target, result)).await;
        });
        cx.notify();
    }

    /// Foco na linha/aba de onde o menu saiu (o `menuOrigem` do web), para o diálogo devolvê-lo ao fechar.
    pub(super) fn focus_origin(&self, target: &Target, window: &mut Window, cx: &mut Context<Self>) {
        self.row_focus(target).unwrap_or(&self.root_focus).focus(window, cx);
    }

    fn confirm_delete(&mut self, target: Target, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_origin(&target, window, cx);
        let this = cx.entity().downgrade();
        chrome::confirm_alert(window, cx, tr("sidebar_close_title"), target.name.clone(), tr("sidebar_close"), ButtonVariant::Danger,
            move |window, cx| { let _ = this.update(cx, |this, cx| {
                    // A linha some na hora e volta se o servidor recusar; sem conexão nada sai, ela não some e o motivo aparece.
                    if this.machine_api(&target.server).is_none() {
                        window.push_notification(Notification::error(tr("sidebar_close_failed").replace("{n}", &this.machine_error(&target.server))), cx);
                        return;
                    }
                    this.delete_target(target.clone(), cx);
                    // O Confirm fecha com animação e só então devolve o foco à linha, que já sumiu: passado esse prazo, a raiz
                    // o recebe (`fallbackFocus` do web). Se a linha voltou (o servidor recusou), o foco fica nela.
                    let target = target.clone();
                    cx.spawn_in(window, async move |this, cx| {
                        cx.background_executor().timer(*dialog::ANIMATION_DURATION + Duration::from_millis(50)).await;
                        let _ = this.update_in(cx, |this, window, cx| {
                            let gone = this.sidebar.deleting.contains(&target) || this.target_session(&target).is_none();
                            let on_row = this.row_focus(&target).is_some_and(|f| f.is_focused(window));
                            if gone && (on_row || window.focused(cx).is_none()) { this.root_focus.focus(window, cx); }
                        });
                    }).detach();
                }); true });
    }

    pub(super) fn receive_sidebar(&mut self, reply: SidebarReply, window: &mut Window, cx: &mut Context<Self>) {
        match reply {
            SidebarReply::NotSaved(error) => {
                eprintln!("grupos recolhidos não gravaram: {error}");
                window.push_notification(Notification::warning(tr("sidebar_collapse_not_saved").replace("{n}", &error)), cx);
            }
            SidebarReply::Preview(seq, target, result) => {
                // Espiada é opcional: falha não vira erro na tela (como o web), só no log.
                let text = match result { Ok(text) => text, Err(error) => { eprintln!("prévia de {}: {}", target.id(), error.detail); return; } };
                self.sidebar.cache.insert(target.clone(), (text.clone(), Instant::now()));
                if seq == self.sidebar.hover_seq && self.sidebar.hover.as_ref() == Some(&target) { self.show_preview(target, text, cx); }
            }
            SidebarReply::MuteRead(seq, result) => {
                let Some(menu) = self.sidebar.menu.as_mut().filter(|m| m.seq == seq) else { return };
                menu.mute = match result {
                    Ok(value) => match value.get("muted").and_then(Value::as_array) {
                        Some(list) => Mute::Known(list.iter().any(|v| v.as_str() == Some(menu.target.name.as_str()))),
                        None => Mute::Failed(tr("invalid_response")),
                    },
                    Err(error) => Mute::Failed(Self::fetch_failure(&error)),
                };
                cx.notify();
            }
            SidebarReply::BranchRead(seq, result) => {
                let Some(menu) = self.sidebar.menu.as_mut().filter(|m| m.seq == seq) else { return };
                menu.branches = Some(match result {
                    Ok(value) => serde_json::from_value::<BranchList>(value).map(Branches::Known)
                        .unwrap_or_else(|_| Branches::Failed(tr("invalid_response"))),
                    Err(error) => Branches::Failed(Self::fetch_failure(&error)),
                });
                cx.notify();
            }
            SidebarReply::AccountsRead(seq, result) => {
                let Some(menu) = self.sidebar.menu.as_mut().filter(|m| m.seq == seq) else { return };
                menu.accounts = Some(match result {
                    Ok((list, notice)) => Accounts::Known(list, notice),
                    Err(error) => Accounts::Failed(Self::fetch_failure(&error)),
                });
                cx.notify();
            }
            SidebarReply::Note(target, kind, text) => window.push_notification(git_note(&target.name, kind, text), cx),
            SidebarReply::Group(seq, reply) => self.receive_group(seq, reply, window, cx),
            SidebarReply::Sheet(reply) => self.receive_sheet(reply, window, cx),
            SidebarReply::Chained(from, seq, target, result) => {
                // Só o diálogo que mandou este pedido recebe a resposta; fechado, ela vai à notificação.
                let open = self.sidebar.chain.as_ref().filter(|c| c.status.borrow().sent == Some(seq));
                match (result, open) {
                    (Ok(_), open) => {
                        if open.is_some() { self.sidebar.chain = None; window.close_dialog(cx); }
                        window.push_notification(git_note(&from.name, NotificationType::Success, tr("sidebar_chained").replace("{n}", &target)), cx);
                    }
                    (Err(error), Some(chain)) => *chain.status.borrow_mut() = Pending { sent: None,
                        error: Some(tr("sidebar_chain_failed").replace("{n}", &Self::fetch_failure(&error))) },
                    (Err(error), None) => window.push_notification(git_note(&from.name, NotificationType::Error,
                        tr("sidebar_chain_failed").replace("{n}", &Self::fetch_failure(&error))), cx),
                }
                cx.notify();
            }
            SidebarReply::Wrote(target, what, result) => {
                // Com status, o motivo do servidor; sem resposta, "falha na conexão". Nunca o texto de entrega de conversa, que o
                // `failure` dá a um 5xx de POST.
                let failed = |key: &str, error: &Failure| tr(key).replace("{n}", &Self::fetch_failure(error));
                if matches!(what, Write::Account { .. }) {
                    if self.selected_target().as_ref() == Some(&target) {
                        // A lista que chegou durante a troca ficou de lado: agora ela decide (transcript novo ou sumiço).
                        if let Some(entry) = self.sidebar.moving.get_mut(&target) { entry.1 = Some(std::time::Instant::now()); }
                        let list = self.sessions_of(&target.server).to_vec();
                        self.follow_open(&list, window, cx);
                        // A lista pode não mudar mais: passado o prazo, confere de novo para não deixar a conversa congelada.
                        let again = target.clone();
                        cx.spawn_in(window, async move |this, cx| {
                            cx.background_executor().timer(Duration::from_secs(16)).await;
                            let _ = this.update_in(cx, |this, window, cx| {
                                if this.sidebar.moving.contains_key(&again) && this.selected_target().as_ref() == Some(&again) {
                                    let list = this.sessions_of(&again.server).to_vec();
                                    this.follow_open(&list, window, cx);
                                }
                            });
                        }).detach();
                    } else {
                        self.sidebar.moving.remove(&target);
                    }
                }
                // Diálogo desta sessão ainda aberto (não cancelado): o resultado aparece nele, não em notificação solta.
                let in_dialog = match what {
                    Write::Rename(_, seq) => self.sidebar.editing.as_ref().is_some_and(|e| !e.inline && e.status.borrow().sent == Some(seq) && e.target == target),
                    _ => false,
                };
                let note = match (&what, result) {
                    (Write::Rename(new, _), Ok(value)) => {
                        self.sidebar.renaming.remove(&target);
                        let new = Target::new(&target.server, value.get("name").and_then(Value::as_str).unwrap_or(new));
                        if in_dialog {
                            self.sidebar.editing = None;
                            window.close_dialog(cx);
                            // Foco na linha/aba do nome novo; ainda fora da lista, ela o recebe quando aparecer.
                            match self.row_focus(&new).cloned() {
                                Some(focus) => focus.focus(window, cx),
                                None => { self.root_focus.focus(window, cx); self.sidebar.focus_tab = Some(new.clone()); }
                            }
                        }
                        let lost = self.sidebar.lost.as_ref() == Some(&target);
                        if lost { self.sidebar.lost = None; }
                        // Só a aberta nesta máquina segue o nome novo: a de mesmo nome em outra fica onde está.
                        let open_here = self.selected_target().as_ref() == Some(&target);
                        if (open_here || (lost && self.selected.is_none())) && self.select_target(&new, window, cx).is_none() {
                            self.sidebar.follow = Some(new);
                        }
                        None
                    }
                    (Write::Rename(..), Err(error)) => {
                        self.sidebar.renaming.remove(&target);
                        // Sumiu da lista e o renomear falhou: aí sim ela foi encerrada.
                        if self.sidebar.lost.as_ref() == Some(&target) {
                            self.sidebar.lost = None;
                            if self.selected.is_none() { self.error = Some(tr("session_gone")); }
                        }
                        match self.sidebar.editing.as_ref().filter(|_| in_dialog) {
                            Some(edit) => { *edit.status.borrow_mut() = Pending { sent: None, error: Some(failed("sidebar_rename_failed", &error)) }; None }
                            None => Some(Notification::error(failed("sidebar_rename_failed", &error))),
                        }
                    }
                    (Write::Mute(muted), Ok(_)) => Some(Notification::success(tr(if *muted { "sidebar_muted" } else { "sidebar_unmuted" }))),
                    (Write::Mute(_), Err(error)) => Some(Notification::error(failed("sidebar_mute_failed", &error))),
                    (Write::Editor, Ok(_)) => None,
                    (Write::Editor, Err(error)) => Some(Notification::error(failed("sidebar_editor_failed", &error))),
                    (Write::Mode(_), Ok(_)) => None,
                    (Write::Mode(_), Err(error)) => Some(Notification::error(failed("sidebar_mode_failed", &error))),
                    (Write::Account { label, .. }, Ok(_)) => Some(Notification::success(tr("sidebar_same_moved").replace("{n}", label))),
                    (Write::Account { .. }, Err(error)) => Some(Notification::error(failed("sidebar_same_failed", &error))),
                    (Write::Delete, Ok(value)) => {
                        // Fechou, mas um par do grupo não foi avisado: o motivo aparece em vez de fechar mudo.
                        value.get("warning").filter(|w| !w.is_null()).map(|w| Notification::warning(
                            w.get("msg").and_then(Value::as_str).map(str::to_owned).unwrap_or_else(|| w.to_string())))
                    }
                    (Write::Delete, Err(error)) => {
                        self.sidebar.deleting.remove(&target);
                        Some(Notification::error(failed("sidebar_close_failed", &error)))
                    }
                };
                if let Some(note) = note { window.push_notification(note, cx); }
                cx.notify();
            }
        }
    }

    // ── Desenho ──

    /// Cabeçalho do grupo de projeto: ▾, rótulo, contagem e quantos aguardam; clicar recolhe. O `AccordionTrigger` anuncia
    /// aberto/fechado e entra no Tab com Enter e Espaço, como os disparadores das configurações.
    pub(super) fn render_group_header(&self, group: &Group, awaiting: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let open = !self.sidebar.collapsed.contains(&group.key);
        let id = SharedString::from(format!("group-{}", group.key));
        let focus = window.use_keyed_state(SharedString::from(format!("{id}-focus")), cx, |_, cx| cx.focus_handle().tab_stop(true)).read(cx).clone();
        let (key, weak) = (group.key.clone(), cx.entity().downgrade());
        AccordionTrigger::new(id).open(open).track_focus(&focus).aria_label(format!("{} · {}", group.label, group.sessions.len()))
            .flex_shrink_0().mt(px(8.)).h(px(28.)).px(px(8.)).rounded(px(8.)).border_1().border_color(transparent_black())
            .flex().items_center().gap(px(6.)).cursor_pointer().hover(|el| el.bg(theme::hover())).focus_visible(|el| el.border_color(theme::accent_focus()))
            .child(chrome::small_icon(if open { IconName::ChevronDown } else { IconName::ChevronRight }, 14., theme::faint()))
            .child(div().min_w_0().truncate().text_xs().font_weight(FontWeight::MEDIUM).text_color(theme::muted()).child(group.label.clone()))
            .child(div().flex_shrink_0().font_family(theme::MONO).text_size(px(11.)).text_color(theme::faint()).child(group.sessions.len().to_string()))
            .when(awaiting > 0, |el| el.child(div().flex_shrink_0().font_family(theme::MONO).text_size(px(11.)).text_color(theme::warning())
                .child(format!("{awaiting} {}", tr("sidebar_awaiting_short")))))
            .on_change(move |_, _, _, cx| { let _ = weak.update(cx, |this, cx| this.toggle_group(key.clone(), cx)); })
            .into_any_element()
    }

    /// Escopo "Todas as sessões" com o seletor Agrupar (no web mora no popover do ⋯ do topo, que o nativo não tem).
    pub(super) fn render_group_picker(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let project = Self::by_project();
        let weak = cx.entity().downgrade();
        Button::new("sidebar-group").ghost().xsmall().icon(IconName::Layers)
            .tooltip(tr("sidebar_group_by")).accessibility_label(tr("sidebar_group_by"))
            .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
                let pick = |label: &str, value: bool| {
                    let weak = weak.clone();
                    PopupMenuItem::new(tr(label)).checked(project == value).on_click(move |_, _, cx| {
                        let _ = weak.update(cx, |this, cx| this.set_group(value, cx));
                    })
                };
                menu_style(menu).label(tr("sidebar_group_by")).item(pick("sidebar_group_none", false)).item(pick("sidebar_group_project", true))
            })
    }

    pub(super) fn render_preview(&self, window: &Window) -> Option<AnyElement> {
        let (_, view, y) = self.sidebar.preview.as_ref()?;
        if appearance::get().navigation != appearance::Navigation::Sidebar || self.settings.is_some() { return None; }
        let left = 284. + 8. + if theme::is_floating() { 10. } else { 0. };
        let max_top = (f32::from(window.viewport_size().height) - PREVIEW_H - 8.).max(8.);
        Some(div().absolute().left(px(left)).top(px((y - 14.).clamp(8., max_top))).w(px(PREVIEW_W)).max_h(px(PREVIEW_H)).overflow_hidden()
            .px(px(14.)).py(px(10.)).rounded(px(12.)).bg(theme::elevated()).border_1().border_color(theme::border_strong())
            .shadow(theme::popover_shadow()).text_sm()
            .child(TextView::new(view).selectable(false).scrollable(false))
            .into_any_element())
    }
}

/// Apresentação da T37; teclado, foco e submenus continuam pertencendo ao kit.
pub(super) fn menu_style(menu: PopupMenu) -> PopupMenu {
    use gpui_kit::component::menu::PopupMenuAppearance;
    menu.appearance(PopupMenuAppearance::default()
        .surface_style(StyleRefinement::default().rounded(px(12.)).border_1().border_color(theme::glass_border())
            .bg(theme::raised()).text_color(theme::text()).shadow(theme::popover_shadow()))
        .list_style(StyleRefinement::default().p(px(popup::INSET)).gap(px(2.)))
        .item_height(px(28.25))
        .item_style(StyleRefinement::default().px(px(8.)).rounded(px(7.)).text_size(px(13.)).line_height(relative(1.25)))
        .separator_style(StyleRefinement::default().border_b_0().h(px(1.)).mx(px(-popup::INSET)).my(px(2.)).bg(theme::border()))
        .label_renderer(|label, _, _| popup::title(label.to_string(), Some("esc")).w_full())
        .key_renderer(|key, _, _| popup::key_hint("").child(key.appearance(false))))
}

/// O mesmo menu no clique direito da linha, no ⋯ e na aba, para a sessão de qualquer máquina (`server` é a chave dela).
pub(super) fn session_menu(hangar: WeakEntity<Hangar>, server: String, session: SessionInfo) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
    let target = Target::new(&server, &session.name);
    move |menu, window, cx| {
        let Some(entity) = hangar.upgrade() else { return menu };
        // Sem nome, processo nem turno próprios para renomear, fechar ou interromper: o menu do orquestrador só leva ao árbitro.
        if session.orq() {
            let missing = entity.read(cx).arbiter_of(&target).is_none();
            let (hangar, target) = (hangar.clone(), target.clone());
            return menu_style(menu).min_w(px(240.)).label(session.name.clone())
                .item(PopupMenuItem::new(tr_shared("orq_talk_to_arbiter", &[])).disabled(missing)
                    .on_click(move |_, window, cx| { let _ = hangar.update(cx, |this, cx| this.open_arbiter(&target, window, cx)); }));
        }
        // As outras sessões da mesma máquina, na ordem da barra sem o filtro, são as candidatas do encadear (web:
        // `chainCandidates`, que lê os grupos inteiros daquele servidor): o vínculo é resolvido pelo backend dela.
        let others = |hangar: &Hangar, target: &Target| -> Vec<String> {
            let l = layout(hangar.sessions_of(&target.server), "", Hangar::by_project(), &hangar.sidebar.hidden_on(&target.server));
            l.waiting.iter().chain(l.groups.iter().flat_map(|g| g.sessions.iter())).filter(|s| s.name != target.name).map(|s| s.name.clone()).collect()
        };
        let invite = entity.read(cx).invite_target(&target);
        // Convite não tem Silenciar, então nada a atualizar na posição dele.
        if !invite {
            let seen = RefCell::new(entity.read(cx).sidebar.menu_for(&target));
            let (weak, again) = (hangar.clone(), target.clone());
            cx.observe_in(&entity, window, move |menu, entity, _, cx| {
                let now = entity.read(cx).sidebar.menu_for(&again);
                if *seen.borrow() == now { return; }
                *seen.borrow_mut() = now.clone();
                // Atualiza só Silenciar: o submenu aberto mantém entidade, seleção e foco.
                menu.replace_item(2, mute_item(&weak, &again, now), cx);
            }).detach();
        }
        let view = entity.read(cx).sidebar.menu_for(&target);
        let list = others(entity.read(cx), &target);
        let group = entity.read(cx).group_candidates(&target);
        let active = entity.read(cx).is_active_key(&target.server);
        fill_menu(menu, &hangar, &target, &session, Access { invite, active }, view, list, group, window, cx)
    }
}

/// O que o menu de uma linha pode oferecer: convite só o que é da própria sessão; `active` diz se a máquina é a ativa.
struct Access { invite: bool, active: bool }

fn mute_item(hangar: &WeakEntity<Hangar>, target: &Target, mute: Option<Mute>) -> PopupMenuItem {
    match mute {
        Some(Mute::Known(muted)) => {
            let (hangar, target) = (hangar.clone(), target.clone());
            PopupMenuItem::new(tr(if muted { "sidebar_unmute" } else { "sidebar_mute" }))
                .on_click(move |_, _, cx| { let _ = hangar.update(cx, |this, cx| this.write(target.clone(), Write::Mute(!muted), cx)); })
        }
        // Sem leitura confirmada o item não grava: carregando mostra "…", falha mostra o motivo.
        Some(Mute::Failed(reason)) => PopupMenuItem::new(tr("sidebar_mute_unread").replace("{n}", &reason)).disabled(true),
        Some(Mute::Loading) | None => PopupMenuItem::new(format!("{}…", tr("sidebar_mute"))).disabled(true),
    }
}

#[allow(clippy::too_many_arguments)]
fn fill_menu(menu: PopupMenu, hangar: &WeakEntity<Hangar>, target: &Target, session: &SessionInfo, access: Access, mute: Option<Mute>,
    others: Vec<String>, group: Vec<String>, window: &mut Window, cx: &mut Context<PopupMenu>) -> PopupMenu {
    let item = |label: String, act: fn(&mut Hangar, Target, &mut Window, &mut Context<Hangar>)| {
        let (hangar, target) = (hangar.clone(), target.clone());
        PopupMenuItem::new(label).on_click(move |_, window, cx| { let _ = hangar.update(cx, |this, cx| act(this, target.clone(), window, cx)); })
    };
    let invite = access.invite;
    let cwd = session.cwd.clone().filter(|c| !c.is_empty());
    // A sessão da outra pessoa num par: o servidor dela recusa tudo que não é ler.
    if session.read_only() {
        return menu_style(menu).min_w(px(240.)).label(session.name.clone())
            .when_some(cwd, |menu, cwd| menu.item(PopupMenuItem::new(tr("sidebar_copy_cwd"))
                .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(cwd.clone())))));
    }
    // No convite o item de fechar vira "Parar de acompanhar": sai só deste aparelho, nunca fecha a sessão do dono. Com o convite
    // como servidor ativo ele não sai daqui (a troca de ativo é das configurações).
    let label = if invite { tr_shared("convite_parar", &[]) } else { tr("sidebar_close") };
    let close = {
        let (hangar, target) = (hangar.clone(), target.clone());
        PopupMenuItem::element(move |_, _| div().text_color(theme::danger()).child(label.clone())).disabled(invite && access.active)
            .on_click(move |_, window, cx| { let _ = hangar.update(cx, |this, cx| this.close_target(target.clone(), window, cx)); })
    };
    let git = has_git(session);
    let chain_label = match &session.then_target {
        Some(target) => tr("sidebar_chained_to").replace("{n}", target),
        None => tr("sidebar_chain"),
    };
    let (weak, chain_from, current) = (hangar.clone(), target.clone(), session.then_target.clone());
    let (group_weak, group_origin, leave) = (hangar.clone(), target.clone(), super::grouping::can_leave(session));
    // "Com resumo" abre o criar, travado na máquina dela; "A mesma conversa" leva esta conversa para a conta escolhida.
    let (baton_weak, baton_target, baton_cwd) = (hangar.clone(), target.clone(), session.cwd.clone());
    let (same_weak, same_target, idle) = (hangar.clone(), target.clone(), session.state == "idle");
    // Só Claude e Codex trocam de modo, e só parados (o backend responde 409 fora disso); o rótulo é o destino.
    let mode = matches!(session.provider.as_str(), "claude" | "codex").then(|| {
        let label = web(if session.headless { "modo_abrir_no_terminal" } else { "modo_continuar_sem_terminal" });
        let idle = session.state == "idle";
        let label = if idle { label } else { format!("{label} · {}", web("modo_so_ociosa")) };
        item(label, |this, target, window, cx| this.confirm_mode(target, window, cx)).disabled(!idle)
    });
    // O convite só alcança as rotas da própria sessão, como o menu do web: silenciar, editor, encadear, agrupar, modo e bastão
    // são do servidor inteiro ou mexem no processo do dono.
    menu_style(menu).min_w(px(240.)).label(session.name.clone())
        .item(item(tr("sidebar_rename"), |this, target, window, cx| this.start_session_rename(target, window, cx)))
        .when(!invite, |menu| menu.item(mute_item(hangar, target, mute))
            .item(item(tr_shared("compartilhar_menu", &[]), |this, target, window, cx| this.open_share_dialog(target, window, cx)))
            .item(item(tr("par_menu"), |this, target, window, cx| this.open_pair_accept_dialog(Some(target), None, window, cx))))
        .when_some(cwd, |menu, cwd| menu
            .item(PopupMenuItem::new(tr("sidebar_copy_cwd")).on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(cwd.clone()))))
            .when(!invite, |menu| menu.item(item(tr("sidebar_open_editor"), |this, target, _, cx| this.write(target, Write::Editor, cx)))))
        .when(git, |menu| {
            let (weak, target) = (hangar.clone(), target.clone());
            menu.separator()
                .item(item(tr("sidebar_git"), |this, target, window, cx| this.open_git(target, window, cx)))
                .item(item(tr("sidebar_git_pull"), |this, target, window, cx| this.git_write(target, GitWrite::Pull, window, cx)))
                .submenu(tr("sidebar_switch_branch"), window, cx, move |menu, window, cx| branch_menu(menu, &weak, &target, window, cx))
        })
        .when(!invite, |menu| menu.separator()
            .submenu(chain_label, window, cx, move |menu, _, _| fill_chain(menu, &weak, &chain_from, current.clone(), &others))
            // Mesmo diálogo do arrastar, para quem não arrasta (e o teclado).
            .submenu(tr("group_with"), window, cx, move |menu, _, _| super::grouping::fill_group(menu, &group_weak, &group_origin, &group))
            .when(leave, |menu| menu.item(item(tr("group_leave"), |this, target, window, cx| this.request_leave(target, window, cx))))
            .separator()
            .when_some(mode, |menu, mode| menu.item(mode))
            .submenu(tr("sidebar_baton"), window, cx, move |menu, window, cx| {
                let (hangar, target, cwd) = (baton_weak.clone(), baton_target.clone(), baton_cwd.clone());
                let menu = menu_style(menu).min_w(px(200.)).item(PopupMenuItem::new(tr("sidebar_baton_summary")).on_click(move |_, window, cx| {
                    let baton = super::create::Baton { name: target.name.clone(), cwd: cwd.clone(), server: target.server.clone() };
                    let _ = hangar.update(cx, |this, cx| {
                        this.focus_origin(&target, window, cx);
                        this.open_new_session(Some(baton), window, cx);
                    });
                }));
                // Sem leitura de contas a sessão não muda de conta (motor, Codex, outro agente): fica só o resumo.
                let shows = same_weak.upgrade().is_some_and(|e| e.read(cx).sidebar.accounts_for(&same_target).is_some());
                let (weak, target) = (same_weak.clone(), same_target.clone());
                menu.when(shows, |menu| menu.submenu(tr("sidebar_same"), window, cx, move |menu, window, cx| accounts_menu(menu, &weak, &target, idle, window, cx)))
            }))
        .separator()
        .item(close)
}

/// O submenu de branches se refaz sozinho quando a leitura chega: refazer o menu de cima fecharia o submenu aberto.
fn branch_menu(menu: PopupMenu, hangar: &WeakEntity<Hangar>, target: &Target, window: &mut Window, cx: &mut Context<PopupMenu>) -> PopupMenu {
    let Some(entity) = hangar.upgrade() else { return menu };
    let seen = RefCell::new(entity.read(cx).sidebar.branches_for(target));
    let now = seen.borrow().clone();
    let (weak, owner) = (hangar.clone(), target.clone());
    cx.observe_in(&entity, window, move |menu, entity, window, cx| {
        let now = entity.read(cx).sidebar.branches_for(&owner);
        if *seen.borrow() == now { return; }
        *seen.borrow_mut() = now.clone();
        let (weak, owner) = (weak.clone(), owner.clone());
        menu.rebuild(window, cx, move |menu, _, _| fill_branches(menu, &weak, &owner, now));
    }).detach();
    fill_branches(menu, hangar, target, now)
}

/// Submenu "Trocar branch": o estado da leitura feita ao abrir o menu; a atual com ✓ e sem ação.
fn fill_branches(menu: PopupMenu, hangar: &WeakEntity<Hangar>, target: &Target, branches: Option<Branches>) -> PopupMenu {
    let menu = menu_style(menu).min_w(px(240.)).label(tr("sidebar_switch_branch"));
    let list = match branches {
        Some(Branches::Known(list)) => list,
        // Falha em vermelho, não no cinza do "carregando…": é um estado, não uma espera.
        Some(Branches::Failed(reason)) => {
            let text = tr("sidebar_branches_failed").replace("{n}", &reason);
            return menu.item(PopupMenuItem::element(move |_, _| div().text_color(theme::danger()).child(text.clone())).disabled(true));
        }
        Some(Branches::Loading) | None => return menu.item(PopupMenuItem::new(tr("sidebar_loading")).disabled(true)),
    };
    if list.branches.is_empty() { return menu.item(PopupMenuItem::new(tr("sidebar_no_branches")).disabled(true)); }
    list.branches.iter().fold(menu.max_h(px(260.)).scrollable(true), |menu, branch| {
        let current = list.current.as_deref() == Some(branch.as_str());
        let (hangar, target, branch, dirty) = (hangar.clone(), target.clone(), branch.clone(), list.dirty);
        menu.item(mono_item(branch.clone(), current).on_click(move |_, window, cx| {
            if current { return; }
            let _ = hangar.update(cx, |this, cx| this.pick_branch(target.clone(), branch.clone(), dirty, window, cx));
        }))
    })
}

/// Submenu "A mesma conversa": refeito quando a leitura das contas chega, como o de branches.
fn accounts_menu(menu: PopupMenu, hangar: &WeakEntity<Hangar>, target: &Target, idle: bool, window: &mut Window, cx: &mut Context<PopupMenu>) -> PopupMenu {
    let Some(entity) = hangar.upgrade() else { return menu };
    let seen = RefCell::new(entity.read(cx).sidebar.accounts_for(target));
    let now = seen.borrow().clone();
    let (weak, owner) = (hangar.clone(), target.clone());
    cx.observe_in(&entity, window, move |menu, entity, window, cx| {
        let now = entity.read(cx).sidebar.accounts_for(&owner);
        if *seen.borrow() == now { return; }
        *seen.borrow_mut() = now.clone();
        let (weak, owner) = (weak.clone(), owner.clone());
        menu.rebuild(window, cx, move |menu, _, cx| fill_accounts(menu, &weak, &owner, idle, now, cx));
    }).detach();
    fill_accounts(menu, hangar, target, idle, now, cx)
}

fn fill_accounts(menu: PopupMenu, hangar: &WeakEntity<Hangar>, target: &Target, idle: bool, accounts: Option<Accounts>, cx: &App) -> PopupMenu {
    let menu = menu_style(menu).min_w(px(200.)).label(tr("sidebar_same"));
    // Trocar reinicia o processo: com a sessão trabalhando ou perguntando, só o motivo.
    if !idle { return menu.item(PopupMenuItem::new(web("modo_so_ociosa")).disabled(true)); }
    let (list, notice) = match accounts {
        Some(Accounts::Known(list, notice)) => (list, notice),
        Some(Accounts::Failed(reason)) => {
            let text = tr("sidebar_accounts_failed").replace("{n}", &reason);
            return menu.item(PopupMenuItem::element(move |_, _| div().text_color(theme::danger()).child(text.clone())).disabled(true));
        }
        Some(Accounts::Loading) | None => return menu.item(PopupMenuItem::new(tr("sidebar_loading")).disabled(true)),
    };
    let menu = menu.when_some(notice, |menu, text| menu
        .item(PopupMenuItem::element(move |_, _| div().text_color(theme::danger()).whitespace_normal().child(text.clone())).disabled(true))
        .separator());
    if list.is_empty() { return menu.item(PopupMenuItem::new(tr("sidebar_no_accounts")).disabled(true)); }
    let returning = hangar.upgrade().is_some_and(|entity| entity.read(cx).target_session(target)
        .is_some_and(|s| s.engine.as_deref().is_some_and(|e| !e.is_empty())));
    list.into_iter().fold(menu.max_h(px(260.)).scrollable(true), |menu, AccountTarget { path, label, pct, low, full, engine_account }| {
        let (hangar, target) = (hangar.clone(), target.clone());
        // O % da janela mais cheia ao lado do nome; esgotada não aceita a conversa, acabando aceita com aviso na confirmação.
        let name = if returning && engine_account.is_none() { tr("sidebar_return_claude_account").replace("{n}", &label) }
            else if engine_account.is_some() { format!("ChatGPT · {label}") } else { label.clone() };
        let text = match pct { Some(pct) => format!("{name} · {pct:.0}%"), None => name };
        let text = if full { format!("{text} · {}", tr("sidebar_account_full")) } else if low { format!("{text} · {}", tr("sidebar_account_low")) } else { text };
        let warn = pct.filter(|_| low);
        menu.item(PopupMenuItem::new(text).disabled(full).on_click(move |_, window, cx| {
            let _ = hangar.update(cx, |this, cx| match &engine_account {
                Some(account) => this.confirm_engine_account(target.clone(), account.clone(), label.clone(), warn, window, cx),
                None => this.confirm_account(target.clone(), path.clone(), label.clone(), warn, window, cx),
            });
        }))
    })
}

/// Nome de branch ou de sessão em fonte mono, como o web; o atual com ✓ e na cor de destaque.
pub(super) fn mono_item(text: String, current: bool) -> PopupMenuItem {
    PopupMenuItem::element(move |_, _| div().font_family(theme::MONO).text_size(px(13.)).when(current, |el| el.text_color(theme::accent_text())).child(text.clone()))
        .checked(current)
}

/// Submenu do encadear: as outras sessões (✓ no alvo atual) e, com alvo, o Remover vínculo.
fn fill_chain(menu: PopupMenu, hangar: &WeakEntity<Hangar>, from: &Target, current: Option<String>, others: &[String]) -> PopupMenu {
    let menu = menu_style(menu).label(tr("sidebar_chain"));
    let menu = if others.is_empty() { menu.item(PopupMenuItem::new(tr("sidebar_no_other")).disabled(true)) } else {
        others.iter().fold(menu.min_w(px(220.)).max_h(px(260.)).scrollable(true), |menu, target| {
            let (hangar, from, target) = (hangar.clone(), from.clone(), target.clone());
            menu.item(mono_item(target.clone(), current.as_deref() == Some(target.as_str())).on_click(move |_, window, cx| {
                let _ = hangar.update(cx, |this, cx| this.start_chain(from.clone(), target.clone(), window, cx));
            }))
        })
    };
    let (hangar, from) = (hangar.clone(), from.clone());
    menu.when(current.is_some(), |menu| menu.separator().item(PopupMenuItem::element(|_, _| div().text_color(theme::danger()).child(tr("sidebar_unlink")))
        .on_click(move |_, window, cx| { let _ = hangar.update(cx, |this, cx| this.git_write(from.clone(), GitWrite::Unlink, window, cx)); })))
}

/// A barra recolhida mora no mesmo arquivo dos grupos recolhidos: é mais uma coisa recolhida.
const RAIL_KEY: &str = "sidebar:rail";
pub(super) const RAIL_WIDTH: f32 = 56.;
/// Caracteres de cada linha do nome no trilho, como o `RAIL_MAX` do web.
const RAIL_MAX: usize = 8;

/// O `railLabel` do web: o nome partido nos separadores, a primeira palavra em cima e o resto embaixo, oito caracteres cada.
fn rail_label(name: &str) -> (String, String) {
    let parts: Vec<&str> = name.split(|c: char| !c.is_alphanumeric()).filter(|p| !p.is_empty()).collect();
    let cut = |s: &str| s.chars().take(RAIL_MAX).collect::<String>();
    match parts.split_first() {
        None => (if name.is_empty() { "?".into() } else { cut(name) }, String::new()),
        Some((first, rest)) => (cut(first), cut(&rest.join("-"))),
    }
}

impl Hangar {
    /// A lista recolhida no trilho, como o Ctrl+B do web. Com abas não há barra para recolher.
    pub(super) fn rail(&self) -> bool {
        !appearance::get().navigation.tabs() && self.sidebar.is_collapsed(RAIL_KEY)
    }

    pub(super) fn nav_width(&self) -> f32 {
        let full = appearance::get().full_sidebar_width();
        match self.rail_progress() {
            Some(p) => full + (RAIL_WIDTH - full) * p,
            None if self.rail() => RAIL_WIDTH,
            None => full,
        }
    }

    /// Alça na borda direita da barra cheia, como a do web: a largura segue o ponteiro e fica gravada ao soltar.
    pub(super) fn nav_resize_handle(&self, width: f32, cx: &mut Context<Self>) -> AnyElement {
        div().id("nav-resize").role(Role::Splitter).aria_label(tr("sidebar_resize"))
            .absolute().right_0().top_0().bottom_0().w(px(6.)).cursor_col_resize()
            .hover(|el| el.bg(theme::accent_dim()))
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                this.sidebar.resize = Some((f32::from(event.position.x), width));
                cx.stop_propagation();
                cx.notify();
            }))
            .into_any_element()
    }

    pub(super) fn nav_resizing(&self) -> bool { self.sidebar.resize.is_some() }

    pub(super) fn drag_nav(&mut self, x: f32, pressed: bool, cx: &mut Context<Self>) {
        let Some((start_x, start_width)) = self.sidebar.resize else { return };
        let mut next = appearance::get();
        next.sidebar_width = Some((start_width + x - start_x).clamp(appearance::SIDEBAR_MIN, appearance::SIDEBAR_MAX));
        if !pressed { self.sidebar.resize = None; }
        self.apply_appearance(next, !pressed, cx);
    }

    /// A marca de "trabalhando" de uma linha da barra. Durante a troca com o trilho ela fica parada dentro da linha: a
    /// animada é pintada fora da lista e não acompanharia a camada que some.
    pub(super) fn nav_mark(&self, key: String, size: f32, color: Hsla, badge: Option<SharedString>) -> AnyElement {
        if self.rail_progress().is_some() {
            return div().relative().size(px(size)).flex_shrink_0().child(chrome::hangar_mark(size, color))
                .children(badge.map(|provider| chrome::provider_badge(&provider))).into_any_element();
        }
        match badge {
            Some(provider) => self.badged_mark_slot(panes::Area::Nav, key, size, color, provider),
            None => self.working_mark_slot(panes::Area::Nav, key, size, color),
        }
    }

    /// Quanto a troca andou rumo ao trilho, de 0 (lista cheia) a 1 (trilho); `None` parada.
    pub(super) fn rail_progress(&self) -> Option<f32> {
        let (start, to_rail) = self.sidebar.rail_anim?;
        if start.elapsed() >= motion::NAV_FOLD.total() { return None; }
        let t = motion::NAV_FOLD.ease(motion::NAV_FOLD.raw(start));
        Some(if to_rail { t } else { 1. - t })
    }

    /// Chamado no desenho da raiz: enquanto a troca anda, pede o quadro seguinte para a própria raiz, que dá a largura à
    /// área da barra e acorda as áreas; terminada, solta o relógio e não pede mais nada.
    pub(super) fn rail_frame(&mut self, window: &mut Window) {
        if self.sidebar.rail_anim.is_none() { return; }
        if self.rail_progress().is_none() { self.sidebar.rail_anim = None; return; }
        window.request_animation_frame();
    }

    pub(super) fn toggle_rail(&mut self, cx: &mut Context<Self>) {
        if appearance::get().navigation.tabs() { return; }
        self.hide_preview();
        // Movimento reduzido troca direto; senão a largura anda de onde estiver agora.
        self.sidebar.rail_anim = (!cx.reduce_motion()).then(|| (Instant::now(), !self.rail()));
        self.toggle_group(RAIL_KEY.into(), cx);
    }

    /// Recolher/expandir, a última peça do rodapé nas duas formas da barra.
    /// `id` próprio de cada forma: na animação de recolher as duas aparecem juntas.
    pub(super) fn fold_button(&self, id: &'static str, cx: &mut Context<Self>) -> impl IntoElement {
        let rail = self.rail();
        Button::new(id).ghost().icon(chrome::small_icon(IconName::PanelLeft, 18., theme::muted()))
            .h(px(36.)).w(px(if rail { 40. } else { 36. })).flex_shrink_0().rounded(px(8.))
            .accessibility_label(web(if rail { "sessao_expandir_barra" } else { "sessao_recolher_barra" }))
            .tooltip(web(if rail { "sessao_expandir_atalho" } else { "sessao_recolher_atalho" }))
            .on_click(cx.listener(|this, _, _, cx| this.toggle_rail(cx)))
    }

    /// O trilho de 56 px do web: a marca em cima, cada sessão como estado em cima e o nome em duas linhas mono embaixo,
    /// e no rodapé a nova sessão, o recolher e a conexão. Os cabeçalhos somem e nenhum grupo fica recolhido.
    pub(super) fn render_nav_rail(&self, selected_name: Option<&str>, cx: &mut Context<Self>) -> AnyElement {
        let a = appearance::get();
        let fit_content = a.panels == appearance::Panels::Floating && a.sidebar_height == appearance::SidebarHeight::Content;
        let mixed = self.sessions.iter().filter(|s| !s.orq()).map(|s| agent_name(&s.provider)).collect::<HashSet<_>>().len() > 1;
        let local = self.sidebar_layout(cx);
        let mut rows: Vec<AnyElement> = Vec::new();
        let place = |layout: &Layout, remote: Option<&str>, rows: &mut Vec<AnyElement>, cx: &mut Context<Self>| {
            for session in layout.waiting.iter().chain(layout.groups.iter().flat_map(|g| g.sessions.iter())) {
                let selected = selected_name == Some(session.name.as_str()) && self.open_key().as_deref() == remote;
                rows.push(self.render_rail_row((*session).clone(), selected, mixed, remote.map(str::to_owned), cx));
            }
        };
        if self.multi_server() {
            let active = self.active_key();
            for entry in self.servers.iter().filter(|s| !s.disabled) {
                let key = servers::norm(&entry.address);
                if key == active { place(&local, None, &mut rows, cx); }
                else if let Some(list) = self.remote.get(&key) { place(&self.remote_layout(&key, &list.sessions, cx), Some(&key), &mut rows, cx); }
            }
        } else {
            place(&local, None, &mut rows, cx);
        }
        let host = self.server_label(cx);
        let (on, enabled) = (self.new_chat_screen() && self.reopen.is_none(), self.api.is_some());
        div().w_full().min_h_0().flex().flex_col().items_center().when(!fit_content, |el| el.h_full())
            // O ponto do chip "N no Hangar" fica no canto da marca.
            .child(div().relative().h(px(44.)).w_full().flex_shrink_0().flex().items_center().justify_center()
                .child(chrome::hangar_mark(20., theme::accent()))
                .children(self.render_hangar_chip(hangar_live::Chip::Dot, cx).map(|dot| div().absolute().top(px(6.)).right(px(8.)).child(dot))))
            // A tela sem sessão da barra aberta, só com o ícone.
            .child(div().id("rail-new-chat").flex_shrink_0().size(px(36.)).mb(px(4.)).flex().items_center().justify_center().rounded(px(8.))
                .role(Role::Button).aria_selected(on).aria_label(tr("new_chat_title"))
                .when(on, |el| el.bg(theme::selected_row()))
                .when(!enabled, |el| el.opacity(0.5))
                .when(enabled && !on, |el| el.cursor_pointer().hover(|el| el.bg(theme::hover())))
                .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new(tr("new_chat_title")).build(window, cx))
                .child(chrome::small_icon(IconName::SquarePen, 16., theme::muted()))
                .when(enabled, |el| el.on_click(cx.listener(|this, _, window, cx| this.go_home(window, cx)))))
            .child(div().id("session-list").min_h_0().w_full().overflow_y_scroll().px(px(3.)).flex().flex_col().gap(px(4.))
                .when(!fit_content, |el| el.flex_1())
                .children(rows))
            .child(div().w_full().flex_shrink_0().pt(px(8.)).pb(px(8.)).mt(px(4.)).border_t_1().border_color(theme::border())
                .flex().flex_col().items_center().gap(px(4.))
                // O CTA do web no trilho: o botão cheio de destaque, só com o +.
                .child(Button::new("rail-new-session").custom(ButtonCustomVariant::new(cx).color(theme::accent()).foreground(theme::on_accent())
                        .hover(theme::accent().opacity(0.85)).active(theme::accent().opacity(0.75)))
                    .icon(chrome::small_icon(IconName::Plus, 16., theme::on_accent())).w(px(44.)).h(px(36.)).rounded(px(8.))
                    .tooltip(tr("create_title")).accessibility_label(tr("create_title")).disabled(self.api.is_none())
                    .on_click(cx.listener(|this, _, window, cx| this.open_new_session(None, window, cx))))
                .child(self.fold_button("rail-fold", cx))
                .child(Button::new("rail-connection").ghost().size(px(36.)).rounded(px(8.)).tooltip(host).accessibility_label(tr("connection"))
                    .child(div().size(px(7.)).rounded_full().bg(if self.list_online { theme::success() } else { theme::warning() }))
                    .on_click(cx.listener(|this, _, window, cx| this.open_connection(window, cx)))))
            .into_any_element()
    }

    fn render_rail_row(&self, session: SessionInfo, selected: bool, mixed: bool, remote: Option<String>, cx: &mut Context<Self>) -> AnyElement {
        let state = session.state.as_str();
        let limited = session.limited == Some(true);
        let awaiting = state == "awaiting_input";
        let color = if limited { theme::limited() } else { theme::status(state) };
        let (top, bottom) = rail_label(&session.name);
        let target = Target::new(&remote.clone().unwrap_or_else(|| self.active_key()), &session.name);
        let row_key = match &remote { Some(_) => target.id(), None => session.name.clone() };
        let host = match &remote { Some(key) => self.servers.iter().find(|s| servers::norm(&s.address) == *key).map_or(key.clone(), |s| s.label.clone()),
            None => self.server_label(cx) };
        let questions = session.pending_questions;
        let mut tip = format!("{} · {host} · {}", session.name, tr(&format!("chip_{}", if limited { "limited" } else { state })));
        if questions > 0 { tip.push_str(&format!(" · ? {questions}")); }
        if session.orq() { tip.push_str(&format!(" · {}", tr_shared("orq_row_badge", &[]))); }
        // Trabalhando é a marca animada da lista aberta, pintada fora da lista guardada; os outros estados são um ponto na cor
        // deles, e quem espera resposta ganha o halo (parado: pulsar redesenharia a janela o tempo todo).
        let status = if state == "working" && !limited {
            self.nav_mark(format!("rail-mark-{row_key}"), 12., color, None)
        } else {
            div().size(px(11.)).flex().items_center().justify_center().rounded_full().when(awaiting, |el| el.bg(color.opacity(0.55)))
                .child(div().size(px(7.)).rounded_full().bg(color)).into_any_element()
        };
        let tip_text = tip.clone();
        let el = div().id(SharedString::from(format!("rail-{row_key}"))).relative().flex_shrink_0().w_full().min_h(px(44.)).pt(px(3.))
            .flex().flex_col().items_center().gap(px(2.)).rounded(px(8.)).cursor_pointer()
            .when(selected, |el| el.bg(theme::accent_dim()).child(div().absolute().left_0().top_0().bottom_0().w(px(3.)).bg(theme::accent())))
            .when(!selected, |el| el.hover(|el| el.bg(theme::hover())))
            .role(Role::Button).aria_selected(selected).aria_label(self.session_number_label(&target, tip))
            .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip_text.clone()).build(window, cx))
            .children(self.session_number_badge(&target).map(|badge| div().absolute().top_0().right_0().child(badge)))
            .child(div().h(px(11.)).flex().items_center().justify_center().child(status))
            .child(div().flex().flex_col().items_center().font_family(theme::MONO).text_size(px(9.)).line_height(px(10.)).whitespace_nowrap()
                .child(div().font_weight(FontWeight::SEMIBOLD).text_color(if awaiting { theme::warning() } else { theme::text() }).child(top))
                .child(div().min_h(px(10.)).text_color(theme::faint()).child(bottom)))
            .when(questions > 0, |el| el.child(div().text_size(px(9.)).text_color(theme::warning()).child(format!("? {questions}"))))
            .when(mixed && !session.orq(), |el| el.child(chrome::provider_badge(&session.provider)));
        // Linha de qualquer máquina: o mesmo menu e o mesmo clique, cada um na máquina dela.
        let (weak, menu_target, open) = (cx.entity().downgrade(), target.clone(), target.clone());
        el.on_mouse_down(MouseButton::Right, cx.listener(move |this, _, _, cx| this.start_menu(menu_target.clone(), cx)))
            .on_click(cx.listener(move |this, _, window, cx| this.open_target(&open, window, cx)))
            .context_menu(session_menu(weak, target.server, session))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    // Sem glob: o `test` do gpui_kit, que o `super::*` traz, esconderia o `#[test]` da linguagem.
    use super::{BranchList, HashSet, SessionInfo, first_line, has_git, layout, rail_label, save_collapsed};

    #[test]
    fn empty_second_line_keeps_the_last_one_of_the_same_conversation() {
        use super::{Sub, Target, kept_sub};
        let (mut last, row) = (super::LastSubs::new(), Target::new("m", "s"));
        let working = || Some(("Puttering…".to_owned(), Sub::Working));
        assert_eq!(kept_sub(&mut last, &row, Some("a.jsonl"), None), None);
        assert_eq!(kept_sub(&mut last, &row, Some("a.jsonl"), working()), working());
        // O rótulo some entre uma etapa e outra: a linha fica.
        assert_eq!(kept_sub(&mut last, &row, Some("a.jsonl"), None), working());
        // A linha nova, quando vem, vale e passa a ser a guardada.
        let reply = Some(("Pronto.".to_owned(), Sub::Reply));
        assert_eq!(kept_sub(&mut last, &row, Some("a.jsonl"), reply.clone()), reply);
        assert_eq!(kept_sub(&mut last, &row, Some("a.jsonl"), None), reply);
        // `/clear` troca o transcript: a linha da conversa anterior não volta. A de outra sessão também não.
        assert_eq!(kept_sub(&mut last, &row, Some("b.jsonl"), None), None);
        assert_eq!(kept_sub(&mut last, &Target::new("m", "outra"), Some("a.jsonl"), None), None);
    }

    #[test]
    fn list_step_wraps_and_enters_from_the_end_it_points_to() {
        use super::wrap_step;
        assert_eq!((wrap_step(Some(2), 3, 1), wrap_step(Some(0), 3, -1)), (Some(0), Some(2)));
        assert_eq!((wrap_step(None, 3, 1), wrap_step(None, 3, -1)), (Some(0), Some(2)));
        assert_eq!(wrap_step(None, 0, 1), None);
    }

    #[test]
    fn second_line_holds_across_state_changes_until_new_text() {
        use super::{Sub, Target, kept_sub};
        let (mut last, row) = (super::LastSubs::new(), Target::new("m", "s"));
        let reply = Some(("Pronto.".to_owned(), Sub::Reply));
        kept_sub(&mut last, &row, Some("a.jsonl"), reply.clone());
        // Mandou mensagem: trabalhando ainda sem rótulo, a resposta anterior fica no card.
        assert_eq!(kept_sub(&mut last, &row, Some("a.jsonl"), None), reply);
        // O rótulo chega e troca; o turno acaba e a resposta nova ainda não veio: o rótulo fica.
        let working = Some(("Precipitating…".to_owned(), Sub::Working));
        assert_eq!(kept_sub(&mut last, &row, Some("a.jsonl"), working.clone()), working);
        assert_eq!(kept_sub(&mut last, &row, Some("a.jsonl"), None), working);
    }

    #[test]
    fn unknown_transcript_neither_drops_nor_swaps_the_second_line() {
        use super::{Sub, Target, kept_sub};
        let (mut last, row) = (super::LastSubs::new(), Target::new("m", "s"));
        let reply = Some(("Pronto.".to_owned(), Sub::Reply));
        kept_sub(&mut last, &row, Some("a.jsonl"), reply.clone());
        // Transcript sumiu da lista por um instante: a linha fica.
        assert_eq!(kept_sub(&mut last, &row, None, None), reply);
        // Texto novo sem transcript conhecido ainda vale, e a conversa guardada continua sendo a mesma.
        let working = Some(("Puttering…".to_owned(), Sub::Working));
        assert_eq!(kept_sub(&mut last, &row, None, working.clone()), working);
        assert_eq!(kept_sub(&mut last, &row, Some("a.jsonl"), None), working);
        assert_eq!(kept_sub(&mut last, &row, Some("b.jsonl"), None), None);
    }

    #[test]
    fn failed_local_proxy_discovery_keeps_claude_return_and_its_error() {
        let legacy: Vec<super::AccountTarget> = serde_json::from_value(serde_json::json!([
            {"path":"/claude-storage", "label":"Claude storage"}])).unwrap();
        let (targets, notice) = super::merge_account_targets(legacy.clone(), Err("duplicate proxy accounts".into()), true).unwrap();
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].path, "/claude-storage");
        assert!(targets[0].engine_account.is_none());
        assert_eq!(notice.as_deref(), Some("duplicate proxy accounts"));
        let (generic, notice) = super::merge_account_targets(legacy.clone(), Ok(None), false).unwrap();
        assert!(generic.is_empty() && notice.is_none());
        assert!(super::merge_account_targets(legacy, Err("invalid engine response".into()), false).is_err());
    }

    #[test]
    fn account_requests_distinguish_proxy_identity_from_claude_storage() {
        assert_eq!(super::account_body("/claude-storage", Some("other")), serde_json::json!({"engine_account":"other"}));
        assert_eq!(super::account_body("/claude-storage", None), serde_json::json!({"config_dir":"/claude-storage"}));
    }

    #[test]
    fn transfer_requires_captured_life_and_original_history_without_pending_actions() {
        let mut source = SessionInfo { provider: "claude".into(), name: "session".into(), state: "idle".into(),
            conta: Some("claude:/registered".into()), lifecycle_id: Some("k:original".into()), jsonl: Some("/original.jsonl".into()),
            ..Default::default() };
        assert_eq!(super::transfer_source(&source), Some(("k:original", "/original.jsonl")));
        source.state = "dead".into();
        assert!(super::transfer_source(&source).is_some());
        for state in ["working", "awaiting_input", "loading", ""] {
            source.state = state.into();
            assert!(super::transfer_source(&source).is_none());
        }
        source.state = "idle".into();
        source.pending_questions = 1;
        assert!(super::transfer_source(&source).is_none());
        source.pending_questions = 0;
        source.question = Some("question".into());
        assert!(super::transfer_source(&source).is_none());
        source.question = None;
        for phase in ["preparing", "source_stopped", "imported", "publishing", "restoring", "restore_failed", "complete"] {
            source.transfer_phase = Some(phase.into());
            assert!(super::transfer_source(&source).is_none());
        }
        for phase in ["rejected", "rolled_back"] {
            source.transfer_phase = Some(phase.into());
            assert!(super::transfer_source(&source).is_some());
        }
        source.transfer_phase = None;
        source.guest_kind = Some("pair".into());
        assert!(super::transfer_source(&source).is_none());
        source.guest_kind = None;
        source.tracked = Some(false);
        assert!(super::transfer_source(&source).is_none());
        source.tracked = Some(true);
        source.conta = Some("engine:key".into());
        assert!(super::transfer_source(&source).is_none());
        source.conta = Some("claude:/registered".into());
        source.engine = Some("proxy".into());
        assert!(super::transfer_source(&source).is_none());
        source.engine = None;
        source.engine_account = Some("default".into());
        assert!(super::transfer_source(&source).is_none());
        source.engine_account = None;
        source.lifecycle_id = None;
        assert!(super::transfer_source(&source).is_none());
        source.lifecycle_id = Some("t:original".into());
        source.jsonl = None;
        assert!(super::transfer_source(&source).is_none());
    }

    #[test]
    fn rail_label_splits_like_the_web() {
        assert_eq!(rail_label("hangar-2"), ("hangar".into(), "2".into()));
        assert_eq!(rail_label("análise-app"), ("análise".into(), "app".into()), "acento é letra, não separador");
        assert_eq!(rail_label("storefront-web-admin"), ("storefro".into(), "web-admi".into()));
        assert_eq!(rail_label("---"), ("---".into(), String::new()));
    }

    #[test]
    fn git_items_need_a_repository_and_the_first_output_line_is_the_result() {
        let mut repo = s("r", "idle", Some("/p/r"));
        repo.branch = Some("main".into());
        assert!(has_git(&repo));
        assert!(!has_git(&s("r", "idle", Some("/p/r"))), "branch nula: pasta fora de um repositório");
        assert!(!has_git(&super::SessionInfo { branch: Some("main".into()), ..s("r", "idle", None) }), "sem pasta não há git");
        assert_eq!(first_line(&serde_json::json!({"output": "\nUpdating 0..1\nFast-forward"})).as_deref(), Some("Updating 0..1"));
        assert_eq!(first_line(&serde_json::json!({"output": "  "})), None);
        let list: BranchList = serde_json::from_value(serde_json::json!({"current": "main", "branches": ["main", "b"], "remotes": ["x"]})).unwrap();
        assert!(!list.dirty && list.branches.len() == 2, "dirty ausente é árvore limpa; remotes não entram no menu, como no web");
    }

    fn s(name: &str, state: &str, cwd: Option<&str>) -> SessionInfo {
        SessionInfo { name: name.into(), state: state.into(), cwd: cwd.map(str::to_owned), ..Default::default() }
    }

    fn names(list: &[&SessionInfo]) -> Vec<String> { list.iter().map(|s| s.name.clone()).collect() }

    #[test]
    fn waiting_on_top_then_by_name_and_never_twice() {
        let all = [s("zeta", "idle", Some("/p/a")), s("Beta", "awaiting_input", Some("/p/b")), s("alfa", "working", Some("/p/a/")),
            s("api", "idle", None), s("ação", "idle", None)];
        let l = layout(&all, "", false, &HashSet::new());
        assert_eq!(names(&l.waiting), ["Beta"]);
        assert_eq!(names(&l.groups[0].sessions), ["ação", "alfa", "api", "zeta"], "ç ordena como c, como o localeCompare");
        let p = layout(&all, "", true, &HashSet::new());
        // Sem pasta é o grupo "sem projeto", não some.
        let groups: Vec<(&str, Vec<String>)> = p.groups.iter().map(|g| (g.key.as_str(), names(&g.sessions))).collect();
        assert_eq!(groups, [("/p/a", vec!["alfa".to_owned(), "zeta".into()]), (super::NO_CWD, vec!["ação".into(), "api".into()])],
            "/p/a e /p/a/ são o mesmo projeto; Beta fica só em Aguardando");
        assert_eq!(p.groups[0].label, "a");
    }

    #[test]
    fn filter_shows_from_seven_ignores_case_and_keeps_accents() {
        let mut all: Vec<SessionInfo> = (0..6).map(|i| s(&format!("s{i}"), "idle", Some("/x"))).collect();
        all.push(s("Ação", "idle", Some("/proj/Área")));
        assert!(layout(&all, "", false, &HashSet::new()).show_filter());
        assert_eq!(names(&layout(&all, "AÇÃO", false, &HashSet::new()).groups[0].sessions), ["Ação"]);
        assert!(layout(&all, "acao", false, &HashSet::new()).filter_empty());
        assert_eq!(names(&layout(&all, "área", false, &HashSet::new()).groups[0].sessions), ["Ação"], "casa na pasta");
        let hidden: HashSet<String> = ["s0".to_owned()].into();
        let short = layout(&all, "AÇÃO", false, &hidden);
        assert!(!short.show_filter() && short.groups[0].sessions.len() == 6, "fechada não conta e filtro escondido não filtra");
    }

    #[test]
    fn older_collapse_write_never_overwrites_newer() {
        let dir = std::env::temp_dir().join(format!("hangar-collapsed-{}", std::process::id()));
        let path = dir.join("sidebar-collapsed.json");
        let (both, one): (HashSet<String>, HashSet<String>) = (["/a".to_owned(), "/b".to_owned()].into(), ["/a".to_owned()].into());
        save_collapsed(Some(path.clone()), 2, &both).unwrap();
        save_collapsed(Some(path.clone()), 1, &one).unwrap();
        let read: HashSet<String> = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(read, both, "a gravação do 1º clique terminou por último e não voltou o arquivo");
        assert!(!path.with_extension("tmp").exists());
        let _ = std::fs::remove_dir_all(dir);
    }
}
