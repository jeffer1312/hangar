//! Barra lateral como a do web (Sidebar.svelte + sessionListModel): filtro, agrupar por projeto, ordem por nome,
//! prévia da última resposta ao parar o mouse e o menu da sessão (Renomear, Silenciar, Copiar cwd, Abrir no editor,
//! Fechar). Toda resposta volta amarrada ao nome capturado no gesto e ao número do pedido: resposta velha não mexe
//! no que a pessoa fez depois.
use super::*;
use gpui_kit::base::AccordionTrigger;
use gpui_kit::component::{WindowExt, dialog::{self, DialogButtonProps}, menu::{DropdownMenu, PopupMenu}, notification::NotificationType};
use super::machines::enter_to_focused;
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

/// Estado de silenciar da sessão do menu aberto, lido de `GET /api/push/settings`.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Mute { Loading, Known(bool), Failed(String) }

/// Branches do repositório da sessão do menu aberto, lidas de `GET …/branches` ao abrir o menu.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Branches { Loading, Known(BranchList), Failed(String) }

#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize)]
pub(super) struct BranchList { #[serde(default)] branches: Vec<String>, current: Option<String>, #[serde(default)] dirty: bool }

struct MenuRead { name: String, seq: u64, mute: Mute, branches: Option<Branches> }

#[derive(Clone, Debug)]
/// O renomear leva o número do pedido: resposta de um diálogo cancelado não fecha a tentativa seguinte na mesma sessão.
pub(super) enum Write { Rename(String, u64), Mute(bool), Editor, Delete }

/// Gravações de git e o remover vínculo: o resultado é a notificação da sessão, já em texto.
enum GitWrite { Pull, Checkout(String), StashCheckout(String), Unlink }

pub(super) enum SidebarReply {
    Preview(u64, String, Result<String, Failure>),
    MuteRead(u64, Result<Value, Failure>),
    BranchRead(u64, Result<Value, Failure>),
    Wrote(String, Write, Result<Value, Failure>),
    /// Resultado de uma gravação de git: nível e texto da notificação daquela sessão.
    Note(String, NotificationType, String),
    Chained(String, u64, String, Result<Value, Failure>),
    NotSaved(String),
}

/// Uma notificação por sessão: o "git pull…" dá lugar ao resultado, como o `flash` único do web.
struct GitNote;

fn git_note(name: &str, kind: NotificationType, text: String) -> Notification {
    // O título diz de qual sessão é: o gesto pode ter sido numa linha que não é a aberta.
    Notification::new().title(name.to_owned()).message(text).with_type(kind).id1::<GitNote>(SharedString::from(name.to_owned()))
}

/// Git no menu só com pasta num repositório: o backend manda `branch` nulo fora de um (como o web, SCM:162).
fn has_git(s: &SessionInfo) -> bool { s.cwd.as_deref().is_some_and(|c| !c.is_empty()) && s.branch.is_some() }

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

/// Encadear: o alvo escolhido no submenu e o prompt, num diálogo curto.
pub(super) struct Chain { from: String, target: String, input: Entity<InputState>, status: Rc<RefCell<Pending>>, _events: Subscription }

/// Leva a resposta de volta à janela, amarrada à conexão do pedido.
struct Tell(async_channel::Sender<Envelope>, u64);

impl Tell {
    async fn send(&self, reply: SidebarReply) {
        let _ = self.0.send(Envelope { connection: self.1, selection: None, payload: Payload::Sidebar(reply) }).await;
    }
}

/// Nome em edição: na própria linha (barra lateral) ou num diálogo (abas no topo, como o web com a barra recolhida).
pub(super) struct Edit {
    pub(super) old: String, pub(super) input: Entity<InputState>, inline: bool, status: Rc<RefCell<Pending>>, _events: Subscription,
}

/// Diálogo de renomear: o pedido em voo e a falha dele seguram o resultado junto do campo; só fecha com o renomear confirmado,
/// como o web. Fora do `Hangar` porque o `open_dialog` chama o construtor na hora, com o `Hangar` em atualização.
#[derive(Default)]
struct Pending { sent: Option<u64>, error: Option<String> }

pub(super) struct Sidebar {
    pub(super) filter: Entity<InputState>,
    collapsed: HashSet<String>,
    /// Fechadas agora: somem da lista na hora e voltam se o servidor recusar.
    deleting: HashSet<String>,
    pub(super) editing: Option<Edit>,
    renaming: HashSet<String>,
    /// Aberta e renomeada: o nome novo abre assim que aparecer na lista.
    follow: Option<String>,
    /// A aberta sumiu da lista com o renomear em voo: a resposta decide se ela reabre pelo nome novo.
    lost: Option<String>,
    menu: Option<MenuRead>,
    menu_seq: u64,
    rename_seq: u64,
    pub(super) chain: Option<Chain>,
    chain_seq: u64,
    /// Renomeada pelo diálogo antes de a aba nova existir: o foco vai a ela quando a lista trouxer o nome.
    focus_tab: Option<String>,
    /// Número do último clique em cabeçalho de grupo: gravação de um clique anterior que termine depois não volta o arquivo.
    collapse_gen: u64,
    /// Linha cujo ⋯ está com o menu aberto: o botão fica na tela enquanto o ponteiro anda pelo menu.
    pub(super) button_menu: Option<String>,
    pub(super) hover: Option<String>,
    hover_seq: u64,
    pointer_y: f32,
    pub(super) preview: Option<(String, Entity<TextViewState>, f32)>,
    cache: HashMap<String, (String, Instant)>,
    press_seq: u64,
    long_pressed: bool,
}

impl Sidebar {
    pub(super) fn new(window: &mut Window, cx: &mut Context<Hangar>) -> Self {
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder(tr("sidebar_filter")).clean_on_escape());
        cx.subscribe(&filter, |_, _, _: &InputEvent, cx| cx.notify()).detach();
        Self { filter, collapsed: load_collapsed(), deleting: HashSet::new(), editing: None, renaming: HashSet::new(), follow: None, lost: None,
            menu: None, menu_seq: 0, rename_seq: 0, chain: None, chain_seq: 0, focus_tab: None, collapse_gen: 0, button_menu: None, hover: None, hover_seq: 0, pointer_y: 0., preview: None, cache: HashMap::new(), press_seq: 0,
            long_pressed: false }
    }

    /// Troca de servidor: o que é da conexão anterior sai; filtro e grupos recolhidos são deste computador e ficam.
    pub(super) fn reset_server(&mut self) {
        self.deleting.clear();
        self.renaming.clear();
        (self.editing, self.follow, self.lost, self.menu, self.button_menu, self.hover, self.preview) = (None, None, None, None, None, None, None);
        (self.focus_tab, self.chain) = (None, None);
        self.chain_seq += 1;
        self.cache.clear();
        self.menu_seq += 1;
        self.hover_seq += 1;
        self.press_seq += 1;
    }

    fn menu_for(&self, name: &str) -> Option<Mute> {
        self.menu.as_ref().filter(|m| m.name == name).map(|m| m.mute.clone())
    }

    fn branches_for(&self, name: &str) -> Option<Branches> {
        self.menu.as_ref().filter(|m| m.name == name).and_then(|m| m.branches.clone())
    }

    pub(super) fn hidden(&self) -> &HashSet<String> { &self.deleting }
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
    cwd.map(|c| c.trim_end_matches('/')).filter(|c| !c.is_empty()).and_then(|c| c.rsplit('/').next())
        .filter(|b| !b.is_empty()).map(str::to_owned).unwrap_or_else(|| tr("sidebar_no_project"))
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
    let base = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(base.join("hangar-native").join("sidebar-collapsed.json"))
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
    fn sidebar_tell(&self) -> Tell { Tell(self.tx.clone(), self.connection) }

    pub(super) fn by_project() -> bool { appearance::get().sidebar_group == appearance::SidebarGroup::Project }

    pub(super) fn sidebar_layout(&self, cx: &App) -> Layout<'_> {
        layout(&self.sessions, &self.sidebar.filter.read(cx).value(), Self::by_project(), &self.sidebar.deleting)
    }

    /// Ordem em que o Ctrl+↓/↑ anda: a das linhas à vista na barra (pula filtrada e recolhida) ou a das abas.
    fn visible_order(&self, cx: &App) -> Vec<String> {
        if appearance::get().navigation == appearance::Navigation::Tabs {
            return self.sessions.iter().filter(|s| !self.sidebar.deleting.contains(&s.name)).map(|s| s.name.clone()).collect();
        }
        let l = self.sidebar_layout(cx);
        l.waiting.iter().map(|s| s.name.clone())
            .chain(l.groups.iter().filter(|g| !(l.by_project && self.sidebar.collapsed.contains(&g.key)))
                .flat_map(|g| g.sessions.iter().map(|s| s.name.clone())))
            .collect()
    }

    pub(super) fn step_session(&mut self, step: isize, window: &mut Window, cx: &mut Context<Self>) {
        // Com o nome em edição a seta é do campo: trocar de sessão deixaria o campo aberto numa linha que não é a aberta.
        if window.has_active_dialog(cx) || self.connection_dialog || (self.settings.is_some() && !self.settings_live())
            || self.sidebar.editing.is_some() { return; }
        let order = self.visible_order(cx);
        if order.is_empty() { return; }
        let current = self.selected.as_ref().and_then(|s| order.iter().position(|n| n == &s.name));
        let next = match current {
            Some(i) => (i as isize + step).rem_euclid(order.len() as isize) as usize,
            None if step > 0 => 0,
            None => order.len() - 1,
        };
        if let Some(session) = self.sessions.iter().find(|s| s.name == order[next]).cloned() { self.select(session, window, cx); }
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

    /// A lista nova chegou: fechadas que saíram dela deixam de ser escondidas; renomeada aberta é reaberta pelo nome novo.
    pub(super) fn sidebar_sessions_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let names: HashSet<String> = self.sessions.iter().map(|s| s.name.clone()).collect();
        self.sidebar.deleting.retain(|n| names.contains(n));
        if self.sidebar.hover.as_ref().is_some_and(|n| !names.contains(n)) { self.hide_preview(); }
        // Só se o foco ainda está onde o renomear o deixou: gesto novo nesse meio-tempo vence.
        if let Some(new) = self.sidebar.focus_tab.clone().filter(|n| names.contains(n)) {
            self.sidebar.focus_tab = None;
            if self.root_focus.is_focused(window) && let Some(focus) = self.tab_focus.get(&new) { focus.focus(window, cx); }
        }
        if let Some(new) = self.sidebar.follow.clone().filter(|n| names.contains(n)) {
            self.sidebar.follow = None;
            if self.selected.is_none() && let Some(s) = self.sessions.iter().find(|s| s.name == new).cloned() {
                self.error = None;
                self.select(s, window, cx);
            }
        }
    }

    /// A sessão aberta sumiu da lista: se está sendo renomeada, a resposta do renomear decide; não é "sessão encerrada".
    pub(super) fn lost_while_renaming(&mut self, name: &str) -> bool {
        let renaming = self.sidebar.renaming.contains(name);
        if renaming { self.sidebar.lost = Some(name.to_owned()); }
        renaming
    }

    // ── Prévia ──

    pub(super) fn row_hover(&mut self, name: String, hovered: bool, cx: &mut Context<Self>) {
        if !hovered {
            if self.sidebar.hover.as_deref() == Some(name.as_str()) { self.hide_preview(); cx.notify(); }
            self.sidebar.press_seq += 1;
            return;
        }
        self.hide_preview();
        self.sidebar.hover = Some(name.clone());
        let seq = self.sidebar.hover_seq;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(PREVIEW_DELAY).await;
            let _ = this.update(cx, |this, cx| this.preview_due(seq, name, cx));
        }).detach();
        cx.notify();
    }

    pub(super) fn row_pointer(&mut self, y: f32) { self.sidebar.pointer_y = y; }

    pub(super) fn hide_preview(&mut self) {
        self.sidebar.hover_seq += 1;
        (self.sidebar.hover, self.sidebar.preview) = (None, None);
    }

    fn preview_due(&mut self, seq: u64, name: String, cx: &mut Context<Self>) {
        if seq != self.sidebar.hover_seq { return; }
        if let Some((text, _)) = self.sidebar.cache.get(&name).filter(|(_, at)| at.elapsed() < PREVIEW_TTL).cloned() {
            self.show_preview(name, text, cx);
            return;
        }
        let Some(api) = self.api.clone() else { return };
        let tell = self.sidebar_tell();
        self.runtime.spawn(async move {
            let result = api.history(&name, PREVIEW_TAIL, None).await.map(|h| h.events.unwrap_or_default().into_iter().rev()
                .find(|e| e.kind == "assistant_msg" && e.text.as_deref().is_some_and(|t| !t.is_empty()))
                .and_then(|e| e.text).unwrap_or_default());
            tell.send(SidebarReply::Preview(seq, name, result)).await;
        });
    }

    fn show_preview(&mut self, name: String, text: String, cx: &mut Context<Self>) {
        // Texto vazio não abre, como o web.
        if text.trim().is_empty() { return; }
        let view = cx.new(|cx| TextViewState::markdown(&safe_markdown(&text), cx));
        self.sidebar.preview = Some((name, view, self.sidebar.pointer_y));
        cx.notify();
    }

    // ── Pressionar para renomear ──

    pub(super) fn row_press(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        self.sidebar.press_seq += 1;
        self.sidebar.long_pressed = false;
        let seq = self.sidebar.press_seq;
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(PRESS).await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.sidebar.press_seq != seq { return; }
                this.sidebar.long_pressed = true;
                this.start_session_rename(name, window, cx);
            });
        }).detach();
    }

    pub(super) fn row_release(&mut self) { self.sidebar.press_seq += 1; }

    /// O clique que termina um pressionar longo não abre a sessão.
    pub(super) fn take_long_press(&mut self) -> bool { std::mem::take(&mut self.sidebar.long_pressed) }

    // ── Menu ──

    /// Abrir o menu lê o silenciar daquela sessão; resposta de um menu anterior é descartada pelo número.
    pub(super) fn start_menu(&mut self, name: String, cx: &mut Context<Self>) {
        self.hide_preview();
        self.sidebar.press_seq += 1;
        self.sidebar.menu_seq += 1;
        let seq = self.sidebar.menu_seq;
        // Branches só onde o menu mostra o git (pasta num repositório); como o silenciar, lidas a cada abertura do menu.
        let git = self.sessions.iter().find(|s| s.name == name).is_some_and(has_git);
        let Some(api) = self.api.clone() else {
            let failed = tr("connection_failed");
            self.sidebar.menu = Some(MenuRead { name, seq, mute: Mute::Failed(failed.clone()), branches: git.then_some(Branches::Failed(failed)) });
            return;
        };
        self.sidebar.menu = Some(MenuRead { name: name.clone(), seq, mute: Mute::Loading, branches: git.then_some(Branches::Loading) });
        if git {
            let (api, tell) = (api.clone(), self.sidebar_tell());
            self.runtime.spawn(async move { tell.send(SidebarReply::BranchRead(seq, api.read(&name, &["branches"], &[], 30).await)).await; });
        }
        let tell = self.sidebar_tell();
        self.runtime.spawn(async move { tell.send(SidebarReply::MuteRead(seq, api.server_read(&["push", "settings"], &[], 15).await)).await; });
        cx.notify();
    }

    pub(super) fn button_menu(&mut self, name: String, open: bool, cx: &mut Context<Self>) {
        if open { self.start_menu(name.clone(), cx); self.sidebar.button_menu = Some(name); }
        else if self.sidebar.button_menu.as_deref() == Some(name.as_str()) { self.sidebar.button_menu = None; cx.notify(); }
    }

    pub(super) fn start_session_rename(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        if !self.sessions.iter().any(|s| s.name == name) { return; }
        let inline = appearance::get().navigation == appearance::Navigation::Sidebar;
        let input = cx.new(|cx| InputState::new(window, cx).default_value(name.clone()));
        let old = name.clone();
        let events = cx.subscribe_in(&input, window, move |this, _, event: &InputEvent, window, cx| match event {
            InputEvent::PressEnter { .. } => this.commit_session_rename(window, cx),
            // Sair do campo salva, como o blur do web; no diálogo quem decide são os botões.
            InputEvent::Blur if this.sidebar.editing.as_ref().is_some_and(|e| e.inline && e.old == old) => this.commit_session_rename(window, cx),
            // O diálogo é desenhado com a janela: refazê-la liga e desliga o Renomear enquanto se digita.
            InputEvent::Change => cx.notify(),
            _ => {}
        });
        // Foco e seleção no quadro seguinte: o campo ainda não está na tela, e o diálogo toma o foco ao abrir.
        let field = input.clone();
        cx.defer_in(window, move |_, window, cx| field.update(cx, |state, cx| { state.focus(window, cx); state.select_all(window, cx); }));
        self.hide_preview();
        let status = Rc::new(RefCell::new(Pending::default()));
        self.sidebar.editing = Some(Edit { old: name.clone(), input: input.clone(), inline, status: status.clone(), _events: events });
        if !inline {
            // O diálogo devolve ao fechar o foco de quando abriu: o do menu morre com ele, então a aba de origem vem antes.
            self.focus_origin(&name, window, cx);
            let weak = cx.entity().downgrade();
            let owner = name.clone();
            window.open_dialog(cx, move |dialog, _, cx| {
                let value = input.read(cx).value().trim().to_owned();
                let (busy, error) = { let s = status.borrow(); (s.sent.is_some(), s.error.clone()) };
                let (commit, cancel, close, owner) = (weak.clone(), weak.clone(), weak.clone(), owner.clone());
                // Vazio ou igual ao atual não renomeia, e gravando não manda de novo: o botão fica desligado, como o do web.
                let rename = Button::new("rename-ok").primary().label(tr("sidebar_rename")).disabled(busy || value.is_empty() || value == owner)
                    .on_click(move |_, window, cx| {
                    let _ = commit.update(cx, |this, cx| this.commit_session_rename(window, cx));
                });
                let forget = move |weak: &WeakEntity<Hangar>, owner: &str, cx: &mut App| { let _ = weak.update(cx, |this, cx| {
                    if this.sidebar.editing.as_ref().is_some_and(|e| e.old == owner) { this.sidebar.editing = None; cx.notify(); }
                }); };
                let cancel_owner = owner.clone();
                // Borda vermelha liga o erro ao campo, como o `aria-invalid` do web; o anel de foco do kit a cobriria, então sai
                // enquanto o erro está à vista (o cursor segue mostrando o foco).
                let field = Input::new(&input).aria_label(tr("sidebar_new_name"))
                    .when(error.is_some(), |el| el.focus_bordered(false).border_color(theme::danger()));
                dialog.w(px(420.)).title(tr("sidebar_rename_title")).child(field)
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
        let Some(edit) = self.sidebar.editing.as_mut() else { return };
        let new = edit.input.read(cx).value().trim().to_owned();
        let unchanged = new.is_empty() || new == edit.old;
        let old = edit.old.clone();
        if edit.inline {
            // Na linha o campo fecha na hora e a falha vai à notificação, como o `saveEdit` do web.
            self.sidebar.editing = None;
            self.root_focus.focus(window, cx);
            cx.notify();
            if unchanged || self.api.is_none() { return; }
        } else {
            // No diálogo, vazio ou igual não faz nada (o web só envia válido), e gravando não manda de novo. Ele fica aberto
            // com o botão ocupado até a resposta, que fecha ou mostra o motivo junto do campo.
            let mut status = edit.status.borrow_mut();
            if unchanged || status.sent.is_some() || self.api.is_none() { return; }
            *status = Pending { sent: Some(self.sidebar.rename_seq + 1), error: None };
        }
        self.sidebar.rename_seq += 1;
        self.sidebar.renaming.insert(old.clone());
        self.write(old, Write::Rename(new, self.sidebar.rename_seq), cx);
    }

    fn write(&mut self, name: String, what: Write, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        let tell = self.sidebar_tell();
        self.runtime.spawn(async move {
            let result = match &what {
                Write::Mute(muted) => api.server_send(reqwest::Method::POST, &["push", "mute"], Some(json!({"session": name, "muted": muted})), 15).await,
                Write::Editor => api.act(&name, &["open-editor"], None, false, 15).await,
                Write::Delete => api.act(&name, &[], None, true, 30).await,
                Write::Rename(new, _) => api.act(&name, &["rename"], Some(json!({"new": new})), false, 30).await,
            };
            tell.send(SidebarReply::Wrote(name, what, result)).await;
        });
        cx.notify();
    }

    // ── Git e encadear ──

    /// "Git": a visão de git do nativo é a seção Projeto do painel da direita (decisão do árbitro), já com os arquivos alterados.
    fn open_git(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected.as_ref().is_none_or(|s| s.name != name) {
            let Some(session) = self.sessions.iter().find(|s| s.name == name).cloned() else { return };
            self.select(session, window, cx);
        }
        self.side.open = true;
        self.load_files(cx);
    }

    fn git_write(&mut self, name: String, what: GitWrite, window: &mut Window, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else {
            window.push_notification(git_note(&name, NotificationType::Error, tr("connection_failed")), cx);
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
            tell.send(SidebarReply::Note(name, kind, text)).await;
        });
    }

    /// Branch com a árvore suja pergunta antes, como o web; limpa troca direto.
    fn pick_branch(&mut self, name: String, branch: String, dirty: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !dirty { self.git_write(name, GitWrite::Checkout(branch), window, cx); return; }
        self.focus_origin(&name, window, cx);
        let weak = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            let run = |id: &'static str, label: String, stash: bool| {
                let (weak, name, branch) = (weak.clone(), name.clone(), branch.clone());
                Button::new(id).label(label).when(stash, |b| b.primary()).on_click(move |_, window, cx| {
                    window.close_dialog(cx);
                    let what = if stash { GitWrite::StashCheckout(branch.clone()) } else { GitWrite::Checkout(branch.clone()) };
                    let _ = weak.update(cx, |this, cx| this.git_write(name.clone(), what, window, cx));
                })
            };
            let line = |label: String, text: String| div().child(div().font_weight(FontWeight::SEMIBOLD).child(label))
                .child(div().text_color(theme::muted()).child(text));
            dialog.w(px(460.)).title(tr("sidebar_dirty_title"))
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

    fn start_chain(&mut self, from: String, target: String, window: &mut Window, cx: &mut Context<Self>) {
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
            dialog.w(px(460.)).title(tr("sidebar_chain_title").replace("{n}", &target)).child(field)
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
        let Some(api) = self.api.clone() else {
            *chain.status.borrow_mut() = Pending { sent: None, error: Some(tr("sidebar_chain_failed").replace("{n}", &tr("connection_failed"))) };
            cx.notify();
            return;
        };
        self.sidebar.chain_seq += 1;
        let seq = self.sidebar.chain_seq;
        *chain.status.borrow_mut() = Pending { sent: Some(seq), error: None };
        let (from, target, tell) = (chain.from.clone(), chain.target.clone(), self.sidebar_tell());
        self.runtime.spawn(async move {
            let result = api.server_send(reqwest::Method::PUT, &["sessions", &from, "then"], Some(json!({"target": target, "text": text})), 30).await;
            tell.send(SidebarReply::Chained(from, seq, target, result)).await;
        });
        cx.notify();
    }

    /// Foco na linha/aba de onde o menu saiu (o `menuOrigem` do web), para o diálogo devolvê-lo ao fechar.
    fn focus_origin(&self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.tab_focus.get(name).unwrap_or(&self.root_focus).focus(window, cx);
    }

    fn confirm_delete(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_origin(&name, window, cx);
        let this = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let (this, name) = (this.clone(), name.clone());
            alert.title(SharedString::from(tr("sidebar_close_title"))).description(SharedString::from(name.clone()))
                .button_props(DialogButtonProps::default().show_cancel(true).ok_text(tr("sidebar_close")).ok_variant(ButtonVariant::Danger)
                    .cancel_text(tr("cancel")))
                .on_ok(move |_, window, cx| { let _ = this.update(cx, |this, cx| {
                    // A linha some na hora e volta se o servidor recusar; sem conexão nada sai, e ela não some.
                    if this.api.is_none() { return; }
                    this.sidebar.deleting.insert(name.clone());
                    this.write(name.clone(), Write::Delete, cx);
                    // O Confirm fecha com animação e só então devolve o foco à linha, que já sumiu: passado esse prazo, a raiz
                    // o recebe (`fallbackFocus` do web). Se a linha voltou (o servidor recusou), o foco fica nela.
                    let name = name.clone();
                    cx.spawn_in(window, async move |this, cx| {
                        cx.background_executor().timer(*dialog::ANIMATION_DURATION + Duration::from_millis(50)).await;
                        let _ = this.update_in(cx, |this, window, cx| {
                            let gone = this.sidebar.deleting.contains(&name) || !this.sessions.iter().any(|s| s.name == name);
                            let on_row = this.tab_focus.get(&name).is_some_and(|f| f.is_focused(window));
                            if gone && (on_row || window.focused(cx).is_none()) { this.root_focus.focus(window, cx); }
                        });
                    }).detach();
                }); true })
        });
    }

    pub(super) fn receive_sidebar(&mut self, reply: SidebarReply, window: &mut Window, cx: &mut Context<Self>) {
        match reply {
            SidebarReply::NotSaved(error) => {
                eprintln!("grupos recolhidos não gravaram: {error}");
                window.push_notification(Notification::warning(tr("sidebar_collapse_not_saved").replace("{n}", &error)), cx);
            }
            SidebarReply::Preview(seq, name, result) => {
                // Espiada é opcional: falha não vira erro na tela (como o web), só no log.
                let text = match result { Ok(text) => text, Err(error) => { eprintln!("prévia de {name}: {}", error.detail); return; } };
                self.sidebar.cache.insert(name.clone(), (text.clone(), Instant::now()));
                if seq == self.sidebar.hover_seq && self.sidebar.hover.as_deref() == Some(name.as_str()) { self.show_preview(name, text, cx); }
            }
            SidebarReply::MuteRead(seq, result) => {
                let Some(menu) = self.sidebar.menu.as_mut().filter(|m| m.seq == seq) else { return };
                menu.mute = match result {
                    Ok(value) => match value.get("muted").and_then(Value::as_array) {
                        Some(list) => Mute::Known(list.iter().any(|v| v.as_str() == Some(menu.name.as_str()))),
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
            SidebarReply::Note(name, kind, text) => window.push_notification(git_note(&name, kind, text), cx),
            SidebarReply::Chained(from, seq, target, result) => {
                // Só o diálogo que mandou este pedido recebe a resposta; fechado, ela vai à notificação.
                let open = self.sidebar.chain.as_ref().filter(|c| c.status.borrow().sent == Some(seq));
                match (result, open) {
                    (Ok(_), open) => {
                        if open.is_some() { self.sidebar.chain = None; window.close_dialog(cx); }
                        window.push_notification(git_note(&from, NotificationType::Success, tr("sidebar_chained").replace("{n}", &target)), cx);
                    }
                    (Err(error), Some(chain)) => *chain.status.borrow_mut() = Pending { sent: None,
                        error: Some(tr("sidebar_chain_failed").replace("{n}", &Self::fetch_failure(&error))) },
                    (Err(error), None) => window.push_notification(git_note(&from, NotificationType::Error,
                        tr("sidebar_chain_failed").replace("{n}", &Self::fetch_failure(&error))), cx),
                }
                cx.notify();
            }
            SidebarReply::Wrote(name, what, result) => {
                // Com status, o motivo do servidor; sem resposta, "falha na conexão". Nunca o texto de entrega de conversa, que o
                // `failure` dá a um 5xx de POST.
                let failed = |key: &str, error: &Failure| tr(key).replace("{n}", &Self::fetch_failure(error));
                // Diálogo desta sessão ainda aberto (não cancelado): o resultado aparece nele, não em notificação solta.
                let in_dialog = match what {
                    Write::Rename(_, seq) => self.sidebar.editing.as_ref().is_some_and(|e| !e.inline && e.status.borrow().sent == Some(seq) && e.old == name),
                    _ => false,
                };
                let note = match (&what, result) {
                    (Write::Rename(new, _), Ok(value)) => {
                        self.sidebar.renaming.remove(&name);
                        let new = value.get("name").and_then(Value::as_str).unwrap_or(new).to_owned();
                        if in_dialog {
                            self.sidebar.editing = None;
                            window.close_dialog(cx);
                            // Foco na aba do nome novo; ainda fora da lista, ela o recebe quando aparecer.
                            match self.tab_focus.get(&new) {
                                Some(focus) => focus.focus(window, cx),
                                None => { self.root_focus.focus(window, cx); self.sidebar.focus_tab = Some(new.clone()); }
                            }
                        }
                        let lost = self.sidebar.lost.as_deref() == Some(name.as_str());
                        if lost { self.sidebar.lost = None; }
                        if self.selected.as_ref().is_some_and(|s| s.name == name) || (lost && self.selected.is_none()) {
                            match self.sessions.iter().find(|s| s.name == new).cloned() {
                                Some(s) => { self.error = None; self.select(s, window, cx); }
                                None => self.sidebar.follow = Some(new),
                            }
                        }
                        None
                    }
                    (Write::Rename(..), Err(error)) => {
                        self.sidebar.renaming.remove(&name);
                        // Sumiu da lista e o renomear falhou: aí sim ela foi encerrada.
                        if self.sidebar.lost.as_deref() == Some(name.as_str()) {
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
                    (Write::Delete, Ok(value)) => {
                        // Fechou, mas um par do grupo não foi avisado: o motivo aparece em vez de fechar mudo.
                        value.get("warning").filter(|w| !w.is_null()).map(|w| Notification::warning(
                            w.get("msg").and_then(Value::as_str).map(str::to_owned).unwrap_or_else(|| w.to_string())))
                    }
                    (Write::Delete, Err(error)) => {
                        self.sidebar.deleting.remove(&name);
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
                menu.label(tr("sidebar_group_by")).item(pick("sidebar_group_none", false)).item(pick("sidebar_group_project", true))
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

/// O menu da sessão, o mesmo no clique direito da linha, no ⋯ e na aba. Na ordem do web; Copiar e Abrir só com cwd.
/// O item Silenciar acompanha a leitura: o menu se refaz quando ela chega.
pub(super) fn session_menu(hangar: WeakEntity<Hangar>, session: SessionInfo) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
    move |menu, window, cx| {
        let Some(entity) = hangar.upgrade() else { return menu };
        // As outras sessões deste servidor, na ordem da barra sem o filtro, são as candidatas do encadear (web: `chainCandidates`,
        // que lê os grupos inteiros).
        let others = |hangar: &Hangar, name: &str| -> Vec<String> {
            let l = layout(&hangar.sessions, "", Hangar::by_project(), &hangar.sidebar.deleting);
            l.waiting.iter().chain(l.groups.iter().flat_map(|g| g.sessions.iter())).filter(|s| s.name != name).map(|s| s.name.clone()).collect()
        };
        let seen = Rc::new(RefCell::new(entity.read(cx).sidebar.menu_for(&session.name)));
        let (weak, again, seen_now) = (hangar.clone(), session.clone(), seen.clone());
        cx.observe_in(&entity, window, move |menu, entity, window, cx| {
            let now = entity.read(cx).sidebar.menu_for(&again.name);
            if *seen_now.borrow() == now { return; }
            *seen_now.borrow_mut() = now.clone();
            let (weak, again, list) = (weak.clone(), again.clone(), others(entity.read(cx), &again.name));
            menu.rebuild(window, cx, move |menu, window, cx| fill_menu(menu, &weak, &again, now, list, window, cx));
        }).detach();
        let view = seen.borrow().clone();
        let list = others(entity.read(cx), &session.name);
        fill_menu(menu, &hangar, &session, view, list, window, cx)
    }
}

fn fill_menu(menu: PopupMenu, hangar: &WeakEntity<Hangar>, session: &SessionInfo, mute: Option<Mute>, others: Vec<String>,
    window: &mut Window, cx: &mut Context<PopupMenu>) -> PopupMenu {
    let item = |label: String, act: fn(&mut Hangar, String, &mut Window, &mut Context<Hangar>)| {
        let (hangar, name) = (hangar.clone(), session.name.clone());
        PopupMenuItem::new(label).on_click(move |_, window, cx| { let _ = hangar.update(cx, |this, cx| act(this, name.clone(), window, cx)); })
    };
    let mute_item = match mute {
        Some(Mute::Known(muted)) => {
            let (hangar, name) = (hangar.clone(), session.name.clone());
            PopupMenuItem::new(tr(if muted { "sidebar_unmute" } else { "sidebar_mute" }))
                .on_click(move |_, _, cx| { let _ = hangar.update(cx, |this, cx| this.write(name.clone(), Write::Mute(!muted), cx)); })
        }
        // Sem leitura confirmada o item não grava: carregando mostra "…", falha mostra o motivo.
        Some(Mute::Failed(reason)) => PopupMenuItem::new(tr("sidebar_mute_unread").replace("{n}", &reason)).disabled(true),
        Some(Mute::Loading) | None => PopupMenuItem::new(format!("{}…", tr("sidebar_mute"))).disabled(true),
    };
    let cwd = session.cwd.clone().filter(|c| !c.is_empty());
    let close = {
        let (hangar, name) = (hangar.clone(), session.name.clone());
        PopupMenuItem::element(|_, _| div().text_color(theme::danger()).child(tr("sidebar_close")))
            .on_click(move |_, window, cx| { let _ = hangar.update(cx, |this, cx| this.confirm_delete(name.clone(), window, cx)); })
    };
    let git = has_git(session);
    let chain_label = match &session.then_target {
        Some(target) => tr("sidebar_chained_to").replace("{n}", target),
        None => tr("sidebar_chain"),
    };
    let (weak, name, current) = (hangar.clone(), session.name.clone(), session.then_target.clone());
    menu.min_w(px(200.))
        .item(item(tr("sidebar_rename"), |this, name, window, cx| this.start_session_rename(name, window, cx)))
        .item(mute_item)
        .when_some(cwd, |menu, cwd| menu
            .item(PopupMenuItem::new(tr("sidebar_copy_cwd")).on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(cwd.clone()))))
            .item(item(tr("sidebar_open_editor"), |this, name, _, cx| this.write(name, Write::Editor, cx))))
        .when(git, |menu| {
            let (weak, name) = (hangar.clone(), session.name.clone());
            menu.separator()
                .item(item(tr("sidebar_git"), |this, name, window, cx| this.open_git(name, window, cx)))
                .item(item(tr("sidebar_git_pull"), |this, name, window, cx| this.git_write(name, GitWrite::Pull, window, cx)))
                .submenu(tr("sidebar_switch_branch"), window, cx, move |menu, window, cx| branch_menu(menu, &weak, &name, window, cx))
        })
        .separator()
        .submenu(chain_label, window, cx, move |menu, _, _| fill_chain(menu, &weak, &name, current.clone(), &others))
        .separator()
        .item(close)
}

/// O submenu de branches se refaz sozinho quando a leitura chega: refazer o menu de cima fecharia o submenu aberto.
fn branch_menu(menu: PopupMenu, hangar: &WeakEntity<Hangar>, name: &str, window: &mut Window, cx: &mut Context<PopupMenu>) -> PopupMenu {
    let Some(entity) = hangar.upgrade() else { return menu };
    let seen = RefCell::new(entity.read(cx).sidebar.branches_for(name));
    let now = seen.borrow().clone();
    let (weak, owner) = (hangar.clone(), name.to_owned());
    cx.observe_in(&entity, window, move |menu, entity, window, cx| {
        let now = entity.read(cx).sidebar.branches_for(&owner);
        if *seen.borrow() == now { return; }
        *seen.borrow_mut() = now.clone();
        let (weak, owner) = (weak.clone(), owner.clone());
        menu.rebuild(window, cx, move |menu, _, _| fill_branches(menu, &weak, &owner, now));
    }).detach();
    fill_branches(menu, hangar, name, now)
}

/// Submenu "Trocar branch": o estado da leitura feita ao abrir o menu; a atual com ✓ e sem ação.
fn fill_branches(menu: PopupMenu, hangar: &WeakEntity<Hangar>, name: &str, branches: Option<Branches>) -> PopupMenu {
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
        let (hangar, name, branch, dirty) = (hangar.clone(), name.to_owned(), branch.clone(), list.dirty);
        menu.item(mono_item(branch.clone(), current).on_click(move |_, window, cx| {
            if current { return; }
            let _ = hangar.update(cx, |this, cx| this.pick_branch(name.clone(), branch.clone(), dirty, window, cx));
        }))
    })
}

/// Nome de branch ou de sessão em fonte mono, como o web; o atual com ✓ e na cor de destaque.
fn mono_item(text: String, current: bool) -> PopupMenuItem {
    PopupMenuItem::element(move |_, _| div().font_family(theme::MONO).text_sm().when(current, |el| el.text_color(theme::accent_text())).child(text.clone()))
        .checked(current)
}

/// Submenu do encadear: as outras sessões (✓ no alvo atual) e, com alvo, o Remover vínculo.
fn fill_chain(menu: PopupMenu, hangar: &WeakEntity<Hangar>, name: &str, current: Option<String>, others: &[String]) -> PopupMenu {
    let menu = if others.is_empty() { menu.item(PopupMenuItem::new(tr("sidebar_no_other")).disabled(true)) } else {
        others.iter().fold(menu.min_w(px(220.)).max_h(px(260.)).scrollable(true), |menu, target| {
            let (hangar, name, target) = (hangar.clone(), name.to_owned(), target.clone());
            menu.item(mono_item(target.clone(), current.as_deref() == Some(target.as_str())).on_click(move |_, window, cx| {
                let _ = hangar.update(cx, |this, cx| this.start_chain(name.clone(), target.clone(), window, cx));
            }))
        })
    };
    let (hangar, name) = (hangar.clone(), name.to_owned());
    menu.when(current.is_some(), |menu| menu.separator().item(PopupMenuItem::element(|_, _| div().text_color(theme::danger()).child(tr("sidebar_unlink")))
        .on_click(move |_, window, cx| { let _ = hangar.update(cx, |this, cx| this.git_write(name.clone(), GitWrite::Unlink, window, cx)); })))
}

#[cfg(test)]
mod tests {
    // Sem glob: o `test` do gpui_kit, que o `super::*` traz, esconderia o `#[test]` da linguagem.
    use super::{BranchList, HashSet, SessionInfo, first_line, has_git, layout, save_collapsed};

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
        assert_eq!(p.groups.len(), 1, "/p/a e /p/a/ são o mesmo projeto; Beta fica só em Aguardando");
        assert_eq!((p.groups[0].key.as_str(), p.groups[0].label.as_str()), ("/p/a", "a"));
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
