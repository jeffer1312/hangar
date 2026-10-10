//! Voz nativa na barra de cima: a pílula, o painel e a ponte entre a chamada (`crate::voice`) e a sessão na tela.
use super::*;
use std::collections::VecDeque;
use serde::Deserialize;
use gpui_kit::component::{select::{Select, SelectEvent, SelectState}, searchable_list::SearchableListItem};
use crate::voice::{Activity as CallActivity, Backstage, CallId, Phase, Voice, VoiceEvent, VoiceFailure, VoiceOptions, rpc::Codex,
    organizer::{ConfirmGate, DEFAULT_EFFORT, Effective, Mode, ModeModel, ModeModels, OpenRequest, OrganizerAction, SWITCH_REFUSED, TIER_FAST, TIER_STANDARD,
        family_root, mentions_session, name_tokens, session_context, squash, switch_requested, tool_reply, word_fits}, usage::RateWindow};
use super::{create::choices::{PERMISSIONS, checked_choice, creation_defaults}, grouping::{can_leave, can_pair}, sidebar::Target};

/// Só as vozes que o Realtime v3 aceita; as outras do esquema do Codex derrubam a chamada. Vazio é o padrão do Codex.
const VOICES: [&str; 9] = ["arbor", "breeze", "cove", "ember", "juniper", "maple", "sol", "spruce", "vale"];

#[derive(Clone)]
pub(super) struct VoiceChoice { id: String }

impl SearchableListItem for VoiceChoice {
    type Value = String;
    fn title(&self) -> SharedString { if self.id.is_empty() { tr_shared("codex_voice_default", &[]).into() } else { self.id.clone().into() } }
    fn value(&self) -> &String { &self.id }
}

/// Conta Codex desta máquina: `home` é o que o id `codex:<home>` do backend carrega; vazio é a conta padrão.
/// `id`: o `codex_account` do backend (`default` na padrão), que o catálogo de modelos pede.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct CodexAccount { home: String, label: String, id: String }

impl SearchableListItem for CodexAccount {
    type Value = String;
    fn title(&self) -> SharedString {
        match (self.home.is_empty(), self.label.is_empty()) {
            (false, _) => self.label.clone().into(),
            (true, true) => tr("voice_account_default").into(),
            (true, false) => tr("voice_account_default_named").replace("{account}", &self.label).into(),
        }
    }
    fn value(&self) -> &String { &self.home }
}

/// Primeiro item é sempre a conta padrão (`home` vazio, rótulo da conta ativa quando a lista a traz), depois as outras contas
/// Codex com pasta própria; o rótulo é o da tela de contas (apelido, senão e-mail, senão nome).
fn codex_accounts(list: &Value) -> Vec<CodexAccount> {
    let text = |v: &Value| v.as_str().filter(|t| !t.is_empty()).map(str::to_owned);
    let mut default = CodexAccount { home: String::new(), label: String::new(), id: "default".into() };
    let mut others = Vec::new();
    // `codex_account` só existe nas contas com pasta; as de cota avulsa não servem de CODEX_HOME.
    for c in list.as_array().into_iter().flatten().filter(|c| c["tipo"] == "codex" && text(&c["codex_account"]).is_some()) {
        let Some(home) = c["id"].as_str().and_then(|i| i.strip_prefix("codex:")).filter(|h| !h.is_empty()) else { continue };
        let label = text(&c["apelido"]).or_else(|| text(&c["login"]["email"])).or_else(|| text(&c["nome"])).unwrap_or_else(|| home.to_owned());
        if c["ativa"].as_bool() == Some(true) { default.label = label; }
        else { others.push(CodexAccount { home: crate::app::disk::plain_path(home), label, id: text(&c["codex_account"]).unwrap_or_default() }); }
    }
    std::iter::once(default).chain(others).collect()
}

/// `None` = sem escolha ou a conta não está na lista; quem inicia a chamada distingue os dois pelo valor salvo.
fn chosen_home(accounts: &[CodexAccount], saved: Option<&str>) -> Option<std::path::PathBuf> {
    let saved = saved.filter(|s| !s.is_empty())?;
    accounts.iter().find(|a| a.home == saved).map(|a| std::path::PathBuf::from(&a.home))
}

/// Modelo do catálogo Codex da conta escolhida (`/api/model-options`, o mesmo do diálogo de criar).
#[derive(Clone, Debug, Deserialize)]
pub(super) struct OrganizerModel { id: String, name: Option<String>, #[serde(default)] efforts: Vec<String>, #[serde(default)] service_tiers: Vec<ServiceTier> }

#[derive(Clone, Debug, Deserialize)]
pub(super) struct ServiceTier { id: String }

/// O modelo aceita Fast; sem modelo escolhido (o do config) ou fora do catálogo não se sabe, e a escolha fica livre.
fn fast_known_unavailable(models: &[OrganizerModel], model: Option<&str>) -> bool {
    model.and_then(|m| models.iter().find(|o| o.id == m)).is_some_and(|o| !o.service_tiers.iter().any(|t| t.id == TIER_FAST))
}

/// Velocidade gravada → item do seletor: vazio é a da conta.
fn tier_id(tier: Option<&str>) -> String { tier.unwrap_or_default().to_owned() }

/// Nome curto da velocidade efetiva; o Codex aceita `fast` como apelido de `priority`.
fn tier_label(tier: Option<&str>) -> String {
    match tier { Some(TIER_FAST | "fast") => tr("ctl_fast"), Some("flex") => "Flex".into(), _ => tr("voice_tier_standard") }
}

/// Últimas linhas guardadas dos bastidores.
const BACKSTAGE_KEEP: usize = 60;

/// Item dos seletores do organizador; `id` vazio é o modelo do config do Codex.
#[derive(Clone)]
pub(super) struct OrganizerChoice { id: String, label: String }

impl SearchableListItem for OrganizerChoice {
    type Value = String;
    fn title(&self) -> SharedString { self.label.clone().into() }
    fn value(&self) -> &String { &self.id }
}

/// Esforços do modelo escolhido; sem escolha (o modelo do config não se sabe antes da chamada) ou sem lista, os três básicos.
fn organizer_efforts(models: &[OrganizerModel], model: Option<&str>) -> Vec<String> {
    model.and_then(|m| models.iter().find(|o| o.id == m)).map(|o| o.efforts.clone()).filter(|e| !e.is_empty())
        .unwrap_or_else(|| ["low", "medium", "high"].map(str::to_owned).to_vec())
}

/// O esforço gravado se o modelo o aceita; senão o padrão, senão o primeiro da lista.
fn fit_effort(efforts: &[String], current: Option<&str>) -> String {
    [current.unwrap_or(DEFAULT_EFFORT), DEFAULT_EFFORT].into_iter().find(|e| efforts.iter().any(|x| x == e)).map(str::to_owned)
        .or_else(|| efforts.first().cloned()).unwrap_or_else(|| DEFAULT_EFFORT.to_owned())
}

/// Preferências da voz gravadas neste computador.
#[derive(Debug, Default, PartialEq)]
pub(super) struct SavedVoice { voice: Option<String>, account: Option<String>, organizer: ModeModels }

/// O par do Direto mora nas chaves antigas (`organizer_model`/`organizer_effort`); sem `organizer_plan` gravado, o Planejar
/// nasce igual ao Direto, e assim o arquivo de um par só vale para os dois modos.
fn parse_saved_voice(value: &Value) -> SavedVoice {
    let text = |v: &Value, k: &str| v[k].as_str().filter(|v| !v.is_empty()).map(str::to_owned);
    let pair = |v: &Value, model: &str, effort: &str, tier: &str| ModeModel { model: text(v, model),
        effort: text(v, effort).unwrap_or_else(|| DEFAULT_EFFORT.to_owned()), tier: text(v, tier) };
    let direct = pair(value, "organizer_model", "organizer_effort", "organizer_tier");
    let plan = value["organizer_plan"].is_object().then(|| pair(&value["organizer_plan"], "model", "effort", "tier")).unwrap_or_else(|| direct.clone());
    SavedVoice { voice: text(value, "voice"), account: text(value, "codex_home"), organizer: ModeModels { direct, plan } }
}

/// Posição do modo nos seletores do cartão.
fn slot(mode: Mode) -> usize { match mode { Mode::Direct => 0, Mode::Plan => 1 } }

#[derive(Default)]
pub(super) struct VoiceUi {
    pub(super) accounts: Vec<CodexAccount>,
    /// Escolha gravada (home da conta); só vale enquanto estiver em `accounts`.
    pub(super) account: Option<String>,
    pub(super) account_select: Option<(Entity<SelectState<Vec<CodexAccount>>>, Subscription)>,
    /// Modelo e esforço do organizador por modo, gravados; editar na chamada vale já para o modo atual.
    pub(super) organizer: ModeModels,
    /// Catálogo da conta escolhida: `None` = lendo (ou nunca pedido, com `models_seq` 0).
    pub(super) organizer_models: Option<Result<Vec<OrganizerModel>, String>>,
    pub(super) models_seq: u64,
    /// Um seletor por modo, na ordem de `slot`.
    pub(super) model_select: [Option<(Entity<SelectState<Vec<OrganizerChoice>>>, Subscription)>; 2],
    pub(super) effort_select: [Option<(Entity<SelectState<Vec<OrganizerChoice>>>, Subscription)>; 2],
    pub(super) enabled: bool,
    pub(super) codex: Option<Codex>,
    pub(super) call: Option<Voice>,
    /// Sobe a cada chamada nova ou parada: eventos de uma chamada velha não mexem na atual.
    pub(super) generation: u64,
    pub(super) phase: Option<Phase>,
    /// Ganhos 0..1 (entrada, saída), já com o `level_gain`.
    pub(super) levels: (f32, f32),
    /// Quem aparece falando e desde quando a leitura crua concorda com isso: evita o rótulo piscar.
    pub(super) shown: Option<(Speaker, std::time::Instant)>,
    /// O que o organizador faz agora; só aparece quando ninguém está falando.
    pub(super) activity: CallActivity,
    /// Resumo do raciocínio do turno em curso (cauda) e a ação que o organizador executa; o fim do turno limpa.
    pub(super) thought: String,
    pub(super) action: Option<OrganizerAction>,
    /// Passo da animação de espera: muda a cada `ANIM_STEP` e força a repintura entre níveis iguais.
    pub(super) anim_step: u128,
    /// Quando a chamada ficou ao vivo: base do cronômetro.
    pub(super) live_since: Option<std::time::Instant>,
    /// Repinta o cronômetro a cada segundo; largar a Task para o relógio.
    pub(super) ticker: Option<Task<()>>,
    /// Conta repinturas das barras: semente do tremor do equalizador.
    pub(super) frame: u64,
    pub(super) draft: Option<String>,
    pub(super) error: Option<String>,
    pub(super) muted: bool,
    pub(super) voice: Option<String>,
    pub(super) open: bool,
    pub(super) target: Option<String>,
    pub(super) target_key: Option<SessionKey>,
    /// Ids de evento já falados: o mesmo texto em outro turno é outra resposta.
    pub(super) spoken: HashSet<String>,
    /// Sessão aberta cujo turno acabou antes de a resposta chegar.
    pub(super) reply_pending: Option<SessionKey>,
    /// Sobe a cada espera armada: o relógio de um turno velho não fala a resposta do seguinte.
    pub(super) reply_epoch: u64,
    pub(super) pending_sends: VecDeque<(SessionKey, String, CallId)>,
    /// Plano enviado à sessão, esperando a confirmação de entrega.
    pub(super) pending_plan: Option<(SessionKey, String)>,
    /// Pergunta do organizador à sessão da tela, esperando a resposta dela.
    pub(super) pending_question: Option<(SessionKey, std::time::Instant)>,
    /// Sessão que recebeu pedido da voz → já foi vista trabalhando.
    pub(super) watched: HashMap<SessionKey, bool>,
    /// Sessões que o usuário mandou acompanhar, `(máquina, nome)`: cada resposta delas é falada. Ficam enquanto o app
    /// estiver aberto; sem a conversa na chave, um `/clear` não as tira da lista.
    pub(super) followed: HashSet<(String, String)>,
    /// Modelo, esforço e velocidade que o organizador usa de fato na chamada.
    pub(super) effective: Option<Effective>,
    /// O que chegou ao organizador e o que ele respondeu, do mais velho ao mais novo.
    pub(super) backstage: VecDeque<Backstage>,
    pub(super) backstage_open: bool,
    pub(super) backstage_scroll: ScrollHandle,
    /// Seletor de velocidade de cada modo, na ordem de `slot`.
    pub(super) tier_select: [Option<(Entity<SelectState<Vec<OrganizerChoice>>>, Subscription)>; 2],
    pub(super) voice_select: Option<(Entity<SelectState<Vec<VoiceChoice>>>, Subscription)>,
    pub(super) mode: Mode,
    /// Arquivo e texto do plano; fica na tela depois da chamada, até a próxima começar.
    pub(super) plan: Option<(std::path::PathBuf, String)>,
    pub(super) plan_scroll: ScrollHandle,
    /// Plano aberto ou fechado pelo usuário; `None` segue o modo (aberto só no Planejar).
    pub(super) plan_open: Option<bool>,
    pub(super) settings_open: bool,
    /// Rolagem do miolo do cartão, entre o estado da chamada e os botões.
    pub(super) body_scroll: ScrollHandle,
    /// Contexto usado e janela do organizador; fica depois da chamada, até a próxima começar.
    pub(super) context: Option<(u64, Option<u64>)>,
    pub(super) five_hour: RateWindow,
    pub(super) seven_day: RateWindow,
    /// Fechar por voz: o pedido armado à espera do sim falado e a chamada que espera a resposta do servidor.
    pub(super) close_gate: ConfirmGate<Target>,
    /// Clique por voz num botão arriscado: armado à espera do sim falado.
    pub(super) click_gate: ConfirmGate<String>,
    /// Sessões oferecidas na pergunta "qual?" de uma troca ambígua.
    pub(super) switch_offer: SwitchOffer,
    /// Quando a voz falou de cada sessão `(máquina, nome)` pela última vez (resumo, leitura ou resposta falada): troca por
    /// voz pergunta "quer um resumo?" só se faz tempo. A pergunta também conta, para não repetir num vai e volta.
    pub(super) talked: HashMap<(String, String), std::time::Instant>,
    /// Última troca feita pelo Jev `(máquina, nome, quando)`: o `switch_session` do organizador para a mesma sessão logo
    /// depois responde "já trocou, fique calado", em vez de "já estou nela", que ele falava como se trocasse agora.
    pub(super) jev_switched: Option<(String, String, std::time::Instant)>,
    /// Jev da configuração do servidor, lido ao ligar a chamada; `None` = sem chave, e tudo segue pelo organizador.
    pub(super) jev: Option<crate::voice::jev::Config>,
    pub(super) close_reply: Option<(Target, CallId)>,
    /// Últimos nomes de sessão passados à chamada; só lista diferente vai de novo.
    pub(super) session_names: Vec<String>,
    /// A última fala e a conversa que veio com ela: sessão citada ali desempata um nome parcial ("a HCC").
    pub(super) heard_recent: String,
    /// Sessões que já receberam o pedido de cada turno: o mesmo pedido não sai duas vezes para a mesma sessão.
    pub(super) sent_turn: Option<(String, HashSet<SessionKey>)>,
    /// Objetivo do `computer` em curso; abortar mata o HCC (parar a chamada usa isto).
    pub(super) computer: Option<tokio::task::JoinHandle<()>>,
    /// Árvore de acessibilidade mantida sem leitor de tela: na chamada (para o `read_screen`) ou com `HANGAR_A11Y_DUMP`.
    pub(super) a11y_retained: bool,
    pub(super) a11y_dump: bool,
}

/// Teto do texto que o `read_screen` devolve ao organizador.
const SCREEN_BUDGET: usize = 12_000;
/// Espera antes de ler de novo uma área que acabou de abrir: alguns quadros, com a animação de entrada.
const SCREEN_RETRY: Duration = Duration::from_millis(400);

/// `(profundidade, papel, id)` de cada linha do retrato de acessibilidade que tem `#id`.
fn snapshot_ids(snapshot: &str) -> Vec<(usize, &str, &str)> {
    snapshot.lines().filter_map(|line| {
        let body = line.trim_start();
        let (_, id) = body.rsplit_once(" #").filter(|(_, id)| !id.is_empty() && !id.contains([' ', '"']))?;
        Some(((line.len() - body.len()) / 2, body.split(' ').next().unwrap_or_default(), id))
    }).collect()
}

/// Raiz do `read_screen`: a área pedida, senão as Configurações abertas, senão o diálogo ou sobreposição aberto, senão
/// a janela inteira (`None`). Área que não está na tela volta com os ids de cima para o organizador escolher.
fn screen_root(snapshot: &str, area: Option<&str>) -> Result<Option<String>, String> {
    let ids = snapshot_ids(snapshot);
    if let Some(area) = area.map(|a| a.trim().trim_start_matches('#')).filter(|a| !a.is_empty()) {
        if ids.iter().any(|(_, _, id)| *id == area) { return Ok(Some(area.to_owned())); }
        let mut depths: Vec<usize> = ids.iter().map(|(d, ..)| *d).collect();
        depths.sort_unstable();
        depths.dedup();
        let top = depths.get(2).or(depths.last()).copied().unwrap_or(0);
        let mut shown: Vec<&str> = Vec::new();
        for (_, _, id) in ids.iter().filter(|(d, ..)| *d <= top) { if !shown.contains(id) && shown.len() < 40 { shown.push(id); } }
        return Err(format!("A área {area} não está na tela. Áreas visíveis: {}.", shown.join(", ")));
    }
    if ids.iter().any(|(_, _, id)| *id == "settings-dialog") { return Ok(Some("settings-dialog".into())); }
    Ok(ids.iter().find(|(_, role, id)| matches!(*role, "Dialog" | "AlertDialog") || id.ends_with("-dialog") || id.ends_with("-overlay"))
        .map(|(_, _, id)| (*id).to_owned()))
}

/// Palavras de botão que muda algo difícil de desfazer ou que fala por ele: o clique por voz pede o sim antes.
const RISKY_CLICK: [&str; 46] = ["apagar", "apague", "excluir", "exclua", "remover", "remova", "deletar", "delete", "remove", "descartar", "discard",
    "encerrar", "encerra", "kill", "matar", "sair", "logout", "desconectar", "disconnect", "resetar", "reset", "limpar", "clear", "desinstalar",
    "uninstall", "revogar", "revoke", "parar", "stop", "interromper", "enviar", "envie", "send", "submit", "publicar", "publish", "push", "commit",
    "merge", "confirmar", "confirm", "aplicar", "apply", "instalar", "install", "reiniciar"];

/// O clique por id é arriscado quando o id ou o nome do botão tem uma dessas palavras, ou fecha uma sessão.
pub(super) fn risky_click(id: &str, label: Option<&str>) -> bool {
    let words: Vec<String> = id.split(|c: char| !c.is_alphanumeric()).chain(label.unwrap_or_default().split(|c: char| !c.is_alphanumeric()))
        .map(squash).filter(|w| !w.is_empty()).collect();
    let has = |w: &str| words.iter().any(|x| x == w);
    words.iter().any(|w| RISKY_CLICK.contains(&w.as_str())) || ((has("fechar") || has("close")) && (has("sessao") || has("session")))
}

/// Linhas inteiras até o teto; o corte é dito no fim.
fn clip_lines(text: &str, budget: usize) -> String {
    if text.len() <= budget { return text.to_owned(); }
    let mut out = String::new();
    let mut kept = 0;
    for line in text.lines() {
        if out.len() + line.len() + 1 > budget { break; }
        out.push_str(line);
        out.push('\n');
        kept += 1;
    }
    out + &format!("[cortado: faltam {} linhas; peça uma área menor]\n", text.lines().count() - kept)
}

/// O que o `read_screen` lê: `(raiz, texto)`; `None` na raiz é a janela inteira.
fn read_screen(window: &Window, area: Option<&str>) -> Result<(Option<String>, String), String> {
    const NOT_READY: &str = "A tela ainda não tem árvore de acessibilidade; tente de novo em um instante.";
    let all = window.a11y_snapshot(None).ok_or(NOT_READY)?;
    let root = screen_root(&all, area)?;
    let text = match &root { Some(root) => window.a11y_snapshot(Some(root)).ok_or(NOT_READY)?, None => all };
    Ok((root, clip_lines(&text, SCREEN_BUDGET)))
}

/// Uma ação da tela do Hangar que a voz executa pelo mesmo caminho do botão; o id é o de acessibilidade dele.
pub(super) struct UiAction { pub(super) id: &'static str, pub(super) label: &'static str, pub(super) description: &'static str }

pub(super) const HANGAR_ACTIONS: [UiAction; 16] = [
    UiAction { id: "topbar-settings", label: "Abrir configurações", description: "Abre a tela de configurações; arg opcional = seção (veja as seções abaixo)." },
    UiAction { id: "settings-back", label: "Fechar configurações", description: "Fecha a tela de configurações e volta à conversa." },
    UiAction { id: "sidebar-new-session", label: "Nova sessão", description: "Abre o diálogo de criar sessão (a pessoa escolhe e confirma)." },
    UiAction { id: "sidebar-fold", label: "Recolher ou mostrar a barra de sessões", description: "Alterna a barra lateral de sessões entre recolhida e aberta." },
    UiAction { id: "side-toggle", label: "Mostrar ou esconder o painel lateral", description: "Alterna o painel lateral da sessão (Contexto, Arquivos, Atividade, Git)." },
    UiAction { id: "side-tab-context", label: "Painel: Contexto", description: "Abre o painel lateral na aba Contexto da sessão ativa." },
    UiAction { id: "side-tab-files", label: "Painel: Arquivos", description: "Abre o painel lateral na aba Arquivos da sessão ativa." },
    UiAction { id: "side-tab-activity", label: "Painel: Atividade", description: "Abre o painel lateral na aba Atividade, quando a sessão tem." },
    UiAction { id: "side-tab-git", label: "Painel: Git", description: "Abre o painel lateral na aba Git, quando a pasta é um repositório." },
    UiAction { id: "terminal-show", label: "Abrir o terminal", description: "Abre o painel de terminal da sessão ativa." },
    UiAction { id: "terminal-close", label: "Fechar o terminal", description: "Fecha o painel de terminal." },
    UiAction { id: "topbar-voice", label: "Abrir o cartão da voz", description: "Mostra o cartão desta conversa por voz." },
    UiAction { id: "voice-panel-close", label: "Fechar o cartão da voz", description: "Esconde o cartão da voz; a conversa continua." },
    UiAction { id: "topbar-cost", label: "Abrir custos", description: "Abre a página de custos." },
    UiAction { id: "costs-usage", label: "Abrir estatísticas de uso", description: "Abre as estatísticas de uso (na página de custos)." },
    UiAction { id: "costs-back", label: "Fechar custos", description: "Fecha a página de custos e estatísticas." },
];

pub(super) fn hangar_actions_text(sections: &[&str]) -> String {
    let mut lines: Vec<String> = HANGAR_ACTIONS.iter().map(|a| format!("- {}: {}. {}", a.id, a.label, a.description)).collect();
    lines.push(format!("Seções de configurações (arg de topbar-settings): {}.", sections.join(", ")));
    lines.join("\n")
}

/// Quanto tempo o `switch_session` do organizador para a sessão que o Jev acabou de abrir conta como eco da mesma fala.
const JEV_SWITCH_ECHO: Duration = Duration::from_secs(20);

/// Quanto tempo depois de falar de uma sessão a troca por voz não oferece resumo dela de novo.
const TALKED_WINDOW: Duration = Duration::from_secs(15 * 60);

/// Troca por voz oferece resumo? Só se a voz não falou dessa sessão há pouco; oferecer conta como ter falado.
pub(super) fn offer_summary(talked: &mut HashMap<(String, String), std::time::Instant>, session: (String, String), now: std::time::Instant) -> bool {
    let recent = talked.get(&session).is_some_and(|at| now.saturating_duration_since(*at) < TALKED_WINDOW);
    if !recent { talked.insert(session, now); }
    !recent
}

/// Troca que o Jev pode fazer sozinho: a sessão escolhida, ou o motivo de não agir (vai ao diário como código).
pub(super) fn jev_switch_choice(decision: &crate::voice::jev::Decision) -> Result<usize, &'static str> {
    use crate::voice::jev::{Intent, SESSION_SURE, SWITCH_INTENT};
    match &decision.intent {
        Some((Intent::Switch, p)) if *p >= SWITCH_INTENT => {}
        Some((Intent::Switch, _)) => return Err("low-intent"),
        _ => return Err("not-switch"),
    }
    match decision.session { Some((at, p)) if p >= SESSION_SURE => Ok(at), Some(_) => Err("low-session"), None => Err("no-session") }
}

/// Ação de tela que o Jev pode fazer sozinho: a ação escolhida, ou o motivo de não agir.
pub(super) fn jev_action_choice(decision: &crate::voice::jev::Decision) -> Result<usize, &'static str> {
    use crate::voice::jev::{ACTION_SURE, Intent};
    match &decision.intent {
        Some((Intent::Screen, p)) if *p >= ACTION_SURE => {}
        Some((Intent::Screen, _)) => return Err("low-intent"),
        _ => return Err("not-screen"),
    }
    match decision.action { Some((at, p)) if p >= ACTION_SURE => Ok(at), Some(_) => Err("low-action"), None => Err("no-action") }
}

/// O que foi perguntado ao Jev sobre uma fala: as sessões (máquina, nome) e as ações na ordem das opções.
pub(super) struct JevAsked { turn: String, speech: String, recent: String, sessions: Vec<(String, String)>, actions: Vec<&'static str>, started: std::time::Instant }

/// Resultado assíncrono de uma ferramenta de sessão; volta à tela com a chamada que espera a resposta.
/// `Opened`: máquina, criação e, com `request`, o pedido e a entrega dele à sessão nova.
pub(super) enum VoiceDone { Opened(String, Result<SessionInfo, String>, Option<(String, Result<Delivery, Failure>)>), Grouped(&'static str, Result<PairResult, Failure>),
    Computer(Result<String, String>), Observed(Result<String, String>) }

/// Uma linha do `list_sessions`.
pub(super) struct Listed { pub(super) name: String, pub(super) machine: Option<String>, pub(super) provider: String, pub(super) state: String,
    pub(super) folder: String, pub(super) on_screen: bool, pub(super) followed: bool }

pub(super) fn sessions_text(rows: &[Listed], unreachable: &[String]) -> String {
    let mut lines: Vec<String> = rows.iter().map(|r| {
        let mut line = format!("- {}", r.name);
        if let Some(machine) = &r.machine { line.push_str(&format!(" (máquina {machine})")); }
        line.push_str(&format!(": {}, {}", r.provider, if r.state.is_empty() { "?" } else { &r.state }));
        if !r.folder.is_empty() { line.push_str(&format!(", pasta {}", r.folder)); }
        if r.on_screen { line.push_str(", na tela"); }
        if r.followed { line.push_str(", acompanhada"); }
        line
    }).collect();
    if lines.is_empty() { lines.push("Nenhuma sessão aberta.".into()); }
    // Lista que falhou não é "sem sessões": o organizador precisa saber que faltam as de lá.
    lines.extend(unreachable.iter().map(|m| format!("Máquina {m} sem resposta; as sessões dela não estão aqui.")));
    lines.join("\n")
}

/// Caminho já dito como caminho (Unix, `~`, Windows): vai direto ao backend, sem procurar pelo nome.
pub(super) fn looks_like_path(text: &str) -> bool {
    text.starts_with(['/', '~', '\\']) || matches!(text.get(1..3), Some(":\\" | ":/"))
}

/// Nome falado → pasta, entre as raízes e as subpastas delas (`(nome, caminho)`, a lista da tela de criação).
pub(super) fn pick_folder(spoken: &str, folders: &[(String, String)], unread: usize) -> Result<String, String> {
    let mut seen = HashSet::new();
    let folders: Vec<&(String, String)> = folders.iter().filter(|(_, path)| seen.insert(path.as_str())).collect();
    let names: Vec<&str> = folders.iter().map(|(name, _)| name.as_str()).collect();
    match match_session(spoken, &names, &[]) {
        SessionMatch::One(i) => Ok(folders[i].1.clone()),
        SessionMatch::Many(found) => Err(format!("Mais de uma pasta combina: {}. Peça para o usuário dizer qual.",
            found.iter().take(5).map(|&i| folders[i].1.as_str()).collect::<Vec<_>>().join(", "))),
        SessionMatch::None if unread > 0 => Err(format!("Não achei pasta com esse nome; {unread} raiz(es) não puderam ser lidas.")),
        SessionMatch::None => Err("Não achei pasta com esse nome. Peça o nome exato ou o caminho.".into()),
    }
}

async fn voice_folder(api: &Api, spoken: &str) -> Result<String, String> {
    if looks_like_path(spoken) { return Ok(spoken.to_owned()); }
    let roots: Vec<super::create::Root> = api.server_read(&["fs", "roots"], &[], 15).await.map_err(|e| Hangar::fetch_failure(&e))
        .and_then(|v| serde_json::from_value(v).map_err(|_| tr("invalid_response")))?;
    let mut folders: Vec<(String, String)> = roots.iter().map(|r| (r.name.clone(), r.path.clone())).collect();
    let mut unread = 0;
    for root in &roots {
        match super::create::scan_of(api.server_read(&["fs", "scan"], &[("root", root.path.as_str())], 15).await) {
            Ok(scan) if scan.error.is_none() => folders.extend(scan.entries.into_iter().map(|e| (e.name, e.path))),
            _ => unread += 1,
        }
    }
    pick_folder(spoken, &folders, unread)
}

/// O mesmo `POST /api/sessions` da tela de criação, com o que ela traria sem toque: padrão marcado do harness ou último
/// modelo lembrado (só se ainda estiver no catálogo), permissão padrão do Claude, modo e conta padrões do servidor.
async fn create_by_voice(api: &Api, request: OpenRequest) -> Result<SessionInfo, String> {
    let cwd = voice_folder(api, &request.folder).await?;
    let provider = request.provider;
    let name = match request.name {
        Some(name) => name,
        None => {
            let taken = api.sessions().await.map_err(|e| Hangar::fetch_failure(&e))?.into_iter().map(|s| s.name).collect();
            super::create::unique_name(crate::composer::basename(&cwd), &taken)
        }
    };
    let server = api.identity();
    let (remembered, permission) = tokio::task::spawn_blocking(move || creation_defaults(&server, provider)).await.unwrap_or_default();
    let mut query = vec![("provider", provider)];
    if provider == "codex" { query.push(("codex_account", "default")); }
    let (model, effort) = match api.server_read(&["model-options"], &query, 30).await {
        Ok(catalog) => checked_choice(&catalog, provider, remembered).unwrap_or_default(),
        // Catálogo que não veio deixa o padrão do servidor, como na tela: nunca um modelo sem conferir.
        Err(error) => { crate::voice::log(format!("open_session model-options failed status={:?}", error.status)); Default::default() }
    };
    let text = |s: &str| if s.is_empty() { Value::Null } else { json!(s) };
    let mut body = json!({"name": name, "cwd": cwd, "provider": provider, "model": text(&model), "effort": text(&effort)});
    if provider == "claude" {
        let permission = permission.unwrap_or_else(|| "bypassPermissions".into());
        if PERMISSIONS.contains(&permission.as_str()) { body["permission_mode"] = json!(permission); }
    }
    let result = api.server_send(reqwest::Method::POST, &["sessions"], Some(body), 120).await;
    super::create::opened(api, result, &name, &cwd, None).await.map(|opened| opened.session)
}

#[derive(Debug, PartialEq)]
pub(super) enum SessionMatch { One(usize), Many(Vec<usize>), None }

/// Nome falado → sessão. Igual vence parcial; o mesmo nome em duas máquinas fica com o da ativa (`on_active`).
pub(super) fn match_session(query: &str, names: &[&str], on_active: &[bool]) -> SessionMatch {
    let original = query;
    let query = squash(query);
    if query.is_empty() { return SessionMatch::None; }
    let squashed: Vec<String> = names.iter().map(|n| squash(n)).collect();
    let pick = |hits: Vec<usize>| match hits.as_slice() { [] => SessionMatch::None, [one] => SessionMatch::One(*one), _ => SessionMatch::Many(hits) };
    let exact: Vec<usize> = (0..names.len()).filter(|&i| squashed[i] == query).collect();
    if !exact.is_empty() {
        let mine: Vec<usize> = exact.iter().copied().filter(|&i| on_active.get(i) == Some(&true)).collect();
        return if mine.len() == 1 { SessionMatch::One(mine[0]) } else { pick(exact) };
    }
    let partial: Vec<usize> = (0..names.len()).filter(|&i| squashed[i].contains(&query)).collect();
    if !partial.is_empty() { return pick(partial); }
    // Palavras soltas em qualquer ordem: "plano do rust" casa com grupos-rust-plano; "do", "da", "a" não contam.
    let words: Vec<String> = original.split(|c: char| !c.is_alphanumeric()).map(squash).filter(|w| w.chars().count() > 2).collect();
    if words.len() >= 2 {
        let all: Vec<usize> = (0..names.len()).filter(|&i| words.iter().all(|w| squashed[i].contains(w.as_str()))).collect();
        if !all.is_empty() { return pick(all); }
    }
    // Aproximado, palavra por pedaço do nome: "setting ux" casa com jev-settings-ux, "seting" com settings.
    let spoken: Vec<String> = original.split(|c: char| !c.is_alphanumeric()).map(squash)
        .filter(|w| !w.is_empty() && !NAME_FILLERS.contains(&w.as_str())).collect();
    if spoken.is_empty() { return SessionMatch::None; }
    pick((0..names.len()).filter(|&i| { let tokens = name_tokens(names[i]); spoken.iter().all(|w| tokens.iter().any(|t| word_fits(w, t))) }).collect())
}

/// Palavras que acompanham o nome falado sem fazer parte dele.
const NAME_FILLERS: [&str; 12] = ["a", "o", "as", "os", "da", "do", "de", "pra", "pro", "para", "sessao", "secao"];

/// Várias sessões casam com o nome falado: uma só no contexto (citada na conversa, acompanhada, falada há pouco, a da
/// tela quando não é troca) decide; senão a raiz da família (hcc-rust-plano diante dos executores dela); senão continua
/// ambíguo e quem chama pergunta, sem escolher.
pub(super) fn narrow_by_context(found: Vec<usize>, names: &[&str], context: &[bool]) -> SessionMatch {
    let hit: Vec<usize> = (0..found.len()).filter(|&i| context[i]).collect();
    if let [one] = hit[..] { return SessionMatch::One(found[one]); }
    // Contexto dividido entre várias: a raiz só vale entre elas.
    let pool: Vec<usize> = if hit.is_empty() { (0..found.len()).collect() } else { hit };
    let pool_names: Vec<&str> = pool.iter().map(|&i| names[i]).collect();
    match family_root(&pool_names) { Some(at) => SessionMatch::One(found[pool[at]]), None => SessionMatch::Many(found) }
}

/// Nomes pelos quais a máquina pode ser dita: o rótulo dela e o host do endereço ("jefferson-felizardo" de
/// `https://jefferson-felizardo.tailnet.ts.net`), sem acento, pontuação e caixa.
pub(super) fn machine_aliases(key: &str, label: &str) -> Vec<String> {
    let host = key.rsplit("://").next().unwrap_or(key).split(['/', ':']).next().unwrap_or_default();
    let first = host.split('.').next().unwrap_or_default();
    let mut aliases: Vec<String> = [label, host, first].into_iter().map(squash).filter(|a| a.chars().count() >= 3).collect();
    aliases.dedup();
    aliases
}

/// Palavras que ligam o nome da sessão à máquina na fala ("hangar lá no PC X", "hangar na máquina X").
const MACHINE_LINKS: [&str; 15] = ["la", "no", "na", "em", "do", "da", "de", "pc", "maquina", "computador", "servidor", "sessao", "secao", "aquela", "aquele"];

/// Nome falado → sessão, aceitando a máquina junto: "X::nome", "nome (máquina X)", "nome lá no PC X". Máquina citada
/// restringe às sessões dela; sem máquina, é o `match_session` de sempre. Ambíguo continua ambíguo: quem chama pergunta.
pub(super) fn match_qualified(spoken: &str, names: &[&str], machines: &[Vec<String>], on_active: &[bool]) -> SessionMatch {
    let (machine_text, name_text) = match spoken.split_once("::") {
        Some((machine, name)) => (Some(squash(machine)), name.to_owned()),
        None => (None, spoken.to_owned()),
    };
    let said = squash(spoken);
    let named = |aliases: &Vec<String>| aliases.iter().any(|alias| match &machine_text {
        Some(m) => !m.is_empty() && (alias.contains(m.as_str()) || m.contains(alias.as_str())),
        None => said.contains(alias.as_str()),
    });
    let on_machine: Vec<usize> = (0..names.len()).filter(|&i| machines.get(i).is_some_and(named)).collect();
    if on_machine.is_empty() {
        return if machine_text.is_some() { SessionMatch::None } else { match_session(spoken, names, on_active) };
    }
    // O que sobra da fala sem as palavras da máquina e as de ligação é o nome da sessão.
    let machine_words: Vec<String> = on_machine.iter().flat_map(|&i| machines[i].iter().cloned()).collect();
    let rest: Vec<String> = name_text.split(|c: char| !c.is_alphanumeric()).map(squash)
        .filter(|w| !w.is_empty() && !MACHINE_LINKS.contains(&w.as_str()) && !machine_words.iter().any(|m| m == w || (w.chars().count() >= 4 && m.contains(w.as_str()))))
        .collect();
    let subset: Vec<&str> = on_machine.iter().map(|&i| names[i]).collect();
    let active: Vec<bool> = on_machine.iter().map(|&i| on_active.get(i).copied().unwrap_or(false)).collect();
    match match_session(&rest.join(" "), &subset, &active) {
        SessionMatch::One(at) => SessionMatch::One(on_machine[at]),
        SessionMatch::Many(found) => SessionMatch::Many(found.into_iter().map(|at| on_machine[at]).collect()),
        SessionMatch::None => SessionMatch::None,
    }
}

/// Pergunta "qual das duas?" depois de um pedido de troca: o "isso"/"a do PC" seguinte não repete o verbo de trocar,
/// e vale só para uma das sessões oferecidas, por um minuto.
#[derive(Default)]
pub(super) struct SwitchOffer { offered: Vec<(String, String)>, at: Option<std::time::Instant> }

const SWITCH_OFFER_WINDOW: Duration = Duration::from_secs(60);

impl SwitchOffer {
    pub(super) fn offer(&mut self, candidates: Vec<(String, String)>, now: std::time::Instant) { (self.offered, self.at) = (candidates, Some(now)); }
    /// `true` consome a oferta.
    pub(super) fn take(&mut self, key: &str, name: &str, now: std::time::Instant) -> bool {
        let fresh = self.at.is_some_and(|at| now.saturating_duration_since(at) < SWITCH_OFFER_WINDOW);
        let ok = fresh && self.offered.iter().any(|(k, n)| k == key && n == name);
        if ok { *self = Self::default(); }
        ok
    }
}

pub(super) fn conversation_pairs(events: &[ChatEvent]) -> Vec<(String, String)> {
    events.iter().filter(|e| matches!(e.kind.as_str(), "user_msg" | "assistant_msg") && e.text.as_deref().is_some_and(|t| !t.trim().is_empty()))
        .map(|e| (e.kind.as_str().to_owned(), e.text.clone().unwrap_or_default())).collect()
}

/// `(id, kind, text)` → `(id da última resposta, respostas depois da última fala do usuário)`.
pub(super) fn last_reply(events: &[(String, String, String)]) -> Option<(String, String)> {
    let start = events.iter().rposition(|(_, kind, _)| kind == "user_msg").map(|i| i + 1).unwrap_or(0);
    let replies: Vec<&(String, String, String)> = events[start..].iter().filter(|(_, k, _)| k == "assistant_msg").collect();
    let last = replies.last()?;
    Some((last.0.clone(), replies.iter().map(|(_, _, t)| t.as_str()).collect::<Vec<_>>().join("\n\n")))
}

/// Quanto a pergunta do organizador espera pela sessão antes de desistir.
pub(super) const ASK_TIMEOUT: Duration = Duration::from_secs(600);
const ASK_MARK: &str = "[Pergunta da conversa de voz]";

/// A marca no começo é o que acha a pergunta no histórico.
pub(super) fn question_text(q: &str) -> String {
    format!("{ASK_MARK} {q}\nResponda curto; é para o planejamento, não execute nada.")
}

pub(super) fn question_expired(since: std::time::Instant, now: std::time::Instant) -> bool { now.saturating_duration_since(since) >= ASK_TIMEOUT }

pub(super) fn question_marked(events: &[(String, String, String)]) -> bool {
    events.iter().any(|(_, kind, text)| kind == "user_msg" && text.starts_with(ASK_MARK))
}

/// `(id, kind, text)` → `(id da última resposta, respostas)` dadas depois da última pergunta marcada e antes de outro pedido.
pub(super) fn question_answer(events: &[(String, String, String)]) -> Option<(String, String)> {
    let start = events.iter().rposition(|(_, kind, text)| kind == "user_msg" && text.starts_with(ASK_MARK))? + 1;
    let end = events[start..].iter().position(|(_, kind, _)| kind == "user_msg").map_or(events.len(), |i| start + i);
    let replies: Vec<&(String, String, String)> = events[start..end].iter().filter(|(_, k, _)| k == "assistant_msg").collect();
    let last = replies.last()?;
    Some((last.0.clone(), replies.iter().map(|(_, _, t)| t.as_str()).collect::<Vec<_>>().join("\n\n")))
}

/// Quanto o fim do turno espera pela última `assistant_msg` antes de falar o que já tem.
const REPLY_WAIT: Duration = Duration::from_millis(1500);

/// Altura (px) de uma barra da pílula, em passos inteiros: só mudança de passo repinta a janela.
fn bar_height(level: f32) -> f32 { 4. + (level.clamp(0., 1.) * 12.).round() }

/// Como o web: RMS de voz fica perto de 0,05–0,2, então ×5 enche a barra.
pub(super) fn level_gain(rms: f32) -> f32 { (rms * 5.).clamp(0., 1.) }

#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum Speaker { You, Voice, Idle }

const SPEAKING: f32 = 0.08;

/// Quem fala agora, pelos ganhos; mudo, a entrada não conta.
pub(super) fn speaker(input: f32, output: f32, muted: bool) -> Speaker {
    let input = if muted { 0. } else { input };
    if input > SPEAKING && input >= output { Speaker::You }
    else if output > SPEAKING && output > input { Speaker::Voice }
    else { Speaker::Idle }
}

/// Folga antes de trocar o rótulo de quem fala.
const SPEAKER_HOLD: Duration = Duration::from_millis(300);

/// `since` é a última vez em que a leitura crua concordou com `shown`; a troca só vale depois de a leitura nova durar `SPEAKER_HOLD`.
pub(super) fn settled_speaker(shown: Speaker, since: std::time::Instant, raw: Speaker, now: std::time::Instant) -> (Speaker, std::time::Instant) {
    if raw == shown { (shown, now) }
    else if now.saturating_duration_since(since) >= SPEAKER_HOLD { (raw, now) }
    else { (shown, since) }
}

/// Passo das animações de espera (ms).
const ANIM_STEP: u128 = 250;

/// Barras baixas acendendo em sequência, pela fase do cronômetro: `step` ms por barra, `rise` da altura útil.
pub(super) fn wave_bars(elapsed: Duration, min: f32, max: f32, step: u128, rise: f32) -> [f32; 5] {
    let lit = (elapsed.as_millis() / step % 5) as usize;
    std::array::from_fn(|i| if i == lit { min + ((max - min) * rise).round() } else { min })
}

/// Cauda do raciocínio guardada; o que aparece é ainda menor (`thought_tail`).
const THOUGHT_KEEP: usize = 2000;

pub(super) fn push_thought(thought: &mut String, delta: &str) {
    thought.push_str(delta);
    let excess = thought.len().saturating_sub(THOUGHT_KEEP);
    if excess > 0 {
        let cut = (excess..thought.len()).find(|i| thought.is_char_boundary(*i)).unwrap_or(thought.len());
        thought.drain(..cut);
    }
}

/// As últimas `lines` linhas não vazias, cada uma cortada em `width` caracteres.
pub(super) fn thought_tail(thought: &str, lines: usize, width: usize) -> Vec<String> {
    let all: Vec<&str> = thought.lines().map(|l| l.trim().trim_matches('*').trim()).filter(|l| !l.is_empty()).collect();
    all[all.len().saturating_sub(lines)..].iter().map(|l| clip(l, width)).collect()
}

fn clip(text: &str, width: usize) -> String {
    if text.chars().count() <= width { return text.to_owned(); }
    format!("{}…", text.chars().take(width - 1).collect::<String>())
}

/// Ação em curso numa linha curta.
pub(super) fn action_text(action: &OrganizerAction) -> String {
    match action {
        OrganizerAction::Tool(tool) => match tool.as_str() {
            "read_session" => tr("voice_tool_read_session"),
            "send_to_session" => tr("voice_tool_send_to_session"),
            "hold_request" => tr("voice_tool_hold_request"),
            "discard_request" => tr("voice_tool_discard_request"),
            "update_plan" => tr("voice_tool_update_plan"),
            "read_plan" => tr("voice_tool_read_plan"),
            "ask_session" => tr("voice_tool_ask_session"),
            "finish_plan" => tr("voice_tool_finish_plan"),
            "set_mode" => tr("voice_tool_set_mode"),
            "switch_session" => tr("voice_tool_switch_session"),
            "list_sessions" => tr("voice_tool_list_sessions"),
            "open_session" => tr("voice_tool_open_session"),
            "close_session" => tr("voice_tool_close_session"),
            "pair_sessions" => tr("voice_tool_pair_sessions"),
            "unpair_session" => tr("voice_tool_unpair_session"),
            "hangar_actions" => tr("voice_tool_hangar_actions"),
            "hangar_action" => tr("voice_tool_hangar_action"),
            "computer" => tr("voice_tool_computer"),
            "read_screen" => tr("voice_tool_read_screen"),
            "click_screen" => tr("voice_tool_click_screen"),
            "observe_system" => tr("voice_tool_observe_system"),
            "edit_files" => tr("voice_tool_edit_files"),
            "follow_session" | "unfollow_session" => tr("voice_tool_follow_session"),
            other => tr("voice_tool_other").replace("{tool}", other),
        },
        OrganizerAction::Search(query) if query.trim().is_empty() => tr("voice_action_search"),
        OrganizerAction::Search(query) => tr("voice_action_search_query").replace("{query}", &clip(query.trim(), 80)),
        OrganizerAction::Command(command) => {
            let line = command.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or_default();
            tr("voice_action_command").replace("{command}", &clip(line, 80))
        }
    }
}

pub(super) fn call_clock(elapsed: Duration) -> String {
    let s = elapsed.as_secs();
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{:02}:{:02}", s / 60, s % 60) }
}

const PROFILE: [f32; 5] = [0.55, 0.9, 1.0, 0.8, 0.6];

/// Alturas (px inteiros) das cinco barras; o tremor sai do contador de repinturas, então é determinístico.
pub(super) fn equalizer(level: f32, frame: u64, min: f32, max: f32) -> [f32; 5] {
    std::array::from_fn(|i| {
        let jitter = 0.85 + 0.05 * ((frame as usize + i * 3) % 4) as f32;
        min + ((max - min) * (level * PROFILE[i] * jitter).clamp(0., 1.)).round()
    })
}

/// O que o resultado diz quando a sessão parou esperando o usuário: a pergunta, ou o fim da última resposta.
pub(super) fn waiting_text(questions: &[&str], reply: Option<&str>) -> String {
    let asked = questions.iter().map(|q| q.trim()).filter(|q| !q.is_empty()).collect::<Vec<_>>().join(" ");
    let detail = if !asked.is_empty() { asked } else {
        let reply = reply.map(str::trim).unwrap_or_default();
        reply.chars().rev().take(600).collect::<Vec<_>>().into_iter().rev().collect()
    };
    if detail.is_empty() { "A sessão está esperando uma resposta ou aprovação no chat.".to_owned() }
    else { format!("A sessão está esperando sua resposta: {detail}") }
}

pub(super) fn went_idle(was_working: bool, state: &str) -> bool { was_working && state != "working" }

pub(super) fn send_reply(session: &str, result: &Result<Delivery, Failure>) -> Value {
    match result {
        Ok(d) if d.ok && d.delivered => tool_reply(format!("status sent: pedido entregue à sessão {session}. Aguarde o resultado real."), true),
        Ok(d) if d.ok => tool_reply(format!("status queued: a sessão {session} está ocupada; o pedido entrou na fila."), true),
        _ => tool_reply(format!("status failed: não foi possível confirmar a entrega à sessão {session}. Não reenvie; peça para o usuário conferir o chat."), false),
    }
}

/// Resposta do `open_session`: `shown` = abriu na tela; `sent` = entrega do pedido ditado junto. `Err` vira falha visível.
pub(super) fn opened_reply(name: &str, shown: bool, sent: Option<&Result<Delivery, Failure>>) -> Result<String, String> {
    let failed = |result: &Result<Delivery, Failure>| match result {
        Ok(d) if d.ok => None,
        Ok(_) => Some("a sessão recusou o pedido".to_owned()),
        Err(error) => Some(Hangar::fetch_failure(error)),
    };
    match (shown, sent) {
        (true, None) => Ok(format!("Sessão {name} criada e aberta na tela; a troca já foi anunciada, não repita.")),
        (false, None) => Err(format!("A sessão {name} foi criada, mas a máquina dela não está conectada para abri-la.")),
        (true, Some(result)) => match (failed(result), result) {
            (None, Ok(d)) if d.delivered => Ok(format!("Sessão {name} aberta e pedido enviado. A troca já foi anunciada, não repita; aguarde o resultado real.")),
            (None, _) => Ok(format!("Sessão {name} aberta e pedido enviado; ele entrou na fila da sessão. A troca já foi anunciada, não repita.")),
            (Some(why), _) => Err(format!("Sessão {name} aberta, mas o pedido não chegou a ela: {why}. Não reenvie; peça para o usuário conferir o chat.")),
        },
        (false, Some(result)) => Err(match failed(result) {
            None => format!("Sessão {name} criada e pedido enviado, mas a máquina dela não está conectada para abri-la na tela."),
            Some(why) => format!("Sessão {name} criada, mas não abriu na tela e o pedido não chegou a ela: {why}."),
        }),
    }
}

fn triples(events: &[ChatEvent]) -> Vec<(String, String, String)> {
    events.iter().filter(|e| matches!(e.kind.as_str(), "user_msg" | "assistant_msg"))
        .map(|e| (e.id.clone(), e.kind.as_str().to_owned(), e.text.clone().unwrap_or_default())).collect()
}

fn failure_text(failure: &VoiceFailure) -> String {
    match failure {
        VoiceFailure::Microphone => tr_shared("composer_sem_acesso_mic", &[]),
        VoiceFailure::Speaker => tr("voice_speaker"),
        VoiceFailure::AppServer => tr("voice_app_server"),
        VoiceFailure::Realtime(detail) => format!("{} {detail}", tr_shared("codex_voice_failed", &[])),
        VoiceFailure::Network => tr("voice_network"),
        VoiceFailure::Timeout => tr_shared("codex_voice_timeout", &[]),
        VoiceFailure::Organizer => tr("voice_organizer"),
        VoiceFailure::ModelSwitch => tr("voice_model_switch_failed"),
        VoiceFailure::OwnFolder => tr("voice_own_folder_failed"),
        VoiceFailure::AudioStopped => tr("voice_audio_stopped"),
        VoiceFailure::Closed => tr("voice_server_closed"),
    }
}

fn voice_file() -> Option<std::path::PathBuf> { Some(appearance::dir()?.join("voice.json")) }

fn read_saved_voice() -> SavedVoice {
    let value: Value = voice_file().and_then(|f| std::fs::read(f).ok()).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    parse_saved_voice(&value)
}

fn save_voice(saved: &SavedVoice) -> Result<(), String> {
    let path = voice_file().ok_or_else(|| tr("keyboard_no_directory"))?;
    let dir = path.parent().ok_or_else(|| tr("keyboard_no_directory"))?;
    std::fs::create_dir_all(dir).map_err(|error| error.to_string())?;
    let temporary = path.with_extension("json.tmp");
    let (direct, plan) = (&saved.organizer.direct, &saved.organizer.plan);
    let bytes = serde_json::to_vec(&json!({"voice": saved.voice, "codex_home": saved.account,
        "organizer_model": direct.model, "organizer_effort": direct.effort, "organizer_tier": direct.tier,
        "organizer_plan": {"model": plan.model, "effort": plan.effort, "tier": plan.tier}})).map_err(|error| error.to_string())?;
    std::fs::write(&temporary, bytes).and_then(|_| std::fs::rename(&temporary, &path)).map_err(|error| error.to_string())
}

/// Nome curto para a pílula.
fn short(name: &str) -> String {
    if name.chars().count() <= 18 { return name.to_owned(); }
    format!("{}…", name.chars().take(17).collect::<String>())
}

impl Hangar {
    /// O servidor desta máquina: a voz usa o Codex daqui, e a opção beta é dele.
    pub(super) fn local_api(&self) -> Option<Api> {
        if let Some(api) = self.api.as_ref().filter(|api| api.is_loopback()) { return Some(api.clone()); }
        self.servers.iter().filter(|s| !s.disabled).find_map(|s| Api::new(&s.address, &s.token).ok().filter(Api::is_loopback))
    }

    pub(super) fn refresh_voice_gate(&mut self, cx: &mut Context<Self>) {
        let Some(api) = self.local_api() else {
            self.voice.enabled = false;
            // Sem a pílula, a chamada ficaria com o microfone aberto e nenhum controle na tela.
            self.stop_voice(cx);
            return;
        };
        let (connection, tx) = (self.connection, self.tx.clone());
        self.runtime.spawn(async move {
            // find_codex roda `npm prefix -g`: fora da thread da tela.
            let codex = tokio::task::spawn_blocking(crate::voice::rpc::find_codex).await.ok().flatten();
            let saved = tokio::task::spawn_blocking(read_saved_voice).await.unwrap_or_default();
            // Leitura que falhou é `None` = sem mudança; só uma resposta válida liga ou desliga.
            let enabled = api.server_read(&["harness", "codex", "opcoes"], &[], 8).await.ok().and_then(|v| v["codex_voice_beta"].as_bool());
            let accounts = api.server_read(&["credenciais"], &[], 15).await.ok().map(|v| codex_accounts(&v));
            if enabled.is_none() || accounts.is_none() { crate::voice::log(format!("gate read failed options={} accounts={}", enabled.is_some(), accounts.is_some())); }
            let _ = tx.send(Envelope { connection, selection: None, payload: Payload::VoiceGate(enabled, codex, saved, accounts) }).await;
        });
    }

    pub(super) fn receive_voice_gate(&mut self, enabled: Option<bool>, codex: Option<Codex>, saved: SavedVoice, accounts: Option<Vec<CodexAccount>>, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(enabled) = enabled { self.voice.enabled = enabled; }
        self.voice.codex = codex;
        if self.voice.call.is_none() {
            (self.voice.voice, self.voice.account) = (saved.voice, saved.account);
            self.voice.organizer = saved.organizer;
            // Lista que não veio fica como estava: leitura falha não é "sem contas".
            if let Some(accounts) = accounts { self.voice.accounts = accounts; }
            // O seletor aberto segue a lista que `chosen_home` usa.
            if self.voice.accounts.len() < 2 { self.voice.account_select = None; }
            else if let Some((picker, _)) = &self.voice.account_select {
                let (items, at) = (self.voice.accounts.clone(), self.account_index());
                picker.update(cx, |select, cx| { select.set_items(items, window, cx); select.set_selected_index(Some(gpui_kit::component::IndexPath::new(at)), window, cx); });
            }
        }
        // Sem a opção ou sem o Codex a pílula some; a chamada não pode seguir com o microfone aberto.
        if (enabled == Some(false) || self.voice.codex.is_none()) && self.voice.call.is_some() {
            self.voice.error = Some(tr(if self.voice.codex.is_none() { "voice_no_codex" } else { "voice_gate_off" }));
            self.stop_voice(cx);
        }
        cx.notify();
    }

    /// Posição da conta escolhida em `accounts`; 0 (padrão) quando não há escolha ou ela sumiu.
    fn account_index(&self) -> usize {
        chosen_home(&self.voice.accounts, self.voice.account.as_deref())
            .and_then(|h| self.voice.accounts.iter().position(|a| std::path::Path::new(&a.home) == h)).unwrap_or(0)
    }

    pub(super) fn voice_context(&self) -> String {
        let name = self.selected.as_ref().map_or("", |s| s.name.as_str());
        session_context(name, &conversation_pairs(&self.chat.events))
    }

    pub(super) fn start_voice(&mut self, cx: &mut Context<Self>) {
        let Some(codex) = self.voice.codex.clone() else { self.voice.error = Some(tr("voice_no_codex")); cx.notify(); return; };
        if self.dictation.recording() { self.voice.error = Some(tr("voice_dictation_busy")); cx.notify(); return; }
        let (events_tx, events) = async_channel::unbounded();
        let target = self.selected.as_ref().map(|s| s.name.clone()).unwrap_or_default();
        let codex_home = chosen_home(&self.voice.accounts, self.voice.account.as_deref());
        // Conta salva que não resolve (lista não veio ou conta sumiu): nunca cai calado na padrão.
        if codex_home.is_none() && self.voice.account.as_deref().is_some_and(|a| !a.is_empty()) {
            crate::voice::log("voice: saved account unavailable, call not started");
            self.voice.error = Some(tr("voice_account_missing"));
            cx.notify();
            return;
        }
        crate::voice::log(format!("voice: account chosen {}", if codex_home.is_some() { "custom" } else { "default" }));
        // Voz salva que o v3 não aceita (escolhida antes do filtro) cai no padrão em vez de derrubar a chamada.
        let voice = self.voice.voice.clone().filter(|v| VOICES.contains(&v.as_str()));
        let options = VoiceOptions { codex, voice, context: self.voice_context(), cwd: self.local_session_dir(), target, codex_home,
            organizer: self.voice.organizer.clone() };
        self.voice.generation += 1;
        self.voice.call = Some(Voice::start(self.runtime.handle(), options, events_tx));
        self.voice.session_names.clear();
        self.voice_push_names();
        self.voice.target = self.selected.as_ref().map(|s| s.name.clone());
        self.voice.target_key = self.selected_key();
        self.voice.spoken.clear();
        self.voice.reply_pending = None;
        self.voice.pending_sends.clear();
        self.voice.pending_plan = None;
        self.voice.pending_question = None;
        (self.voice.close_gate, self.voice.close_reply) = (ConfirmGate::default(), None);
        self.voice.activity = CallActivity::Idle;
        (self.voice.thought, self.voice.action) = (String::new(), None);
        self.voice.shown = None;
        self.voice.watched.clear();
        (self.voice.effective, self.voice.backstage) = (None, VecDeque::new());
        // Dois JSON pequenos: lidos aqui, a chave nunca vai à tela nem ao diário.
        self.voice.jev = std::env::home_dir().and_then(|home| crate::voice::jev::load(&home));
        crate::voice::log(format!("jev configured={}", self.voice.jev.is_some()));
        self.voice.error = None;
        self.voice.mode = Mode::Direct;
        (self.voice.plan, self.voice.plan_open) = (None, None);
        (self.voice.context, self.voice.five_hour, self.voice.seven_day) = (None, None, None);
        self.voice.muted = false;
        self.voice.draft = None;
        self.voice.levels = (0., 0.);
        (self.voice.live_since, self.voice.ticker) = (None, None);
        self.voice.phase = Some(Phase::Connecting);
        let (generation, connection, tx) = (self.voice.generation, self.connection, self.tx.clone());
        self.runtime.spawn(async move {
            while let Ok(event) = events.recv().await {
                if tx.send(Envelope { connection, selection: None, payload: Payload::Voice(generation, event) }).await.is_err() { break; }
            }
        });
        cx.notify();
    }

    /// A árvore só existe com leitor de tela; na chamada ela é mantida para o `read_screen`. Fora do desenho, porque
    /// ligar pede um quadro novo.
    pub(super) fn sync_a11y_retain(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let want = self.voice.call.is_some() || self.voice.a11y_dump;
        if want == self.voice.a11y_retained { return; }
        self.voice.a11y_retained = want;
        window.defer(cx, move |window, _| window.retain_a11y_tree(want));
    }

    /// HANGAR_A11Y_DUMP=<arquivo>: grava a árvore da janela inteira nele, no máximo a cada 2 s e só quando muda (prova o
    /// `read_screen` sem chamada de voz). Sem a variável, nada roda.
    pub(super) fn watch_a11y_dump(window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(path) = std::env::var_os("HANGAR_A11Y_DUMP").filter(|p| !p.is_empty()) else { return false };
        cx.spawn_in(window, async move |_, cx| {
            let mut last = String::new();
            loop {
                cx.background_executor().timer(Duration::from_secs(2)).await;
                let Ok(snapshot) = cx.update(|window, _| window.a11y_snapshot(None)) else { break };
                let Some(snapshot) = snapshot.filter(|s| *s != last) else { continue };
                if let Err(error) = std::fs::write(&path, &snapshot) { crate::voice::log(format!("a11y dump failed: {error}")); }
                last = snapshot;
            }
        }).detach();
        true
    }

    pub(super) fn stop_voice(&mut self, cx: &mut Context<Self>) {
        if let Some(mut call) = self.voice.call.take() { call.stop(); }
        self.stop_computer();
        self.voice.generation += 1; // eventos atrasados da chamada parada não mexem na próxima
        self.voice.phase = None;
        self.voice.mode = Mode::Direct;
        self.voice.draft = None;
        self.voice.levels = (0., 0.);
        (self.voice.live_since, self.voice.ticker) = (None, None);
        self.voice.pending_sends.clear();
        self.voice.pending_plan = None;
        self.voice.pending_question = None;
        self.voice.activity = CallActivity::Idle;
        (self.voice.thought, self.voice.action) = (String::new(), None);
        self.voice.shown = None;
        cx.notify();
    }

    /// Um relógio só por chamada; acorda na virada do segundo do cronômetro para não pular número.
    fn start_call_clock(&mut self, cx: &mut Context<Self>) {
        let Some(since) = self.voice.live_since else { return };
        if self.voice.ticker.is_some() { return; }
        self.voice.ticker = Some(cx.spawn(async move |this, cx| loop {
            let into = since.elapsed().subsec_millis() as u64;
            cx.background_executor().timer(Duration::from_millis(1000 - into)).await;
            if this.update(cx, |this, cx| { this.voice_ask_expiry(); cx.notify() }).is_err() { break; }
        }));
    }

    /// O selo só muda quando o organizador confirma com `VoiceEvent::Mode`.
    fn set_voice_mode(&mut self, mode: Mode, cx: &mut Context<Self>) {
        if self.voice.mode == mode { return; }
        if let Some(call) = &self.voice.call { call.set_mode(mode); }
        cx.notify();
    }

    fn voice_reply(&self, call: CallId, reply: Value) {
        if let Some(voice) = &self.voice.call { voice.reply(call, reply); }
    }

    /// As sessões que a busca enxerga: máquina ativa e remotas, sem as escondidas.
    fn voice_candidates(&self) -> Vec<(String, SessionInfo)> {
        let active = self.active_key();
        self.sessions.iter().filter(|s| !self.sidebar.is_hidden(&active, &s.name))
            .map(|s| (active.clone(), s.clone()))
            .chain(self.remote.iter().flat_map(|(key, l)| l.sessions.iter().filter(|s| !self.sidebar.is_hidden(key, &s.name)).map(move |s| (key.clone(), s.clone()))))
            .collect()
    }

    /// Nome falado (com ou sem a máquina) → as candidatas e o que casou. Várias: desempata pelo contexto e pela raiz da
    /// família (`narrow_by_context`); `for_switch` tira a sessão da tela do contexto, porque trocar é ir para outra.
    fn voice_match(&self, spoken: &str, for_switch: bool, cx: &App) -> (Vec<(String, SessionInfo)>, SessionMatch) {
        let candidates = self.voice_candidates();
        let names: Vec<&str> = candidates.iter().map(|(_, s)| s.name.as_str()).collect();
        let machines: Vec<Vec<String>> = candidates.iter().map(|(key, _)| machine_aliases(key, &self.machine_label(key, cx))).collect();
        let on_active: Vec<bool> = candidates.iter().map(|(key, _)| self.is_active_key(key)).collect();
        let found = match match_qualified(spoken, &names, &machines, &on_active) {
            SessionMatch::Many(found) => {
                let now = std::time::Instant::now();
                let open = self.selected.as_ref().map(|s| (self.open_server(), s.name.clone()));
                let all: Vec<String> = candidates.iter().map(|(_, s)| s.name.clone()).collect();
                let context: Vec<bool> = found.iter().map(|&i| {
                    let (key, session) = &candidates[i];
                    let entry = (key.clone(), session.name.clone());
                    if open.as_ref() == Some(&entry) { return !for_switch; }
                    self.voice.followed.contains(&entry)
                        || self.voice.talked.get(&entry).is_some_and(|at| now.saturating_duration_since(*at) < TALKED_WINDOW)
                        || mentions_session(&self.voice.heard_recent, &session.name, &all)
                }).collect();
                let subset: Vec<&str> = found.iter().map(|&i| names[i]).collect();
                narrow_by_context(found, &subset, &context)
            }
            other => other,
        };
        (candidates, found)
    }

    /// Cada candidata ambígua com a máquina dela, no formato que o organizador pode devolver ("hangar em PC").
    fn voice_ambiguous(&self, candidates: &[(String, SessionInfo)], found: &[usize], cx: &App) -> String {
        let list: Vec<String> = found.iter().take(5).map(|&i| {
            let (key, session) = &candidates[i];
            format!("{} em {}", session.name, self.machine_label(key, cx))
        }).collect();
        format!("Mais de uma sessão combina: {}. Pergunte ao usuário qual e chame de novo com o nome e a máquina (ex.: '{}').",
            list.join(", "), list.first().cloned().unwrap_or_default())
    }

    /// Nome falado → (máquina, sessão). Ambíguo ou ausente volta como texto para o organizador; nunca um palpite.
    fn voice_resolve(&self, tool: &str, spoken: &str, cx: &App) -> Result<(String, SessionInfo), String> {
        let (mut candidates, found) = self.voice_match(spoken, false, cx);
        match found {
            SessionMatch::One(i) => { crate::voice::log(format!("{tool} one")); Ok(candidates.swap_remove(i)) }
            SessionMatch::Many(found) => { crate::voice::log(format!("{tool} many({})", found.len())); Err(self.voice_ambiguous(&candidates, &found, cx)) }
            SessionMatch::None => { crate::voice::log(format!("{tool} none")); Err("Não achei sessão com esse nome (e máquina, se foi dita).".into()) }
        }
    }

    /// `switch_session`: as sessões que a busca enxerga, pelo nome falado.
    /// `said`: a fala do turno; sem pedido de troca para essa sessão, nada muda (o pedido segue para a sessão ativa).
    fn voice_switch(&mut self, call: CallId, spoken: &str, said: &str, recent: &str, window: &mut Window, cx: &mut Context<Self>) {
        let now = std::time::Instant::now();
        let (mut candidates, found) = self.voice_match(spoken, true, cx);
        let resolved = match found {
            SessionMatch::One(i) => { crate::voice::log("switch_session one"); Ok(candidates.swap_remove(i)) }
            SessionMatch::Many(found) => {
                crate::voice::log(format!("switch_session many({})", found.len()));
                // Pedido de troca de verdade, só que ambíguo: a resposta à pergunta "qual?" vale sem repetir o verbo.
                if found.iter().any(|&i| switch_requested(said, recent, &candidates[i].1.name)) {
                    self.voice.switch_offer.offer(found.iter().map(|&i| (candidates[i].0.clone(), candidates[i].1.name.clone())).collect(), now);
                }
                Err(self.voice_ambiguous(&candidates, &found, cx))
            }
            SessionMatch::None => { crate::voice::log("switch_session none"); Err("Não achei sessão com esse nome (e máquina, se foi dita).".into()) }
        };
        let reply = match resolved {
            Ok((key, session)) => {
                if self.selected.as_ref().is_some_and(|s| s.name == session.name) && self.open_server() == key {
                    let by_jev = self.voice.jev_switched.as_ref()
                        .is_some_and(|(k, n, at)| *k == key && *n == session.name && now.saturating_duration_since(*at) < JEV_SWITCH_ECHO);
                    crate::voice::log(format!("switch_session result=already jev={by_jev}"));
                    if by_jev { tool_reply("O Hangar já trocou para essa sessão a pedido do usuário e já avisou por voz. Termine sem escrever nada.", true) }
                    else { tool_reply("Já estou nessa sessão.", true) }
                } else if !switch_requested(said, recent, &session.name) && !self.voice.switch_offer.take(&key, &session.name, now) {
                    crate::voice::log("switch_session refused not asked");
                    tool_reply(format!("{SWITCH_REFUSED}."), false)
                } else {
                    let (name, remote) = (session.name.clone(), !self.is_active_key(&key));
                    if self.select_on(&key, session, window, cx) {
                        let offer = offer_summary(&mut self.voice.talked, (key.clone(), name.clone()), now);
                        crate::voice::log(format!("switch_session result=switched remote={remote} offer={offer}"));
                        tool_reply(if offer {
                            format!("Sessão {name} aberta; a troca já foi anunciada, não repita. Pergunte numa frase curta se ele quer um resumo \
                                do que a sessão fez por último; só se ele disser que sim, leia com read_session e resuma em até três frases.")
                        } else {
                            format!("Sessão {name} aberta; a troca já foi anunciada, não repita. Você falou dela há pouco: não ofereça resumo.")
                        }, true)
                    } else {
                        crate::voice::log(format!("switch_session result=offline remote={remote}"));
                        tool_reply("A máquina dessa sessão não está conectada.", false)
                    }
                }
            }
            Err(text) => tool_reply(text, false),
        };
        self.voice_reply(call, reply);
    }

    /// Falha de ação (servidor, conexão): volta ao organizador e aparece na pílula. Nome ambíguo não passa por aqui.
    fn voice_fail(&mut self, call: CallId, text: String) {
        self.voice.error = Some(tr("voice_action_failed").replace("{erro}", &text));
        self.voice_reply(call, tool_reply(text, false));
    }

    fn voice_list(&mut self, call: CallId, cx: &mut Context<Self>) {
        let open = self.selected.as_ref().map(|s| (self.open_server(), s.name.clone()));
        let multi = self.multi_server();
        let rows: Vec<Listed> = self.voice_candidates().into_iter().map(|(key, s)| Listed {
            on_screen: open.as_ref().is_some_and(|(k, n)| *k == key && *n == s.name),
            followed: self.voice.followed.contains(&(key.clone(), s.name.clone())),
            machine: multi.then(|| self.machine_label(&key, cx)),
            provider: if s.provider.is_empty() { "claude".into() } else { s.provider },
            folder: s.cwd.as_deref().map(crate::composer::basename).unwrap_or_default().to_owned(),
            name: s.name, state: s.state,
        }).collect();
        let unreachable: Vec<String> = self.remote.iter().filter(|(_, l)| l.error.is_some()).map(|(key, _)| self.machine_label(key, cx)).collect();
        crate::voice::log(format!("list_sessions count={} unreachable={}", rows.len(), unreachable.len()));
        self.voice_reply(call, tool_reply(sessions_text(&rows, &unreachable), true));
    }

    fn voice_open(&mut self, call: CallId, request: OpenRequest, cx: &mut Context<Self>) {
        let key = match request.server.as_deref() {
            None => self.active_key(),
            Some(spoken) => {
                let keys: Vec<String> = std::iter::once(self.active_key()).chain(self.remote.keys().cloned()).filter(|k| !k.is_empty()).collect();
                let labels: Vec<String> = keys.iter().map(|k| self.machine_label(k, cx)).collect();
                let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
                match match_session(spoken, &refs, &[]) {
                    SessionMatch::One(i) => keys[i].clone(),
                    SessionMatch::Many(found) => {
                        let list: Vec<&str> = found.iter().map(|&i| refs[i]).collect();
                        self.voice_reply(call, tool_reply(format!("Mais de uma máquina combina: {}. Peça para o usuário dizer qual.", list.join(", ")), false));
                        return;
                    }
                    SessionMatch::None => {
                        self.voice_reply(call, tool_reply(format!("Não achei máquina com esse nome. Máquinas: {}.", refs.join(", ")), false));
                        return;
                    }
                }
            }
        };
        let Some(api) = self.machine_api(&key) else { let reason = self.machine_error(&key); self.voice_fail(call, reason); return };
        crate::voice::log(format!("open_session start provider={} request={}", request.provider, request.request.is_some()));
        let (generation, connection, tx) = (self.voice.generation, self.connection, self.tx.clone());
        self.runtime.spawn(async move {
            let work = request.request.clone();
            let result = create_by_voice(&api, request).await;
            // Como a primeira mensagem da tela de criação: o pedido sai logo que a sessão nasce, sem a espera do microfone
            // (a fala já acabou), e a resposta ao organizador espera a entrega.
            let (result, sent) = match (result, work) {
                (Ok(session), Some(work)) => { let delivery = api.send(&session.name, &work).await; (Ok(session), Some((work, delivery))) }
                (Err(text), Some(_)) => (Err(format!("{text} O pedido não foi enviado.")), None),
                (result, None) => (result, None),
            };
            let _ = tx.send(Envelope { connection, selection: None, payload: Payload::VoiceDone(generation, call, VoiceDone::Opened(key, result, sent)) }).await;
        });
    }

    /// Fechar em duas chamadas (`ConfirmGate`): a primeira só arma e pede a confirmação; a segunda usa o Fechar da barra.
    fn voice_close(&mut self, call: CallId, spoken: &str, confirmed: bool, turn: &str, cx: &mut Context<Self>) {
        let (key, session) = match self.voice_resolve("close_session", spoken, cx) {
            Ok(found) => found,
            Err(text) => { self.voice_reply(call, tool_reply(text, false)); return; }
        };
        let target = Target::new(&key, &session.name);
        // Orquestrador, par de fora e convite não têm "Fechar" no menu da barra.
        if session.orq() || session.read_only() || self.invite_target(&target) {
            self.voice_reply(call, tool_reply("Essa sessão não pode ser fechada por aqui.", false));
            return;
        }
        if self.voice.close_reply.is_some() {
            self.voice_reply(call, tool_reply("Já há um fechamento em andamento; espere o resultado.", false));
            return;
        }
        if !self.voice.close_gate.check(target.clone(), confirmed, turn, std::time::Instant::now()) {
            crate::voice::log("close_session armed");
            let place = self.machine_label(&key, cx);
            self.voice_reply(call, tool_reply(format!("Nada foi fechado. Confirme com o usuário: fechar a sessão {} em {place}? \
                Só depois de um sim explícito, chame close_session de novo com confirmed true.", session.name), true));
            return;
        }
        if self.machine_api(&key).is_none() { let reason = self.machine_error(&key); self.voice_fail(call, reason); return; }
        crate::voice::log("close_session sent");
        self.voice.close_reply = Some((target.clone(), call));
        self.delete_target(target, cx);
    }

    /// Resposta do Fechar da barra: só a do pedido da voz volta ao organizador.
    pub(super) fn voice_closed(&mut self, target: &Target, result: &Result<Value, Failure>) {
        if self.voice.close_reply.as_ref().is_none_or(|(t, _)| t != target) { return; }
        let Some((_, call)) = self.voice.close_reply.take() else { return };
        match result {
            Ok(_) => { crate::voice::log("close_session closed"); self.voice_reply(call, tool_reply(format!("Sessão {} fechada.", target.name), true)); }
            Err(error) => {
                crate::voice::log(format!("close_session failed status={:?}", error.status));
                self.voice_fail(call, Self::fetch_failure(error));
            }
        }
    }

    /// Acompanhar ou parar: a lista de sessões passa a vigiar a sessão a cada turno, não só depois de um pedido da voz.
    fn voice_follow(&mut self, call: CallId, spoken: &str, on: bool, cx: &mut Context<Self>) {
        let (key, session) = match self.voice_resolve(if on { "follow_session" } else { "unfollow_session" }, spoken, cx) {
            Ok(found) => found,
            Err(text) => { self.voice_reply(call, tool_reply(text, false)); return; }
        };
        let entry = (key, session.name.clone());
        let text = if on {
            self.voice.followed.insert(entry);
            self.voice_sessions();
            format!("Acompanhando a sessão {}: cada resposta dela chega para você falar.", session.name)
        } else if self.voice.followed.remove(&entry) {
            format!("Parei de acompanhar a sessão {}.", session.name)
        } else {
            format!("A sessão {} não estava sendo acompanhada.", session.name)
        };
        crate::voice::log(format!("follow on={on} count={}", self.voice.followed.len()));
        self.voice_reply(call, tool_reply(text, true));
    }

    /// Mesma chamada do arrastar/diálogo de agrupar; só na mesma máquina, com as recusas da barra.
    fn voice_pair(&mut self, call: CallId, a: &str, b: &str, cx: &mut Context<Self>) {
        let (first, second) = match (self.voice_resolve("pair_sessions", a, cx), self.voice_resolve("pair_sessions", b, cx)) {
            (Ok(first), Ok(second)) => (first, second),
            (Err(text), _) | (_, Err(text)) => { self.voice_reply(call, tool_reply(text, false)); return; }
        };
        if let Err(refusal) = can_pair(&first.1, &second.1, first.0 == second.0) { self.voice_reply(call, tool_reply(refusal.text(), false)); return; }
        let Some(api) = self.machine_api(&first.0) else { let reason = self.machine_error(&first.0); self.voice_fail(call, reason); return };
        crate::voice::log("pair_sessions sent");
        let (origin, target) = (first.1.name, second.1.name);
        // Tarefa vazia: entrando num grupo que existe vale a dele; dois soltos nascem sem, como o campo vazio do diálogo.
        self.voice_group(call, "grupo_drop_falhou", async move { api.pair(&target, &[origin], "", false).await });
    }

    fn voice_unpair(&mut self, call: CallId, spoken: &str, cx: &mut Context<Self>) {
        let (key, session) = match self.voice_resolve("unpair_session", spoken, cx) {
            Ok(found) => found,
            Err(text) => { self.voice_reply(call, tool_reply(text, false)); return; }
        };
        if !can_leave(&session) { self.voice_reply(call, tool_reply(format!("A sessão {} não está em grupo.", session.name), false)); return; }
        let Some(api) = self.machine_api(&key) else { let reason = self.machine_error(&key); self.voice_fail(call, reason); return };
        crate::voice::log("unpair_session sent");
        self.voice_group(call, "grupo_drop_sair_falhou", async move { api.unpair(&session.name).await });
    }

    /// Uma ação de `HANGAR_ACTIONS` pelo mesmo método do botão. Nenhuma troca a sessão ativa; `Err` é o motivo real.
    pub(super) fn run_hangar_action(&mut self, id: &str, arg: Option<&str>, window: &mut Window, cx: &mut Context<Self>) -> Result<String, String> {
        use crate::appearance::SideTab;
        if self.connection_dialog { return Err("A janela de conexão está aberta; feche-a antes.".into()); }
        let side_tab = |tab| match tab { SideTab::Files => "Arquivos", SideTab::Activity => "Atividade", SideTab::Git => "Git", _ => "Contexto" };
        match id {
            "topbar-settings" => {
                let page = match arg {
                    None => settings::Page::Appearance,
                    Some(spoken) => {
                        let sections: Vec<settings::Page> = settings::Page::sections().collect();
                        let keys: Vec<&str> = sections.iter().map(|p| p.key()).collect();
                        match match_session(spoken, &keys, &[]) {
                            SessionMatch::One(i) => sections[i],
                            _ => return Err(format!("Seção desconhecida: {spoken}. Seções: {}.", keys.join(", "))),
                        }
                    }
                };
                self.open_settings(page, window, cx);
                Ok(format!("Configurações abertas na seção {}.", page.key()))
            }
            "settings-back" if self.settings.is_none() => Ok("As configurações já estavam fechadas.".into()),
            "settings-back" => { self.close_settings(window, cx); Ok("Configurações fechadas.".into()) }
            "sidebar-new-session" if self.create_blocked(window, cx) => Err("Não dá para abrir o diálogo agora: sem servidor conectado ou outro diálogo aberto.".into()),
            "sidebar-new-session" => { self.open_new_session(None, window, cx); Ok("Diálogo de nova sessão aberto; a pessoa escolhe e confirma.".into()) }
            "sidebar-fold" if appearance::get().navigation.tabs() => Err("Com a navegação em abas não há barra de sessões para recolher.".into()),
            "sidebar-fold" => { self.toggle_rail(cx); Ok(if self.rail() { "Barra de sessões recolhida." } else { "Barra de sessões aberta." }.into()) }
            "side-toggle" | "side-tab-context" | "side-tab-files" | "side-tab-activity" | "side-tab-git" if self.selected.is_none() =>
                Err("Nenhuma sessão aberta; o painel lateral é da sessão.".into()),
            "side-toggle" => { self.toggle_side(cx); Ok(if self.side.open { "Painel lateral aberto." } else { "Painel lateral escondido." }.into()) }
            "side-tab-context" | "side-tab-files" | "side-tab-activity" | "side-tab-git" => {
                let tab = match id { "side-tab-files" => SideTab::Files, "side-tab-activity" => SideTab::Activity, "side-tab-git" => SideTab::Git, _ => SideTab::Context };
                if !self.side.open { self.toggle_side(cx); }
                self.choose_side_tab(tab, window, cx);
                // A aba que a sessão não tem cai em outra: dizer, em vez de fingir.
                if self.side_tab() == tab { Ok(format!("Painel lateral na aba {}.", side_tab(tab))) }
                else { Err(format!("A aba {} não está disponível nesta sessão; o painel ficou em {}.", side_tab(tab), side_tab(self.side_tab()))) }
            }
            "terminal-show" if self.terminal.is_some() => Ok("O terminal já está aberto.".into()),
            "terminal-show" => {
                if !self.selected.as_ref().is_some_and(|s| super::terminal::terminal_offered(s, self.has_shortcut_terms())) {
                    return Err("Esta sessão não tem terminal (nenhuma aberta, orquestrador ou só leitura).".into());
                }
                self.toggle_terminal(window, cx);
                if self.terminal.is_some() { Ok("Terminal aberto.".into()) } else { Err("O terminal não abriu.".into()) }
            }
            "terminal-close" if self.terminal.is_none() => Ok("O terminal já estava fechado.".into()),
            "terminal-close" => { self.close_terminal(true, window, cx); Ok("Terminal fechado.".into()) }
            "topbar-voice" if self.voice.open => Ok("O cartão da voz já está aberto.".into()),
            "topbar-voice" => { self.toggle_voice_panel(window, cx); Ok("Cartão da voz aberto.".into()) }
            "voice-panel-close" => { self.voice.open = false; cx.notify(); Ok("Cartão da voz fechado; a conversa continua.".into()) }
            "topbar-cost" | "costs-usage" if self.api.is_none() => Err("Sem servidor conectado.".into()),
            "topbar-cost" => { self.open_costs(window, cx); Ok("Página de custos aberta.".into()) }
            "costs-usage" => {
                if self.costs.view.is_none() { self.open_costs(window, cx); }
                self.show_costs_view(super::costs::View::Usage, window, cx);
                Ok("Estatísticas de uso abertas.".into())
            }
            "costs-back" if self.costs.view.is_none() => Ok("A página de custos já estava fechada.".into()),
            "costs-back" => { self.close_costs(window, cx); Ok("Página de custos fechada.".into()) }
            other => Err(format!("Ação desconhecida: {other}. Veja os ids com hangar_actions.")),
        }
    }

    /// O retrato é do último quadro: a área aberta por uma ação no mesmo instante (o cartão da voz, um diálogo) só entra
    /// no quadro seguinte. Área ausente tenta de novo uma vez depois de alguns quadros; a falha volta só ao organizador.
    fn voice_read_screen(&mut self, call: CallId, area: Option<String>, retry: bool, window: &mut Window, cx: &mut Context<Self>) {
        match read_screen(window, area.as_deref()) {
            Ok((root, text)) => {
                crate::voice::log(format!("read_screen area={} bytes={}", root.as_deref().unwrap_or("window"), text.len()));
                self.voice_reply(call, tool_reply(text, true));
            }
            Err(_) if retry => {
                cx.spawn_in(window, async move |this, cx| {
                    cx.background_executor().timer(SCREEN_RETRY).await;
                    let _ = this.update_in(cx, |this, window, cx| this.voice_read_screen(call, area, false, window, cx));
                }).detach();
            }
            Err(text) => {
                crate::voice::log(format!("read_screen failed area={}", clip(area.as_deref().unwrap_or("-"), 64)));
                self.voice_reply(call, tool_reply(text, false));
            }
        }
    }

    /// Clique pela árvore de acessibilidade, pelo mesmo caminho de um leitor de tela: só o elemento que está de fato sob o
    /// ponto recebe. Botão arriscado arma e pede o sim; o clique sai na chamada seguinte, noutro turno falado.
    fn voice_click(&mut self, call: CallId, id: &str, confirmed: bool, turn: &str, window: &mut Window, cx: &mut Context<Self>) {
        let reason = |code: &str| match code {
            "not-found" | "no-tree" => format!("O id {id} não está na tela agora; leia a tela de novo com read_screen."),
            "ambiguous" => format!("Mais de um elemento tem o id {id}; leia uma área menor e use outro id."),
            "disabled" => format!("O elemento {id} está desabilitado."),
            "covered" => format!("O elemento {id} está coberto por outro (diálogo, menu) ou fora da área visível."),
            other => format!("Não cliquei em {id}: {other}."),
        };
        let label = match window.a11y_target(id) {
            Ok((_, label)) => label,
            Err(code) => { self.voice_reply(call, tool_reply(reason(code), false)); return; }
        };
        let name = label.clone().unwrap_or_else(|| id.to_owned());
        if risky_click(id, label.as_deref()) && !self.voice.click_gate.check(id.to_owned(), confirmed, turn, std::time::Instant::now()) {
            crate::voice::log("click_screen armed");
            self.voice_reply(call, tool_reply(format!("Nada foi clicado. «{name}» muda algo difícil de desfazer: confirme com o usuário e, só depois \
                do sim explícito, chame click_screen de novo com confirmed true."), true));
            return;
        }
        let result = window.a11y_click(id, cx);
        crate::voice::log(format!("click_screen ok={}", result.is_ok()));
        match result {
            Ok(()) => self.voice_reply(call, tool_reply(format!("Cliquei em «{name}». Para ver o resultado, leia a tela de novo."), true)),
            Err(code) => self.voice_reply(call, tool_reply(reason(code), false)),
        }
        cx.notify();
    }

    /// `observe_system`: duas leituras com a amostra no meio, numa thread à parte para não prender a tela nem a chamada.
    fn voice_observe(&mut self, call: CallId, request: crate::voice::observe::Request) {
        let (generation, connection, tx) = (self.voice.generation, self.connection, self.tx.clone());
        self.runtime.spawn(async move {
            let result = tokio::task::spawn_blocking(move || crate::voice::observe::observe(&request)).await
                .map_err(|_| "A leitura dos processos falhou.".to_owned());
            let _ = tx.send(Envelope { connection, selection: None, payload: Payload::VoiceDone(generation, call, VoiceDone::Observed(result)) }).await;
        });
    }

    /// Pergunta ao Jev o que a fala pede, com as sessões e as ações de tela de agora como opções. Roda em paralelo ao
    /// organizador; sem chave, nada acontece.
    fn voice_jev_ask(&mut self, turn: String, speech: String, recent: String, cx: &mut Context<Self>) {
        let Some(config) = self.voice.jev.clone() else { return };
        let open = self.selected.as_ref().map(|s| (self.open_server(), s.name.clone()));
        let candidates = self.voice_candidates();
        // Todas as máquinas: a pessoa diz só o nome, e a sessão pode estar em outro servidor.
        let labels: Vec<String> = candidates.iter().map(|(key, s)| {
            let here = open.as_ref().is_some_and(|(k, n)| k == key && *n == s.name);
            crate::voice::jev::session_label(&s.name, &self.machine_label(key, cx), here)
        }).collect();
        let sessions: Vec<(String, String)> = candidates.into_iter().map(|(key, s)| (key, s.name)).collect();
        let actions: Vec<&'static str> = HANGAR_ACTIONS.iter().map(|a| a.id).collect();
        let action_labels: Vec<String> = HANGAR_ACTIONS.iter().map(|a| format!("{}: {}", a.label, a.description)).collect();
        let screen = open.as_ref().map(|(_, name)| name.as_str());
        let (state, questions) = crate::voice::jev::questions(&speech, &recent, screen, &labels, &action_labels);
        // Só contagens: o texto da fala nunca vai ao diário.
        crate::voice::log(format!("jev asked words={} recent_lines={} sessions={} machines={} actions={}", speech.split_whitespace().count(),
            recent.lines().count(), sessions.len(), sessions.iter().map(|(k, _)| k.as_str()).collect::<HashSet<_>>().len(), actions.len()));
        let asked = JevAsked { turn, speech, recent, sessions, actions, started: std::time::Instant::now() };
        let (generation, connection, tx) = (self.voice.generation, self.connection, self.tx.clone());
        self.runtime.spawn(async move {
            let result = crate::voice::jev::ask(&config, &state, &questions).await.map(|picks| crate::voice::jev::decision(&picks));
            let _ = tx.send(Envelope { connection, selection: None, payload: Payload::VoiceJev(generation, asked, result) }).await;
        });
    }

    /// Só certeza alta age. Troca e ação de tela saem daqui na hora e o organizador recebe uma nota no mesmo turno; a
    /// troca ainda passa pela trava `switch_requested`, como a do organizador. O veredito de envio vai à trava da chamada.
    pub(super) fn voice_jev_result(&mut self, generation: u64, asked: JevAsked, result: Result<crate::voice::jev::Decision, String>,
        window: &mut Window, cx: &mut Context<Self>) {
        use crate::voice::{SendVerdict, jev::{Intent, SURE}};
        if generation != self.voice.generation { return; }
        let ms = asked.started.elapsed().as_millis();
        let decision = match result {
            Ok(decision) => decision,
            Err(error) => { crate::voice::log(format!("jev failed ms={ms} error={error}")); return; }
        };
        let intent = decision.intent.clone();
        crate::voice::log(format!("jev intent={:?} p={:.2} session_p={:.2} action_p={:.2} ms={ms}", intent.as_ref().map(|(i, _)| i),
            intent.as_ref().map_or(0., |(_, p)| *p), decision.session.map_or(0., |(_, p)| p), decision.action.map_or(0., |(_, p)| p)));
        let sure = |what: Intent| intent.as_ref().is_some_and(|(i, p)| *i == what && *p >= SURE);
        // Quando a tela agiu, a voz confirma já e o organizador termina calado: senão a confirmação esperaria o turno dele.
        let mut acted: Option<(String, String)> = None;
        // 'Volta pra lá' não tem nome: com a intenção de troca firme, o destino é o que a voz acabou de oferecer.
        let names: Vec<&str> = asked.sessions.iter().map(|(_, n)| n.as_str()).collect();
        let choice = match jev_switch_choice(&decision) {
            Err("no-session" | "low-session") if let Some(at) = crate::voice::organizer::confirmed_switch_target(&asked.recent, &names) => Ok(at),
            other => other,
        };
        match choice {
            Ok(at) => match asked.sessions.get(at).cloned() {
                None => crate::voice::log("jev skip reason=no-session"),
                Some((key, name)) => {
                    let open = self.selected.as_ref().is_some_and(|s| s.name == name) && self.open_server() == key;
                    let session = self.voice_candidates().into_iter().find(|(k, s)| *k == key && s.name == name).map(|(_, s)| s);
                    let machine = self.machine_label(&key, cx);
                    let reason = if open { Some("on-screen") } else if !switch_requested(&asked.speech, &asked.recent, &name) { Some("not-asked") }
                        else if session.is_none() { Some("gone") } else { None };
                    match (reason, session) {
                        (Some(reason), _) => crate::voice::log(format!("jev skip reason={reason}")),
                        (None, Some(session)) => {
                            let remote = !self.is_active_key(&key);
                            if self.select_on(&key, session, window, cx) {
                                self.voice.jev_switched = Some((key.clone(), name.clone(), std::time::Instant::now()));
                                let offer = offer_summary(&mut self.voice.talked, (key.clone(), name.clone()), std::time::Instant::now());
                                crate::voice::log(format!("jev act=switched remote={remote} offer={offer}"));
                                acted = Some(if offer {
                                    (format!("Pronto, abri a {name}. Quer um resumo do que ela fez por último?"),
                                        format!("O Hangar já trocou a tela para a sessão {name} (máquina {machine}) a pedido do usuário, e a voz já \
                                            perguntou se ele quer um resumo. Não chame switch_session e termine sem escrever nada; se ele disser \
                                            que sim, leia com read_session e resuma em até três frases."))
                                } else {
                                    (format!("Pronto, abri a {name}."),
                                        format!("O Hangar já trocou a tela para a sessão {name} (máquina {machine}) a pedido do usuário e já confirmou \
                                            por voz. Não chame switch_session e termine sem escrever nada."))
                                });
                            } else {
                                crate::voice::log(format!("jev skip reason=offline remote={remote}"));
                            }
                        }
                        (None, None) => crate::voice::log("jev skip reason=gone"),
                    }
                }
            },
            Err("not-switch") => match jev_action_choice(&decision) {
                Ok(at) => if let Some(id) = asked.actions.get(at).copied() {
                    let label = HANGAR_ACTIONS.iter().find(|a| a.id == id).map_or(id, |a| a.label);
                    let result = self.run_hangar_action(id, None, window, cx);
                    crate::voice::log(format!("jev act=screen ok={}", result.is_ok()));
                    acted = Some(match result {
                        Ok(text) => (format!("Pronto. {text}"), format!("O Hangar já fez na tela, a pedido do usuário: {label}. {text} Já confirmou \
                            por voz. Não repita a ação; se faltar um passo (uma seção, um botão), complete com as ferramentas de tela; senão \
                            termine sem escrever nada.")),
                        Err(text) => (String::new(), format!("O Hangar tentou na tela, a pedido do usuário, {label}, mas não deu: {text}")),
                    });
                },
                Err(reason) if reason != "not-screen" => crate::voice::log(format!("jev skip reason=screen-{reason}")),
                Err(_) => {}
            },
            Err(reason) => crate::voice::log(format!("jev skip reason={reason}")),
        }
        let verdict = if sure(Intent::Send) { SendVerdict::Send }
            else if sure(Intent::Talk) || sure(Intent::Hold) || sure(Intent::Noise) { SendVerdict::Block }
            else { SendVerdict::Unsure };
        let (speech, note) = acted.map_or((None, None), |(speech, note)| ((!speech.is_empty()).then_some(speech), Some(note)));
        if let Some(call) = &self.voice.call { call.jev_verdict(asked.turn, verdict, speech, note); }
        cx.notify();
    }

    /// `computer`: o HCC roda fora da tela e responde quando acaba; um objetivo por vez.
    fn voice_computer(&mut self, call: CallId, objective: String) {
        if self.voice.computer.as_ref().is_some_and(|task| !task.is_finished()) {
            self.voice_reply(call, tool_reply("Já há um objetivo no computador em andamento; espere o resultado.", false));
            return;
        }
        crate::voice::log("computer start");
        let (generation, connection, tx) = (self.voice.generation, self.connection, self.tx.clone());
        self.voice.computer = Some(self.runtime.spawn(async move {
            let result = match tokio::task::spawn_blocking(crate::voice::computer::launch).await {
                Ok(Ok(launch)) => crate::voice::computer::run_objective(launch, &objective).await,
                Ok(Err(text)) => Err(text),
                Err(_) => Err("Não consegui preparar o hangar-computer-control.".into()),
            };
            let _ = tx.send(Envelope { connection, selection: None, payload: Payload::VoiceDone(generation, call, VoiceDone::Computer(result)) }).await;
        }));
    }

    /// Parar a chamada para também o objetivo: o Python cai junto com a tarefa.
    fn stop_computer(&mut self) {
        if let Some(task) = self.voice.computer.take() && !task.is_finished() { crate::voice::log("computer aborted"); task.abort(); }
    }

    /// `fallback`: a frase do web quando o servidor não diz o motivo.
    fn voice_group(&self, call: CallId, fallback: &'static str, work: impl std::future::Future<Output = Result<PairResult, Failure>> + Send + 'static) {
        let (generation, connection, tx) = (self.voice.generation, self.connection, self.tx.clone());
        self.runtime.spawn(async move {
            let result = work.await;
            let _ = tx.send(Envelope { connection, selection: None, payload: Payload::VoiceDone(generation, call, VoiceDone::Grouped(fallback, result)) }).await;
        });
    }

    pub(super) fn voice_done(&mut self, generation: u64, call: CallId, done: VoiceDone, window: &mut Window, cx: &mut Context<Self>) {
        if generation != self.voice.generation { return; }
        match done {
            VoiceDone::Opened(key, Ok(session), sent) => {
                crate::voice::log("open_session created");
                let name = session.name.clone();
                // Como na tela de criação: a lista guardada da outra máquina já a inclui, para a leitura seguinte não fechá-la.
                if let Some(list) = self.remote.get_mut(&key).filter(|l| l.loaded && !l.sessions.iter().any(|s| s.name == name)) {
                    list.sessions.push(session.clone());
                }
                let shown = self.select_on(&key, session, window, cx);
                if let Some((work, result)) = &sent {
                    crate::voice::log(format!("open_session request {}", match result { Ok(d) if d.ok && d.delivered => "sent", Ok(d) if d.ok => "queued", _ => "failed" }));
                    // A entrega entra no rastreio da sessão nova como a primeira mensagem da tela de criação: bolha e falha no chat.
                    if shown && let Some(session_key) = self.selected_key().filter(|k| k.name == name) {
                        self.voice.watched.insert(session_key.clone(), false);
                        self.delivery.begin(session_key.clone(), work.clone(), HashSet::new());
                        self.receive_sent(session_key, work.clone(), String::new(), result.clone(), window, cx);
                    }
                }
                match opened_reply(&name, shown, sent.as_ref().map(|(_, result)| result)) {
                    Ok(text) => self.voice_reply(call, tool_reply(text, true)),
                    Err(text) => self.voice_fail(call, text),
                }
            }
            VoiceDone::Opened(_, Err(text), _) => { crate::voice::log("open_session failed"); self.voice_fail(call, text); }
            VoiceDone::Grouped(_, Ok(result)) => {
                crate::voice::log(format!("group done warning={}", result.warning.is_some()));
                // O vínculo mudou, mas alguém não foi avisado: o organizador precisa dizer isso.
                let text = result.warning.map_or_else(|| "Feito.".to_owned(), |w| format!("Feito, mas com aviso: {w}"));
                self.voice_reply(call, tool_reply(text, true));
            }
            VoiceDone::Computer(result) => {
                self.voice.computer = None;
                match result {
                    // "parou: …" é resultado do HCC, não falha do app: volta como está, sem sucesso.
                    Ok(text) => {
                        let done = !text.trim_start().starts_with("parou");
                        crate::voice::log(format!("computer done completed={done}"));
                        self.voice_reply(call, tool_reply(text, done));
                    }
                    Err(text) => { crate::voice::log("computer failed"); self.voice_fail(call, text); }
                }
            }
            VoiceDone::Observed(result) => {
                crate::voice::log(format!("observe done ok={}", result.is_ok()));
                match result { Ok(text) => self.voice_reply(call, tool_reply(text, true)), Err(text) => self.voice_reply(call, tool_reply(text, false)) }
            }
            VoiceDone::Grouped(fallback, Err(error)) => {
                crate::voice::log(format!("group failed status={:?}", error.status));
                self.voice_fail(call, super::grouping::failed(&error, fallback));
            }
        }
        cx.notify();
    }

    pub(super) fn receive_voice(&mut self, generation: u64, event: VoiceEvent, window: &mut Window, cx: &mut Context<Self>) {
        if generation != self.voice.generation { return; }
        match event {
            VoiceEvent::Phase(Phase::Closed) => {
                self.voice.call = None;
                self.stop_computer();
                self.voice.phase = None;
                self.voice.mode = Mode::Direct;
                self.voice.draft = None;
                self.voice.levels = (0., 0.);
                self.voice.shown = None;
                self.voice.activity = CallActivity::Idle;
                (self.voice.thought, self.voice.action) = (String::new(), None);
                (self.voice.live_since, self.voice.ticker) = (None, None);
                self.voice.pending_sends.clear();
                self.voice.pending_plan = None;
                self.voice.pending_question = None;
            }
            VoiceEvent::Phase(phase) => {
                if matches!(phase, Phase::Live) {
                    self.voice.error = None;
                    self.voice.live_since.get_or_insert_with(std::time::Instant::now);
                    self.start_call_clock(cx);
                }
                self.voice.phase = Some(phase);
            }
            VoiceEvent::Levels(input, output) => {
                // ~16 Hz: a janela só repinta quando a barra muda de passo ou quem fala muda.
                let muted = self.voice.muted;
                let bars = |(i, o): (f32, f32)| (bar_height(i), bar_height(o));
                let levels = (level_gain(input), level_gain(output));
                let now = std::time::Instant::now();
                let (old_shown, since) = self.voice.shown.unwrap_or((Speaker::Idle, now));
                let (shown, since) = settled_speaker(old_shown, since, speaker(levels.0, levels.1, muted), now);
                self.voice.shown = Some((shown, since));
                let step = self.voice.live_since.map_or(0, |since| since.elapsed().as_millis() / ANIM_STEP);
                let changed = bars(self.voice.levels) != bars(levels) || shown != old_shown || step != self.voice.anim_step;
                self.voice.anim_step = step;
                self.voice.levels = levels;
                if !changed { return; }
                self.voice.frame = self.voice.frame.wrapping_add(1);
            }
            VoiceEvent::Activity(activity) => self.voice.activity = activity,
            VoiceEvent::Thought(delta) => push_thought(&mut self.voice.thought, &delta),
            VoiceEvent::Action(action) => self.voice.action = action,
            VoiceEvent::TurnDone => (self.voice.thought, self.voice.action) = (String::new(), None),
            VoiceEvent::Draft(draft) => self.voice.draft = draft,
            VoiceEvent::Failed(failure) => {
                self.voice.error = Some(failure_text(&failure));
                // Erro do organizador ou da troca de modelo não para a conversa: aparece na pílula e no painel, sem abrir.
                if !matches!(failure, VoiceFailure::Organizer | VoiceFailure::ModelSwitch | VoiceFailure::OwnFolder) { self.voice.open = true; }
            }
            VoiceEvent::Mode(mode) => self.voice.mode = mode,
            VoiceEvent::Plan { path, markdown } => self.voice.plan = Some((path, markdown)),
            VoiceEvent::AskSession(question) => self.voice_ask(&question, cx),
            VoiceEvent::OrganizerContext { used, window } => self.voice.context = Some((used, window)),
            // Atualização com uma janela só não apaga a outra.
            VoiceEvent::AccountLimits { five_hour, seven_day } => {
                self.voice.five_hour = five_hour.or(self.voice.five_hour);
                self.voice.seven_day = seven_day.or(self.voice.seven_day);
            }
            VoiceEvent::SwitchSession { call, name, spoken, recent } => self.voice_switch(call, &name, &spoken, &recent, window, cx),
            VoiceEvent::HangarActions(call) => {
                let sections: Vec<&str> = settings::Page::sections().map(settings::Page::key).collect();
                self.voice_reply(call, tool_reply(hangar_actions_text(&sections), true));
            }
            // Recusa de ação da tela é recado para o organizador, que conta ao usuário pela voz; no cartão ficaria como erro.
            VoiceEvent::HangarAction { call, id, arg } => {
                let result = self.run_hangar_action(&id, arg.as_deref(), window, cx);
                crate::voice::log(format!("hangar_action ok={}", result.is_ok()));
                match result { Ok(text) => self.voice_reply(call, tool_reply(text, true)), Err(text) => self.voice_reply(call, tool_reply(text, false)) }
            }
            VoiceEvent::Computer(call, objective) => self.voice_computer(call, objective),
            VoiceEvent::ReadScreen(call, area) => self.voice_read_screen(call, area, true, window, cx),
            VoiceEvent::ClickScreen { call, id, confirmed, turn } => self.voice_click(call, &id, confirmed, &turn, window, cx),
            VoiceEvent::Observe(call, request) => self.voice_observe(call, request),
            VoiceEvent::Heard { turn, speech, recent } => {
                self.voice.heard_recent = format!("{speech}\n{recent}");
                self.voice_jev_ask(turn, speech, recent, cx);
            }
            VoiceEvent::ListSessions(call) => self.voice_list(call, cx),
            VoiceEvent::OpenSession(call, request) => self.voice_open(call, request, cx),
            VoiceEvent::CloseSession { call, name, confirmed, turn } => self.voice_close(call, &name, confirmed, &turn, cx),
            VoiceEvent::PairSessions(call, a, b) => self.voice_pair(call, &a, &b, cx),
            VoiceEvent::UnpairSession(call, name) => self.voice_unpair(call, &name, cx),
            VoiceEvent::FollowSession(call, name) => self.voice_follow(call, &name, true, cx),
            VoiceEvent::UnfollowSession(call, name) => self.voice_follow(call, &name, false, cx),
            VoiceEvent::Organizer(effective) => self.voice.effective = Some(effective),
            VoiceEvent::Backstage(line) => {
                if self.voice.backstage.len() >= BACKSTAGE_KEEP { self.voice.backstage.pop_front(); }
                self.voice.backstage.push_back(line);
            }
            VoiceEvent::SendPlan { session, text } => {
                // O plano foi escrito para uma sessão; se a tela mudou, não vai para outra.
                let on_screen = self.selected.as_ref().is_some_and(|s| s.name == session);
                let key = self.selected_key().filter(|_| on_screen);
                let Some(key) = key.filter(|key| self.api_for(&key.server).is_some()) else {
                    self.voice.error = Some(tr("voice_plan_off_screen"));
                    if let Some(voice) = &self.voice.call { voice.session_answer(format!("O plano não foi enviado: a sessão {session} não está na tela.")); }
                    cx.notify();
                    return;
                };
                let was_working = self.chat.state.state == "working";
                self.voice.watched.insert(key.clone(), was_working);
                let known = self.known_user_ids();
                if !self.post(key.clone(), text.clone(), String::new(), false, known, None, cx) { self.delivery.hold(key.clone(), text.clone(), false, None); }
                // O plano só deixa de ser editável quando `voice_sent` confirma a entrega.
                self.voice.pending_plan = Some((key, text));
            }
            VoiceEvent::ReadSession(call) => {
                if let Some(name) = self.selected.as_ref().map(|s| s.name.clone()) { let server = self.open_server(); self.voice_talked(&server, &name); }
                self.voice_reply(call, tool_reply(self.voice_context(), true));
            }
            VoiceEvent::Send { call, request, session, turn } => {
                // Destino nomeado vale pela sessão escolhida, não pela tela; sem nome, a da tela neste instante.
                let target = match session {
                    Some(spoken) => self.voice_send_target(&spoken, cx),
                    None => self.selected_key().map(|key| (key, self.chat.state.state == "working"))
                        .ok_or_else(|| "Nenhuma sessão aberta na tela; nada foi enviado.".to_owned()),
                };
                let (key, was_working) = match target {
                    Ok(found) => found,
                    Err(text) => { self.voice_reply(call, tool_reply(text, false)); cx.notify(); return; }
                };
                if self.api_for(&key.server).is_none() && self.machine_api(&servers::norm(&key.server)).is_none() {
                    self.voice_reply(call, send_reply(&key.name, &Err(Failure::local(tr("server_changed")))));
                    cx.notify();
                    return;
                }
                // Nomes diferentes podem chegar à mesma sessão ("hcc" e "hcc-rust-plano"): a trava final é pela sessão.
                let fresh = match &mut self.voice.sent_turn {
                    Some((sent, keys)) if *sent == turn => keys.insert(key.clone()),
                    slot => { *slot = Some((turn, HashSet::from([key.clone()]))); true }
                };
                if !fresh {
                    crate::voice::log("send refused duplicate session");
                    self.voice_reply(call, tool_reply(crate::voice::organizer::SEND_DUPLICATE, false));
                    cx.notify();
                    return;
                }
                self.voice.watched.insert(key.clone(), was_working);
                let known = self.known_user_ids();
                // Rascunho vazio: a voz já conta a falha ao usuário, e o pedido não vira texto no compositor.
                if self.post(key.clone(), request.clone(), String::new(), false, known, None, cx) {
                    self.voice.pending_sends.push_back((key, request, call));
                } else {
                    let name = key.name.clone();
                    self.delivery.hold(key, request, false, None);
                    self.voice_reply(call, tool_reply(format!("status queued: a sessão {name} tem outro envio em andamento; o pedido entrou na fila."), true));
                }
            }
        }
        cx.notify();
    }

    /// Sessão nomeada para `send_to_session` → a chave de envio dela, sem abrir nada na tela. A da tela usa a mesma chave
    /// do compositor, para o envio casar com a bolha dela.
    fn voice_send_target(&self, spoken: &str, cx: &App) -> Result<(SessionKey, bool), String> {
        let (machine, session) = self.voice_resolve("send_to_session", spoken, cx)?;
        let on_screen = self.selected.as_ref().is_some_and(|s| s.name == session.name) && self.open_server() == machine;
        if on_screen && let Some(key) = self.selected_key() { return Ok((key, self.chat.state.state == "working")); }
        let key = SessionKey::new(&machine, &session).ok_or_else(|| format!("A sessão {} não aceita mensagens agora; nada foi enviado.", session.name))?;
        Ok((key, session.state == "working"))
    }

    fn voice_answer(&self, text: &str) {
        if let Some(voice) = &self.voice.call { voice.session_answer(text.to_owned()); }
    }

    /// Pergunta curta do organizador à sessão da tela; a resposta volta por `voice_ask_intercept`.
    fn voice_ask(&mut self, question: &str, cx: &mut Context<Self>) {
        if self.voice.pending_question.is_some() { self.voice_answer("Já há uma pergunta aguardando a sessão."); return; }
        let Some(key) = self.selected_key().filter(|key| self.api_for(&key.server).is_some()) else {
            self.voice_answer("Nenhuma sessão aberta na tela; a pergunta não foi enviada.");
            return;
        };
        let known = self.known_user_ids();
        let text = question_text(question);
        let bytes = text.len();
        if self.post(key.clone(), text, String::new(), false, known, None, cx) {
            crate::voice::log(format!("ask_session sent bytes={bytes}"));
            self.voice.pending_question = Some((key, std::time::Instant::now()));
        } else {
            crate::voice::log("ask_session refused busy");
            self.voice_answer("A sessão está ocupada; pergunta não enviada.");
        }
    }

    /// Tique de 1 s: pergunta sem resposta por `ASK_TIMEOUT` volta ao organizador como falha.
    fn voice_ask_expiry(&mut self) {
        let Some((_, since)) = &self.voice.pending_question else { return };
        if !question_expired(*since, std::time::Instant::now()) { return; }
        self.voice.pending_question = None;
        crate::voice::log("ask_session timeout");
        self.voice_answer("A sessão não respondeu a tempo.");
    }

    /// Fim de turno de `key`: se a pergunta pendente é dela, a resposta vai ao organizador e não é falada como resultado.
    /// `waiting`: o que dizer se a sessão parou esperando o usuário sem responder.
    fn voice_ask_intercept(&mut self, key: &SessionKey, events: &[(String, String, String)], waiting: Option<String>) -> bool {
        if self.voice.pending_question.as_ref().is_none_or(|(k, _)| k != key) { return false; }
        let answer = match question_answer(events) {
            Some((id, text)) => self.voice.spoken.insert(id).then_some(text),
            None if question_marked(events) => None,
            // A pergunta ainda não está no histórico lido: este fim de turno é de outro pedido.
            None => return false,
        };
        let Some(text) = answer.or(waiting) else {
            // Sem resposta ainda: a próxima assistant_msg tenta de novo (o prazo de `ASK_TIMEOUT` segue valendo).
            self.voice.reply_pending = Some(key.clone());
            return true;
        };
        self.voice.pending_question = None;
        crate::voice::log(format!("ask_session answered bytes={}", text.len()));
        self.voice_answer(&text);
        true
    }

    pub(super) fn voice_sent(&mut self, key: &SessionKey, text: &str, result: &Result<Delivery, Failure>) {
        // Pergunta que não chegou não espera os 10 min do prazo.
        if result.is_err() && text.starts_with(ASK_MARK) && self.voice.pending_question.as_ref().is_some_and(|(k, _)| k == key) {
            self.voice.pending_question = None;
            crate::voice::log("ask_session delivery failed");
            self.voice_answer("A pergunta não chegou à sessão.");
        }
        if self.voice.pending_plan.as_ref().is_some_and(|(k, t)| k == key && t == text) {
            self.voice.pending_plan = None;
            if result.is_err() {
                crate::voice::log("plan delivery failed");
                self.voice.error = Some(tr("voice_plan_failed"));
                self.voice_answer("O plano não chegou à sessão.");
            } else if let Some(voice) = &self.voice.call { voice.plan_delivered(); }
        }
        let Some(index) = self.voice.pending_sends.iter().position(|(k, t, _)| k == key && t == text) else { return };
        let Some((_, _, call)) = self.voice.pending_sends.remove(index) else { return };
        self.voice_reply(call, send_reply(&key.name, result));
    }

    pub(super) fn voice_session_opened(&mut self, cx: &mut Context<Self>) {
        if self.voice.call.is_none() { return; }
        let name = self.selected.as_ref().map(|s| s.name.clone());
        // Mesmo nome em outra máquina é outra sessão: a chave inclui a máquina.
        let key = self.selected_key();
        if name == self.voice.target && key == self.voice.target_key { return; }
        self.voice.target = name.clone();
        self.voice.target_key = key;
        if self.voice.pending_question.take().is_some() {
            crate::voice::log("ask_session timeout switched");
            self.voice_answer("A sessão não respondeu: a conversa trocou de sessão.");
        }
        // A resposta atrasada da sessão que saiu da tela passa a vir pela lista e pelo histórico.
        if let Some(key) = self.voice.reply_pending.take() { self.voice.watched.insert(key, true); }
        // Voltar a uma sessão que acabou enquanto estava fora: a lista não a vigia mais (é a aberta), então a espera volta.
        if let Some(key) = self.selected_key() && self.voice.watched.get(&key) == Some(&true) {
            self.voice.watched.remove(&key);
            self.voice.reply_pending = Some(key);
            self.arm_reply_wait(cx);
        }
        let cwd = self.local_session_dir();
        let Some(voice) = &self.voice.call else { return };
        let name = name.unwrap_or_default();
        // O chat novo ainda está vazio aqui; o organizador lê o resto pelo read_session.
        voice.retarget(name.clone(), format!("A sessão na tela agora é {name}."), cwd);
    }

    pub(super) fn voice_turn_finished(&mut self, state: &str, cx: &mut Context<Self>) {
        if self.voice.call.is_none() { return; }
        let key = self.selected_key();
        if let Some(key) = &key { self.voice.watched.remove(key); }
        if state == "awaiting_input" {
            let name = self.voice.target.clone().unwrap_or_default();
            let questions: Vec<&str> = self.chat.ask.iter().flat_map(|ask| ask.payload.questions.iter().map(|q| q.question.as_str())).collect();
            let events = triples(&self.chat.events);
            let reply = last_reply(&events).map(|(_, text)| text);
            let text = waiting_text(&questions, reply.as_deref());
            if let Some(key) = &key && self.voice_ask_intercept(key, &events, Some(text.clone())) { return; }
            if let Some(voice) = &self.voice.call { voice.session_result(name, text); }
            return;
        }
        // O que já está no chat pode ser só um passo do meio ("vou ler o arquivo…"): fala na próxima
        // assistant_msg ou quando a espera acabar, o que vier primeiro.
        if key.is_some() {
            self.voice.reply_pending = key;
            self.arm_reply_wait(cx);
        }
    }

    fn arm_reply_wait(&mut self, cx: &mut Context<Self>) {
        self.voice.reply_epoch += 1;
        let epoch = self.voice.reply_epoch;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(REPLY_WAIT).await;
            let _ = this.update(cx, |this, cx| this.voice_reply_wait_over(epoch, cx));
        }).detach();
    }

    fn voice_reply_wait_over(&mut self, epoch: u64, cx: &mut Context<Self>) {
        if epoch != self.voice.reply_epoch { return; }
        let Some(key) = self.voice.reply_pending.clone() else { return };
        // Saiu da sessão no meio: a pendência já foi para a vigia da lista.
        if self.selected_key().as_ref() != Some(&key) { return; }
        // Sessão ainda trabalhando: o que está no chat é passo do meio, e o fim do turno rearma a espera.
        if self.chat.state.state == "working" { return; }
        // Histórico ainda não instalado: o chat está vazio ou velho, espera mais um ciclo.
        if !self.history_installed { self.arm_reply_wait(cx); return; }
        self.voice.reply_pending = None;
        self.voice_speak_last_reply();
    }

    /// Fala a última resposta da sessão aberta se ainda não foi falada; devolve se havia uma nova.
    fn voice_speak_last_reply(&mut self) -> bool {
        let events = triples(&self.chat.events);
        if let Some(key) = self.selected_key() && self.voice_ask_intercept(&key, &events, None) { return false; }
        let Some((id, text)) = last_reply(&events) else { return false };
        if !self.voice.spoken.insert(id) { return false; }
        let name = self.voice.target.clone().unwrap_or_default();
        let server = self.open_server();
        self.voice_talked(&server, &name);
        if let Some(voice) = &self.voice.call { voice.session_result(name, text); }
        true
    }

    /// Chamado a cada `assistant_msg` da sessão aberta.
    pub(super) fn voice_message(&mut self) {
        if self.voice.reply_pending.is_some() && self.chat.state.state != "working" {
            self.voice.reply_pending = None;
            self.voice_speak_last_reply();
        }
    }

    /// Sessão que recebeu pedido e não está na tela: a lista diz quando o turno dela acabou, e a resposta vem do histórico.
    pub(super) fn voice_sessions(&mut self) {
        if self.voice.call.is_none() { return; }
        self.voice_push_names();
        self.voice_watch_followed();
        if self.voice.watched.is_empty() { return; }
        let open = self.selected_key();
        let rows: Vec<(SessionKey, String)> = self.voice.watched.keys()
            .filter(|key| Some(*key) != open.as_ref()) // a aberta é tratada pelo SSE
            .filter_map(|key| self.sessions_of(&key.server).iter().find(|s| s.name == key.name).map(|s| (key.clone(), s.state.clone())))
            .collect();
        let mut finished = Vec::new();
        for (key, state) in rows {
            let Some(was_working) = self.voice.watched.get_mut(&key) else { continue };
            if state == "working" { *was_working = true; } else if went_idle(*was_working, &state) { finished.push(key); }
        }
        for key in finished {
            // Sessão de outra máquina que não é a aberta nem a ativa: a conexão é a da lista dela.
            let api = self.api_for(&key.server).or_else(|| self.machine_api(&servers::norm(&key.server)));
            // Sem conexão agora, fica vigiada e a próxima lista tenta de novo.
            let Some(api) = api else { continue };
            self.voice.watched.remove(&key);
            let (generation, connection, tx) = (self.voice.generation, self.connection, self.tx.clone());
            self.runtime.spawn(async move {
                let result = api.history(&key.name, 20, None).await;
                let _ = tx.send(Envelope { connection, selection: None, payload: Payload::VoiceHistory(generation, key, result) }).await;
            });
        }
    }

    /// Acompanhada fora da tela entra na vigia a cada lista; a vigia sai quando a resposta é lida, e a lista seguinte
    /// a põe de volta. A aberta fica com o SSE, que já fala o fim de cada turno dela.
    fn voice_watch_followed(&mut self) {
        let open = self.selected.as_ref().map(|s| (self.open_server(), s.name.clone()));
        let mut seeds = Vec::new();
        for (server, name) in &self.voice.followed {
            if open.as_ref().is_some_and(|(s, n)| s == server && n == name) { continue; }
            if self.voice.watched.keys().any(|k| servers::norm(&k.server) == *server && k.name == *name) { continue; }
            let Some(info) = self.sessions_of(server).iter().find(|s| s.name == *name) else { continue };
            if let Some(key) = SessionKey::new(server, info) { seeds.push((key, info.state == "working")); }
        }
        self.voice.watched.extend(seeds);
    }

    /// A voz falou desta sessão agora: a troca por voz não oferece resumo dela tão cedo.
    fn voice_talked(&mut self, server: &str, name: &str) {
        self.voice.talked.insert((servers::norm(server), name.to_owned()), std::time::Instant::now());
    }

    /// A chamada conhece os nomes para o `set_mode` não confundir sessão com modo.
    fn voice_push_names(&mut self) {
        let names: Vec<String> = self.voice_candidates().into_iter().map(|(_, s)| s.name).collect();
        if names == self.voice.session_names { return; }
        if let Some(call) = &self.voice.call { call.set_sessions(names.clone()); }
        self.voice.session_names = names;
    }

    pub(super) fn voice_history(&mut self, generation: u64, key: SessionKey, result: Result<api::History, Failure>) {
        if generation != self.voice.generation { return; }
        let events = match result {
            Ok(history) => history.events,
            Err(error) => { crate::voice::log(format!("history read failed status={:?} code={:?}", error.status, error.code)); None }
        };
        // A chave já saiu de `watched`: sem este aviso o resultado da sessão fora da tela se perderia calado.
        let Some(events) = events else {
            if let Some(voice) = &self.voice.call { voice.session_result(key.name.clone(), format!("Não consegui ler a resposta da sessão {}.", key.name)); }
            return;
        };
        let events = triples(&events);
        if self.voice_ask_intercept(&key, &events, None) { return; }
        let Some((id, text)) = last_reply(&events) else { return };
        if !self.voice.spoken.insert(id) { return; }
        self.voice_talked(&key.server, &key.name);
        if let Some(voice) = &self.voice.call { voice.session_result(key.name, text); }
    }

    fn toggle_voice_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let open = !self.voice.open;
        self.close_popups();
        self.voice.open = open;
        if open && self.voice.voice_select.is_none() {
            let items: Vec<VoiceChoice> = std::iter::once(String::new()).chain(VOICES.iter().map(|v| (*v).to_owned()))
                .map(|id| VoiceChoice { id }).collect();
            let at = items.iter().position(|c| Some(&c.id) == self.voice.voice.as_ref()).unwrap_or(0);
            let picker = cx.new(|cx| SelectState::new(items, Some(gpui_kit::component::IndexPath::new(at)), window, cx));
            let sub = cx.subscribe_in(&picker, window, |this: &mut Hangar, _, event: &SelectEvent<Vec<VoiceChoice>>, _, cx| {
                let SelectEvent::Confirm(Some(id)) = event else { return };
                this.voice.voice = (!id.is_empty()).then(|| id.clone());
                this.persist_voice_prefs(cx);
                cx.notify();
            });
            self.voice.voice_select = Some((picker, sub));
        }
        if open && self.voice.account_select.is_none() && self.voice.accounts.len() > 1 {
            let (items, at) = (self.voice.accounts.clone(), self.account_index());
            let picker = cx.new(|cx| SelectState::new(items, Some(gpui_kit::component::IndexPath::new(at)), window, cx));
            let sub = cx.subscribe_in(&picker, window, |this: &mut Hangar, _, event: &SelectEvent<Vec<CodexAccount>>, _, cx| {
                let SelectEvent::Confirm(Some(home)) = event else { return };
                this.voice.account = (!home.is_empty()).then(|| home.clone());
                this.persist_voice_prefs(cx);
                // O catálogo é por conta.
                this.load_organizer_models(cx);
                cx.notify();
            });
            self.voice.account_select = Some((picker, sub));
        }
        if open && !matches!(self.voice.organizer_models, Some(Ok(_))) { self.load_organizer_models(cx); }
        cx.notify();
    }

    fn load_organizer_models(&mut self, cx: &mut Context<Self>) {
        self.voice.models_seq += 1;
        (self.voice.organizer_models, self.voice.model_select, self.voice.effort_select) = (None, Default::default(), Default::default());
        self.voice.tier_select = Default::default();
        let Some(api) = self.local_api() else { self.voice.organizer_models = Some(Err(String::new())); cx.notify(); return };
        let account = self.voice.accounts.get(self.account_index()).map_or_else(|| "default".to_owned(), |a| a.id.clone());
        let (seq, connection, tx) = (self.voice.models_seq, self.connection, self.tx.clone());
        self.runtime.spawn(async move {
            let result = api.server_read(&["model-options"], &[("provider", "codex"), ("codex_account", account.as_str())], 30).await
                .map_err(|e| Hangar::fetch_failure(&e))
                .and_then(|v| serde_json::from_value::<Vec<OrganizerModel>>(v["models"].clone()).map_err(|_| tr("invalid_response")));
            match &result { Ok(models) => crate::voice::log(format!("organizer models count={}", models.len())), Err(_) => crate::voice::log("organizer models failed") }
            let _ = tx.send(Envelope { connection, selection: None, payload: Payload::VoiceModels(seq, result) }).await;
        });
    }

    pub(super) fn receive_organizer_models(&mut self, seq: u64, result: Result<Vec<OrganizerModel>, String>, window: &mut Window, cx: &mut Context<Self>) {
        if seq != self.voice.models_seq { return; }
        self.voice.organizer_models = Some(result);
        for mode in [Mode::Direct, Mode::Plan] {
            self.build_model_pick(mode, window, cx);
            self.build_effort_pick(mode, window, cx);
            self.build_tier_pick(mode, window, cx);
        }
        cx.notify();
    }

    fn organizer_pair(&mut self, mode: Mode) -> &mut ModeModel {
        match mode { Mode::Direct => &mut self.voice.organizer.direct, Mode::Plan => &mut self.voice.organizer.plan }
    }

    /// Grava os pares e, com a chamada no ar, os entrega a ela: o do modo atual troca já no próximo turno.
    fn organizer_changed(&mut self, cx: &mut Context<Self>) {
        self.persist_voice_prefs(cx);
        if let Some(call) = &self.voice.call { call.set_models(self.voice.organizer.clone()); }
        cx.notify();
    }

    fn build_model_pick(&mut self, mode: Mode, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Ok(models)) = &self.voice.organizer_models else { return };
        let chosen = self.voice.organizer.get(mode).model.clone();
        let mut items: Vec<OrganizerChoice> = std::iter::once(OrganizerChoice { id: String::new(), label: tr("voice_organizer_default") })
            .chain(models.iter().map(|m| OrganizerChoice { id: m.id.clone(), label: m.name.clone().unwrap_or_else(|| m.id.clone()) })).collect();
        // Gravado que saiu do catálogo continua à vista: a próxima chamada ainda o usa.
        if let Some(model) = chosen.as_ref().filter(|m| !items.iter().any(|i| &i.id == *m)) { items.push(OrganizerChoice { id: model.clone(), label: model.clone() }); }
        let at = items.iter().position(|i| Some(&i.id) == chosen.as_ref()).unwrap_or(0);
        let picker = cx.new(|cx| SelectState::new(items, Some(gpui_kit::component::IndexPath::new(at)), window, cx));
        let sub = cx.subscribe_in(&picker, window, move |this: &mut Hangar, _, event: &SelectEvent<Vec<OrganizerChoice>>, window, cx| {
            let SelectEvent::Confirm(Some(id)) = event else { return };
            this.organizer_pair(mode).model = (!id.is_empty()).then(|| id.clone());
            this.build_effort_pick(mode, window, cx);
            this.build_tier_pick(mode, window, cx);
            this.organizer_changed(cx);
        });
        self.voice.model_select[slot(mode)] = Some((picker, sub));
    }

    /// Refeito a cada troca de modelo: os esforços são do modelo, e o que ele não aceita vira o padrão.
    fn build_effort_pick(&mut self, mode: Mode, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Ok(models)) = &self.voice.organizer_models else { return };
        let pair = self.voice.organizer.get(mode);
        let efforts = organizer_efforts(models, pair.model.as_deref());
        let effort = fit_effort(&efforts, Some(&pair.effort));
        let at = efforts.iter().position(|e| *e == effort).unwrap_or(0);
        self.organizer_pair(mode).effort = effort;
        let items: Vec<OrganizerChoice> = efforts.into_iter().map(|e| OrganizerChoice { label: e.clone(), id: e }).collect();
        let picker = cx.new(|cx| SelectState::new(items, Some(gpui_kit::component::IndexPath::new(at)), window, cx));
        let sub = cx.subscribe_in(&picker, window, move |this: &mut Hangar, _, event: &SelectEvent<Vec<OrganizerChoice>>, _, cx| {
            let SelectEvent::Confirm(Some(id)) = event else { return };
            this.organizer_pair(mode).effort = id.clone();
            this.organizer_changed(cx);
        });
        self.voice.effort_select[slot(mode)] = Some((picker, sub));
    }

    /// Refeito a cada troca de modelo: Fast só aparece quando o catálogo não diz que o modelo não tem; o Fast gravado
    /// que o modelo novo não aceita volta para a velocidade da conta.
    fn build_tier_pick(&mut self, mode: Mode, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Ok(models)) = &self.voice.organizer_models else { return };
        let no_fast = fast_known_unavailable(models, self.voice.organizer.get(mode).model.as_deref());
        if no_fast && self.voice.organizer.get(mode).tier.as_deref() == Some(TIER_FAST) { self.organizer_pair(mode).tier = None; }
        let mut items = vec![OrganizerChoice { id: String::new(), label: tr("voice_tier_account") },
            OrganizerChoice { id: TIER_STANDARD.into(), label: tr("voice_tier_standard") }];
        if !no_fast { items.push(OrganizerChoice { id: TIER_FAST.into(), label: tr("ctl_fast") }); }
        let chosen = tier_id(self.voice.organizer.get(mode).tier.as_deref());
        let at = items.iter().position(|i| i.id == chosen).unwrap_or(0);
        let picker = cx.new(|cx| SelectState::new(items, Some(gpui_kit::component::IndexPath::new(at)), window, cx));
        let sub = cx.subscribe_in(&picker, window, move |this: &mut Hangar, _, event: &SelectEvent<Vec<OrganizerChoice>>, _, cx| {
            let SelectEvent::Confirm(Some(id)) = event else { return };
            this.organizer_pair(mode).tier = (!id.is_empty()).then(|| id.clone());
            this.organizer_changed(cx);
        });
        self.voice.tier_select[slot(mode)] = Some((picker, sub));
    }

    fn persist_voice_prefs(&mut self, cx: &mut Context<Self>) {
        let saved = SavedVoice { voice: self.voice.voice.clone(), account: self.voice.account.clone(), organizer: self.voice.organizer.clone() };
        let write = cx.background_executor().spawn(async move { save_voice(&saved) });
        cx.spawn(async move |this, cx| {
            if let Err(error) = write.await {
                let _ = this.update(cx, |this, cx| {
                    this.voice.error = Some(format!("{} {error}", tr("voice_not_saved")));
                    cx.notify();
                });
            }
        }).detach();
    }

    /// Pílula da barra de cima; `None` sem a opção beta ou sem Codex nesta máquina.
    pub(super) fn render_voice_pill(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.voice.enabled || self.voice.codex.is_none() { return None; }
        let title = tr_shared("codex_voice_title", &[]);
        let button = Button::new("topbar-voice").ghost().small().h(px(26.)).px(px(8.)).rounded_full().selected(self.voice.open)
            .accessibility_label(title.clone())
            .on_click(cx.listener(|this, _, window, cx| this.toggle_voice_panel(window, cx)));
        let button = if self.voice.call.is_none() {
            button.tooltip(title).child(div().flex().items_center().gap(px(6.))
                .child(chrome::small_icon(IconName::Mic, 14., theme::muted()))
                .when(self.voice.error.is_some(), |el| el.child(div().size(px(6.)).rounded_full().bg(theme::danger()))))
        } else {
            let target = self.voice.target.as_deref().map(short);
            button.child(div().flex().items_center().gap(px(6.)).text_size(px(12.5))
                .child(self.render_equalizer(3., 16., 2.))
                .children(self.call_time().map(|time| div().text_color(theme::text()).child(time)))
                .child(div().text_color(self.voice_state_color()).child(self.voice_status()))
                .when(self.voice.mode == Mode::Plan, |el| el.child(div().flex_shrink_0().px(px(5.)).rounded(px(4.)).border_1().border_color(theme::border())
                    .text_size(px(10.)).text_color(theme::muted()).child(tr("voice_planning"))))
                .children(target.map(|name| div().text_color(theme::faint()).child(name)))
                .when(self.voice.draft.is_some(), |el| el.child(div().size(px(6.)).rounded_full().bg(theme::warning())))
                .when(self.voice.error.is_some(), |el| el.child(div().size(px(6.)).rounded_full().bg(theme::danger())))
                .child(beta_badge()))
        };
        Some(popup::anchor(div(), "topbar-voice").child(button).into_any_element())
    }

    fn voice_status(&self) -> String {
        let Some(Phase::Live) = &self.voice.phase else { return tr_shared("codex_voice_connecting", &[]) };
        match self.shown_speaker() {
            Speaker::Voice => tr("voice_assistant_speaking"),
            _ if self.voice.muted => tr_shared("codex_voice_muted", &[]),
            Speaker::You => tr("voice_you_speaking"),
            Speaker::Idle => match self.voice.activity {
                CallActivity::Thinking => tr("voice_thinking"),
                CallActivity::Searching => tr("voice_searching"),
                CallActivity::Working => tr("voice_working"),
                CallActivity::Idle => tr_shared("codex_voice_listening", &[]),
            },
        }
    }

    fn shown_speaker(&self) -> Speaker { self.voice.shown.map_or(Speaker::Idle, |(who, _)| who) }

    fn call_time(&self) -> Option<String> { self.voice.live_since.map(|since| call_clock(since.elapsed())) }

    /// Cor do estado da chamada: a mesma no equalizador e no rótulo.
    fn voice_state_color(&self) -> Hsla {
        if !matches!(self.voice.phase, Some(Phase::Live)) { return theme::muted(); }
        match self.shown_speaker() {
            Speaker::Voice => theme::success(),
            _ if self.voice.muted => theme::muted(),
            Speaker::You => theme::accent(),
            Speaker::Idle => match self.voice.activity {
                CallActivity::Idle => theme::muted(),
                CallActivity::Thinking => theme::text(),
                CallActivity::Searching | CallActivity::Working => theme::warning(),
            },
        }
    }

    /// Você: barras de baixo para cima na cor de destaque. Voz: do centro, em verde. Ouvindo: uma barra lenta;
    /// pensando ou agindo: mais rápida e alta, na cor do estado. Só a altura de um div muda: nada de transform.
    fn render_equalizer(&self, min: f32, max: f32, width: f32) -> Div {
        let (input, output) = self.voice.levels;
        let who = self.shown_speaker();
        let row = div().h(px(max)).flex().gap(px(2.));
        if who == Speaker::Idle && !self.voice.muted && matches!(self.voice.phase, Some(Phase::Live)) {
            let elapsed = self.voice.live_since.map_or(Duration::ZERO, |since| since.elapsed());
            let bars = if self.voice.activity == CallActivity::Idle { wave_bars(elapsed, min, max, ANIM_STEP * 2, 0.2) }
                else { wave_bars(elapsed, min, max, ANIM_STEP, 0.6) };
            let color = if self.voice.activity == CallActivity::Idle { theme::faint() } else { self.voice_state_color() };
            return row.items_center().children(bars.map(|h| div().w(px(width)).h(px(h)).rounded_full().bg(color)));
        }
        let (level, color) = match who {
            Speaker::You => (input, theme::accent()),
            Speaker::Voice => (output, theme::success()),
            Speaker::Idle => (0., theme::faint()),
        };
        let row = if who == Speaker::You { row.items_end() } else { row.items_center() };
        row.children(equalizer(level, self.voice.frame, min, max).map(|h| div().w(px(width)).h(px(h)).rounded_full().bg(color)))
    }

    /// Só o que foi medido: contexto da thread do organizador e janelas da conta. Sem dado, nada aparece.
    fn render_voice_usage(&self) -> Option<AnyElement> {
        let tone = |pct: f64| if pct >= 90. { theme::danger() } else if pct >= 70. { theme::warning() } else { theme::text() };
        let meter_row = |label: String, pct: f64| div().flex().justify_between().gap_2().text_xs().font_family(crate::theme::MONO)
            .child(div().text_color(theme::faint()).child(label))
            .child(div().font_weight(FontWeight::SEMIBOLD).text_color(tone(pct)).child(format!("{}%", pct.round())));
        let context = self.voice.context.map(|(used, window)| {
            let title = tr("voice_context_title");
            let block = div().flex().flex_col();
            match window {
                Some(total) => {
                    let pct = (used as f64 / total as f64 * 100.).min(100.);
                    block.child(meter_row(title, pct)).child(div().mt(px(6.)).child(chrome::meter(pct)))
                        .child(div().mt(px(3.)).text_size(px(11.)).text_color(theme::faint())
                            .child(tr("side_ctx_of").replace("{used}", &side::tokens(used as f64)).replace("{total}", &side::tokens(total as f64))))
                }
                None => block.child(div().flex().justify_between().gap_2().text_xs().font_family(crate::theme::MONO)
                    .child(div().text_color(theme::faint()).child(title))
                    .child(div().text_color(theme::text()).child(tr("voice_context_tokens").replace("{used}", &side::tokens(used as f64))))),
            }
        });
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0., |d| d.as_secs_f64());
        let windows: Vec<(String, f64, String)> = [(tr("limit_5h"), self.voice.five_hour), (tr("side_limit_7d"), self.voice.seven_day)].into_iter()
            .filter_map(|(label, window)| { let (pct, reset) = window?; Some((label, pct, accounts::reset_text(reset.map(|r| r as f64), now))) }).collect();
        if context.is_none() && windows.is_empty() { return None; }
        Some(div().flex().flex_col().gap(px(10.))
            .children(context)
            .when(!windows.is_empty(), |el| el.child(div().flex().flex_col().gap(px(6.))
                .child(div().text_xs().font_weight(FontWeight::MEDIUM).text_color(theme::muted()).child(tr("voice_account_limits")))
                .child(div().flex().flex_wrap().gap_3().children(windows.into_iter().map(|(label, pct, reset)| {
                    div().flex_grow(1.).flex_basis(px(118.)).min_w_0().flex().flex_col()
                        .child(meter_row(label, pct))
                        .child(div().mt(px(6.)).child(chrome::meter(pct)))
                        .when(!reset.is_empty(), |el| el.child(div().mt(px(3.)).text_size(px(11.)).text_color(theme::faint()).truncate()
                            .child(tr("side_resets").replace("{reset}", &reset))))
                }))))).into_any_element())
    }

    /// Conteúdo cru do painel: o `render_popup` já põe a superfície.
    /// Uma linha com o que os Ajustes fechados escondem: voz, conta e o par de cada modo.
    fn voice_settings_summary(&self) -> String {
        let mut parts = vec![self.voice.voice.clone().unwrap_or_else(|| tr_shared("codex_voice_default", &[]).to_string())];
        if self.voice.accounts.len() > 1 { parts.extend(self.voice.accounts.get(self.account_index()).map(|a| a.title().to_string())); }
        for (mode, title) in [(Mode::Direct, "voice_mode_direct"), (Mode::Plan, "voice_mode_plan")] {
            let pair = self.voice.organizer.get(mode);
            let model = match (&pair.model, &self.voice.organizer_models) {
                (None, _) => tr("voice_organizer_default"),
                (Some(id), Some(Ok(models))) => models.iter().find(|m| &m.id == id).and_then(|m| m.name.clone()).unwrap_or_else(|| id.clone()),
                (Some(id), _) => id.clone(),
            };
            let tier = pair.tier.as_deref().map(|t| format!(" {}", tier_label(Some(t)))).unwrap_or_default();
            parts.push(format!("{}: {model} {}{tier}", tr(title), pair.effort));
        }
        parts.join(" · ")
    }

    fn model_name(&self, id: &str) -> String {
        match &self.voice.organizer_models {
            Some(Ok(models)) => models.iter().find(|m| m.id == id).and_then(|m| m.name.clone()).unwrap_or_else(|| id.to_owned()),
            _ => id.to_owned(),
        }
    }

    /// O que o organizador usa agora, como a thread do Codex informou.
    fn organizer_now(&self) -> Option<String> {
        let now = self.voice.effective.as_ref()?;
        let model = now.model.as_deref().map_or_else(|| tr("voice_organizer_default"), |m| self.model_name(m));
        Some(tr("voice_organizer_now").replace("{model}", &model).replace("{effort}", now.effort.as_deref().unwrap_or("?"))
            .replace("{tier}", &tier_label(now.tier.as_deref())))
    }

    fn following_text(&self) -> Option<String> {
        if self.voice.followed.is_empty() { return None; }
        let mut names: Vec<&str> = self.voice.followed.iter().map(|(_, name)| name.as_str()).collect();
        names.sort_unstable();
        Some(tr("voice_following").replace("{sessions}", &names.join(", ")))
    }

    /// Bastidores: o que chegou ao organizador e o que ele respondeu, do mais novo para o mais velho.
    fn render_backstage(&self, cx: &mut Context<Self>) -> AnyElement {
        let open = self.voice.backstage_open;
        let toggle = Button::new("voice-backstage-toggle").ghost().small().w_full().toggled(open)
            .icon(if open { IconName::ChevronDown } else { IconName::ChevronRight })
            .child(div().flex_1().min_w_0().text_xs().font_weight(FontWeight::MEDIUM).text_color(theme::muted())
                .child(format!("{} ({})", tr("voice_backstage"), self.voice.backstage.len())))
            .on_click(cx.listener(move |this, _, _, cx| { this.voice.backstage_open = !open; cx.notify(); }));
        let block = div().flex().flex_col().gap(px(2.)).child(toggle);
        if !open { return block.into_any_element(); }
        let lines: Vec<AnyElement> = self.voice.backstage.iter().rev().map(|line| {
            let (label, text, color) = match line {
                Backstage::Heard(text) => (tr("voice_backstage_heard"), text.clone(), theme::accent()),
                Backstage::Result(session) if session.is_empty() => (tr("voice_backstage_answer_session"), String::new(), theme::warning()),
                Backstage::Result(session) => (tr("voice_backstage_result").replace("{session}", session), String::new(), theme::warning()),
                Backstage::Answer(text) => (tr("voice_backstage_answer"), text.clone(), theme::success()),
            };
            div().flex().flex_col().gap(px(1.)).py(px(3.))
                .child(div().text_size(px(10.5)).font_weight(FontWeight::MEDIUM).text_color(color).child(label))
                .when(!text.is_empty(), |el| el.child(div().text_xs().text_color(theme::text()).whitespace_normal().child(clip(text.trim(), 600))))
                .into_any_element()
        }).collect();
        let content = if lines.is_empty() {
            div().px(px(8.)).text_xs().text_color(theme::faint()).whitespace_normal().child(tr("voice_backstage_empty")).into_any_element()
        } else {
            scrolled("voice-backstage-scroll", &self.voice.backstage_scroll, 220., div().px(px(8.)).flex().flex_col().children(lines))
        };
        block.child(content).into_any_element()
    }

    /// Conteúdo cru do painel: o `render_popup` já põe a superfície. Estado da chamada em cima e botões embaixo ficam
    /// presos; o miolo rola quando o cartão não cabe na janela.
    pub(super) fn render_voice_panel(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let live = self.voice.call.is_some();
        let top = popup::anchor_bounds("topbar-voice").map_or(px(0.), |t| t.bottom());
        let height = (window.viewport_size().height - top - px(32.)).max(px(240.));
        let mut header = div().flex_none().flex().flex_col().gap(px(12.)).p(px(16.)).pb(px(12.))
            .child(div().flex().items_center().gap(px(8.))
                .child(div().text_sm().font_weight(FontWeight::MEDIUM).text_color(theme::text()).child(tr_shared("codex_voice_title", &[])))
                .child(beta_badge()));
        let mut body = div().flex().flex_col().gap(px(12.)).px(px(16.)).pb(px(4.));
        if live {
            // O estado em destaque, e logo abaixo o que o organizador faz e pensa neste turno.
            let action = self.voice.action.as_ref().map(action_text);
            let thought = thought_tail(&self.voice.thought, 3, 140);
            // Texto solto não vira nó de acessibilidade: o estado inteiro vai no nome do bloco.
            let spoken = [Some(self.voice_status().to_string()), self.voice.target.clone(), action.clone()]
                .into_iter().flatten().collect::<Vec<_>>().join(" · ");
            header = header.child(div().id("voice-status").role(Role::Status).aria_label(spoken).flex().flex_col().gap(px(8.)).p(px(12.)).rounded(px(10.)).border_1().border_color(theme::border())
                .child(div().flex().items_center().gap(px(12.))
                    .child(self.render_equalizer(4., 28., 4.).gap(px(3.)))
                    .child(div().flex_1().min_w_0().flex().flex_col().gap(px(2.))
                        .child(div().text_base().font_weight(FontWeight::SEMIBOLD).text_color(self.voice_state_color()).child(self.voice_status()))
                        .children(self.voice.target.as_deref().map(|name| div().text_xs().text_color(theme::faint()).truncate().child(name.to_owned()))))
                    .children(self.call_time().map(|time| div().flex_shrink_0().text_lg().text_color(theme::text()).child(time))))
                .children(action.map(|text| div().text_sm().text_color(theme::text()).truncate().child(text)))
                .when(!thought.is_empty(), |el| el.child(div().flex().flex_col().gap(px(2.))
                    .children(thought.into_iter().map(|line| div().text_xs().text_color(theme::muted()).truncate().child(line))))));
        }
        let ready = live && matches!(self.voice.phase, Some(Phase::Live));
        body = body.child(div().flex().items_center().gap(px(6.))
            .child(Button::new("voice-mode-direct").ghost().small().rounded_full().label(tr("voice_mode_direct"))
                .selected(self.voice.mode == Mode::Direct).aria_selected(self.voice.mode == Mode::Direct).disabled(!ready)
                .on_click(cx.listener(|this, _, _, cx| this.set_voice_mode(Mode::Direct, cx))))
            .child(Button::new("voice-mode-plan").ghost().small().rounded_full().label(tr("voice_mode_plan"))
                .selected(self.voice.mode == Mode::Plan).aria_selected(self.voice.mode == Mode::Plan).disabled(!ready)
                .on_click(cx.listener(|this, _, _, cx| this.set_voice_mode(Mode::Plan, cx)))));
        if live {
            body = body.children(self.organizer_now().map(|text| div().text_xs().text_color(theme::muted()).truncate().child(text)));
        }
        body = body.children(self.following_text().map(|text| div().text_xs().text_color(theme::muted()).whitespace_normal().child(text)));
        if let Some((path, markdown)) = &self.voice.plan {
            let open = path.clone();
            let expanded = self.voice.plan_open.unwrap_or(self.voice.mode == Mode::Plan);
            let name = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
            let first = markdown.lines().map(|l| l.trim().trim_start_matches('#').trim()).find(|l| !l.is_empty()).unwrap_or_default().to_owned();
            // Documento lido numa caixa pequena: títulos no tamanho do texto, como nos relatórios da conversa.
            let style = gpui_kit::component::text::TextViewStyle::default().heading_font_size(|level, _| px(if level <= 1 { 13. } else { 12. }));
            body = body.child(div().flex().flex_col().gap(px(4.))
                .child(div().flex().items_center().gap(px(6.))
                    .child(Button::new("voice-plan-toggle").ghost().small().flex_shrink_0().toggled(expanded)
                        .icon(if expanded { IconName::ChevronDown } else { IconName::ChevronRight }).label(tr("voice_plan_title"))
                        .on_click(cx.listener(move |this, _, _, cx| { this.voice.plan_open = Some(!expanded); cx.notify(); })))
                    .child(div().flex_1().min_w_0().truncate().text_size(px(10.5)).text_color(theme::faint()).child(name))
                    .child(Button::new("voice-plan-open").ghost().small().flex_shrink_0().label(tr("voice_plan_open"))
                        .on_click(cx.listener(move |_, _, _, cx| cx.open_with_system(&open)))))
                .child(if expanded {
                    scrolled("voice-plan-scroll", &self.voice.plan_scroll, 180.,
                        div().text_xs().text_color(theme::text()).child(TextView::markdown("voice-plan", markdown.clone()).selectable(true).scrollable(false).style(style)))
                } else {
                    div().px(px(8.)).text_xs().text_color(theme::muted()).truncate().child(first).into_any_element()
                }));
        }
        body = body.children(self.render_voice_usage());
        if live || !self.voice.backstage.is_empty() { body = body.child(self.render_backstage(cx)); }
        let settings_open = self.voice.settings_open;
        body = body.child(div().flex().flex_col().gap(px(2.))
            .child(Button::new("voice-settings-toggle").ghost().small().w_full().toggled(settings_open)
                .icon(if settings_open { IconName::ChevronDown } else { IconName::ChevronRight })
                .child(div().flex_1().min_w_0().flex().items_center().gap(px(8.))
                    .child(div().flex_shrink_0().text_xs().font_weight(FontWeight::MEDIUM).text_color(theme::muted()).child(tr("voice_settings")))
                    .when(!settings_open, |el| el.child(div().flex_1().min_w_0().truncate().text_xs().text_color(theme::faint())
                        .child(self.voice_settings_summary()))))
                .on_click(cx.listener(move |this, _, _, cx| { this.voice.settings_open = !settings_open; cx.notify(); }))));
        if settings_open {
            body = self.render_voice_settings(body, live);
        }
        // Falha do catálogo aparece com os Ajustes fechados também.
        if let Some(Err(error)) = &self.voice.organizer_models {
            body = body.child(div().text_xs().text_color(theme::danger()).whitespace_normal()
                .child(format!("{} {error}", tr("voice_models_failed")).trim_end().to_owned()));
        }
        if let Some(draft) = &self.voice.draft {
            body = body.child(div().flex().flex_col().gap(px(4.)).p(px(10.)).rounded(px(8.)).border_1().border_color(theme::warning())
                .child(div().text_xs().text_color(theme::warning()).child(tr("voice_draft_held")))
                .child(div().text_sm().text_color(theme::text()).whitespace_normal().child(draft.clone())));
        }
        let mut footer = div().flex_none().flex().flex_col().gap(px(8.)).p(px(16.)).pt(px(12.)).border_t_1().border_color(theme::border());
        if let Some(error) = &self.voice.error {
            footer = footer.child(div().id("voice-error").role(Role::Alert).aria_label(error.clone()).text_xs().text_color(theme::danger()).whitespace_normal().child(error.clone()));
        }
        let actions = if live {
            let mute = if self.voice.muted { "codex_voice_unmute_short" } else { "codex_voice_mute_short" };
            div().flex().justify_end().gap(px(8.))
                .child(Button::new("voice-mute").ghost().small().label(tr_shared(mute, &[]))
                    .disabled(!matches!(self.voice.phase, Some(Phase::Live)))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.voice.muted = !this.voice.muted;
                        if let Some(call) = &this.voice.call { call.set_muted(this.voice.muted); }
                        cx.notify();
                    })))
                .child(Button::new("voice-stop").danger().small().label(tr_shared("codex_voice_stop_short", &[]))
                    .on_click(cx.listener(|this, _, _, cx| this.stop_voice(cx))))
        } else {
            div().flex().justify_end().child(Button::new("voice-connect").primary().small().label(tr_shared("codex_voice_connect_short", &[]))
                .on_click(cx.listener(|this, _, _, cx| this.start_voice(cx))))
        };
        let middle = div().relative().flex_shrink(1.).min_h_0().flex().flex_col()
            .child(div().id("voice-card-body").flex_shrink(1.).min_h_0().overflow_y_scroll().track_scroll(&self.voice.body_scroll).child(body))
            .child(div().absolute().inset_0().child(Scrollbar::vertical(&self.voice.body_scroll).mode(ScrollbarMode::Always)));
        div().id("voice-panel").role(Role::Group).aria_label(tr_shared("codex_voice_title", &[]))
            .max_h(height).flex().flex_col().child(header).child(middle).child(footer.child(actions)).into_any_element()
    }

    /// Voz, conta e o par do organizador de cada modo, dentro dos Ajustes abertos.
    fn render_voice_settings(&self, mut body: Div, live: bool) -> Div {
        if let Some((picker, _)) = &self.voice.voice_select {
            body = body.child(div().flex().items_center().justify_between().gap(px(12.))
                .child(div().text_xs().text_color(theme::muted()).child(tr_shared("codex_voice_label", &[])))
                .child(div().w(px(200.)).child(Select::new(picker).id("voice-select-voice").small().disabled(live).accessibility_label(tr_shared("codex_voice_label", &[])))));
        }
        if let Some((picker, _)) = &self.voice.account_select {
            body = body.child(div().flex().items_center().justify_between().gap(px(12.))
                .child(div().text_xs().text_color(theme::muted()).child(tr("voice_account")))
                .child(div().w(px(200.)).child(Select::new(picker).id("voice-select-account").small().disabled(live).accessibility_label(tr("voice_account")))));
        }
        // Abertos também na chamada: o par do modo atual troca já no próximo turno.
        for (mode, title) in [(Mode::Direct, "voice_mode_direct"), (Mode::Plan, "voice_mode_plan")] {
            let at = slot(mode);
            if self.voice.model_select[at].is_none() && self.voice.effort_select[at].is_none() { continue; }
            body = body.child(div().text_xs().font_weight(FontWeight::MEDIUM).text_color(theme::muted()).child(tr(title)));
            for (select, key) in [(&self.voice.model_select[at], "voice_organizer_model"), (&self.voice.effort_select[at], "voice_organizer_effort"),
                (&self.voice.tier_select[at], "voice_organizer_tier")] {
                let Some((picker, _)) = select else { continue };
                let label = format!("{} · {}", tr(title), tr(key));
                body = body.child(div().flex().items_center().justify_between().gap(px(12.))
                    .child(div().text_xs().text_color(theme::muted()).child(tr(key)))
                    .child(div().w(px(200.)).child(Select::new(picker).id(SharedString::from(format!("voice-select-{title}-{key}")))
                        .small().accessibility_label(label))));
            }
        }
        match &self.voice.organizer_models {
            Some(Ok(models)) => {
                let no_fast: Vec<String> = [(Mode::Direct, "voice_mode_direct"), (Mode::Plan, "voice_mode_plan")].into_iter()
                    .filter(|(mode, _)| fast_known_unavailable(models, self.voice.organizer.get(*mode).model.as_deref()))
                    .map(|(_, title)| tr(title)).collect();
                if !no_fast.is_empty() {
                    body = body.child(div().text_xs().text_color(theme::muted()).whitespace_normal()
                        .child(format!("{}: {}", no_fast.join(", "), tr("ctl_fast_unavailable"))));
                }
                body = body.child(div().text_xs().text_color(theme::faint()).whitespace_normal().child(tr("voice_organizer_hint")))
                    .child(div().text_xs().text_color(theme::faint()).whitespace_normal().child(tr("voice_tier_hint")));
            }
            None if self.voice.models_seq > 0 => body = body.child(div().text_xs().text_color(theme::muted()).child(tr("voice_models_loading"))),
            _ => {}
        }
        body
    }
}

fn beta_badge() -> Div {
    div().flex_shrink_0().px(px(5.)).rounded(px(4.)).border_1().border_color(theme::border()).text_size(px(10.)).text_color(theme::faint())
        .child(tr_shared("comum_beta", &[]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    fn ev(id: &str, kind: &str, text: &str) -> (String, String, String) { (id.into(), kind.into(), text.into()) }

    const HCC: [&str; 7] = ["hcc-rust-plano", "hcc-rust-plano-rev14", "hcc-rust-plano-rev17", "hcc-rust-plano-t14", "hcc-rust-plano-t17",
        "jev-settings-ux", "jev-config-ux"];

    #[test]
    fn partial_and_near_names_resolve() {
        let none = [false; 7];
        assert_eq!(match_session("settings UX", &HCC, &none), SessionMatch::One(5));
        assert_eq!(match_session("setting ux", &HCC, &none), SessionMatch::One(5), "transcrição sem o s");
        assert_eq!(match_session("settins", &HCC, &none), SessionMatch::One(5));
        assert_eq!(match_session("config", &HCC, &none), SessionMatch::One(6));
        assert!(matches!(match_session("ux", &HCC, &none), SessionMatch::Many(found) if found == vec![5, 6]), "as duas têm ux: pergunta");
        assert_eq!(match_session("banana", &HCC, &none), SessionMatch::None);
    }

    /// "HCC" na chamada de 09/10 19:31 casou com cinco sessões e a troca parou; a raiz da família é a pretendida.
    #[test]
    fn prefix_family_picks_its_root_unless_context_says_otherwise() {
        let none = [false; 7];
        let SessionMatch::Many(found) = match_session("HCC", &HCC, &none) else { panic!("cinco sessões casam") };
        assert_eq!(found.len(), 5);
        let subset: Vec<&str> = found.iter().map(|&i| HCC[i]).collect();
        assert_eq!(narrow_by_context(found.clone(), &subset, &[false; 5]), SessionMatch::One(0), "sem contexto: a raiz");
        assert_eq!(narrow_by_context(found.clone(), &subset, &[false, false, false, true, false]), SessionMatch::One(3),
            "a executora t14 citada na conversa vence a raiz");
        assert!(matches!(narrow_by_context(found.clone(), &subset, &[false, true, false, true, false]), SessionMatch::Many(_)),
            "duas no contexto, nenhuma raiz delas: pergunta");
        assert_eq!(narrow_by_context(found, &subset, &[true, false, false, true, false]), SessionMatch::One(0),
            "duas no contexto, uma é raiz da outra: a raiz");
    }

    #[test]
    fn real_ambiguity_still_asks() {
        let names = ["hangar", "hangar", "voz-entendimento"];
        let SessionMatch::Many(found) = match_session("hangar", &names, &[false, false, false]) else { panic!("mesmo nome em duas máquinas") };
        let subset: Vec<&str> = found.iter().map(|&i| names[i]).collect();
        assert!(matches!(narrow_by_context(found.clone(), &subset, &[false, false]), SessionMatch::Many(_)), "sem contexto: pergunta");
        assert_eq!(narrow_by_context(found, &subset, &[false, true]), SessionMatch::One(1), "a que estava na conversa");
        let jev = ["jev-settings-ux", "jev-config-ux"];
        let SessionMatch::Many(found) = match_session("jev", &jev, &[false, false]) else { panic!("as duas são jev") };
        assert!(matches!(narrow_by_context(found, &jev, &[false, false]), SessionMatch::Many(_)), "irmãs, sem raiz: pergunta");
    }

    #[test]
    fn open_with_request_answers_only_after_delivery() {
        let delivered = Ok(Delivery { ok: true, delivered: true });
        let queued = Ok(Delivery { ok: true, delivered: false });
        let failed: Result<Delivery, Failure> = Err(Failure::local("x"));
        assert!(opened_reply("s", true, None).unwrap().contains("criada e aberta"), "sem request, como antes");
        assert!(opened_reply("s", false, None).is_err());
        assert!(opened_reply("s", true, Some(&delivered)).unwrap().starts_with("Sessão s aberta e pedido enviado"));
        assert!(opened_reply("s", true, Some(&queued)).unwrap().contains("fila"));
        let both = opened_reply("s", true, Some(&failed)).unwrap_err();
        assert!(both.contains("aberta") && both.contains("não chegou"), "diz as duas coisas");
        assert!(opened_reply("s", true, Some(&Ok(Delivery { ok: false, delivered: false }))).is_err());
        assert!(opened_reply("s", false, Some(&delivered)).unwrap_err().contains("pedido enviado"));
    }

    const SCREEN: &str = "Window \"Hangar\"\n  GenericContainer #hangar-root\n    Group \"Barra\" #topbar\n      Button \"Voz\" #topbar-voice\n    Group \"Voz\" #voice-panel\n      Status \"Ouvindo · hangar-5\" #voice-status\n";

    #[test]
    fn read_screen_picks_the_area_then_settings_then_dialog_then_window() {
        assert_eq!(screen_root(SCREEN, None), Ok(None), "sem diálogo: a janela inteira");
        assert_eq!(screen_root(SCREEN, Some(" #voice-panel ")), Ok(Some("voice-panel".into())));
        let dialog = format!("{SCREEN}    Dialog \"Buscar\" #search-overlay\n");
        assert_eq!(screen_root(&dialog, None), Ok(Some("search-overlay".into())));
        let settings = format!("{dialog}    Dialog \"Configurações\" #settings-dialog\n      Group #settings-page-advanced\n");
        assert_eq!(screen_root(&settings, None), Ok(Some("settings-dialog".into())), "Configurações vencem outro diálogo");
        assert_eq!(screen_root(&settings, Some("settings-page-advanced")), Ok(Some("settings-page-advanced".into())));
        let missing = screen_root(SCREEN, Some("costs-page")).unwrap_err();
        assert!(missing.starts_with("A área costs-page não está na tela. Áreas visíveis: hangar-root, topbar, topbar-voice, voice-panel, voice-status."), "{missing}");
        assert!(!missing.contains("Ouvindo"), "só ids, nunca o texto da tela");
    }

    #[test]
    fn read_screen_cuts_whole_lines_and_says_so() {
        assert_eq!(clip_lines(SCREEN, 10_000), SCREEN);
        let cut = clip_lines(SCREEN, 60);
        assert_eq!(cut, "Window \"Hangar\"\n  GenericContainer #hangar-root\n[cortado: faltam 4 linhas; peça uma área menor]\n");
    }

    #[test]
    fn hangar_action_ids_are_unique_and_stable() {
        let ids: Vec<&str> = HANGAR_ACTIONS.iter().map(|a| a.id).collect();
        assert_eq!(ids, ["topbar-settings", "settings-back", "sidebar-new-session", "sidebar-fold", "side-toggle", "side-tab-context", "side-tab-files",
            "side-tab-activity", "side-tab-git", "terminal-show", "terminal-close", "topbar-voice", "voice-panel-close", "topbar-cost", "costs-usage", "costs-back"],
            "o organizador guarda estes ids: mudar um quebra o que ele já aprendeu na chamada");
        assert_eq!(ids.iter().collect::<HashSet<_>>().len(), ids.len());
        assert!(HANGAR_ACTIONS.iter().all(|a| !a.label.is_empty() && !a.description.is_empty()));
        let text = hangar_actions_text(&["general", "voice"]);
        assert!(text.contains("- topbar-settings: Abrir configurações.") && text.ends_with("general, voice."));
    }

    #[test]
    fn thought_keeps_a_short_tail_and_shows_last_lines() {
        let mut thought = String::new();
        push_thought(&mut thought, "**Lendo a sessão**");
        push_thought(&mut thought, "\n");
        push_thought(&mut thought, "Vou trocar ");
        push_thought(&mut thought, "de sessão");
        assert_eq!(thought_tail(&thought, 3, 140), ["Lendo a sessão", "Vou trocar de sessão"]);
        push_thought(&mut thought, &format!("\n{}", "é".repeat(3000)));
        assert!(thought.len() <= THOUGHT_KEEP, "cauda limitada e cortada em fronteira de caractere");
        let tail = thought_tail(&thought, 1, 10);
        assert_eq!(tail.len(), 1);
        assert_eq!(tail[0].chars().count(), 10);
        assert!(tail[0].ends_with('…'));
        assert!(thought_tail("a\nb\nc\nd", 3, 140) == ["b", "c", "d"]);
    }

    #[test]
    fn action_text_names_tools_searches_and_commands() {
        assert_eq!(action_text(&OrganizerAction::Tool("switch_session".into())), tr("voice_tool_switch_session"));
        assert_ne!(tr("voice_tool_switch_session"), "voice_tool_switch_session", "chave traduzida");
        assert!(action_text(&OrganizerAction::Tool("novo_tool".into())).contains("novo_tool"));
        assert!(action_text(&OrganizerAction::Search("gpui animation".into())).contains("gpui animation"));
        assert_eq!(action_text(&OrganizerAction::Search(" ".into())), tr("voice_action_search"));
        let command = action_text(&OrganizerAction::Command(format!("\n  cat {}\nsegunda", "x".repeat(200))));
        assert!(command.contains("cat x") && !command.contains("segunda") && command.ends_with('…'));
    }

    #[test]
    fn codex_accounts_put_default_first_and_skip_quota_only_and_other_kinds() {
        let list = json!([
            {"id": "codex:/h/.codex", "tipo": "codex", "codex_account": "default", "ativa": true, "nome": "default", "apelido": "", "login": {"email": "a@example.com"}},
            {"id": "codex:/h/.codex-b", "tipo": "codex", "codex_account": "b", "nome": "b", "apelido": "Second"},
            {"id": "codex:/h/quota-only", "tipo": "codex", "nome": "q"},
            {"id": "claude:/h/.claude", "tipo": "claude", "nome": "c"},
        ]);
        let accounts = codex_accounts(&list);
        assert_eq!(accounts.iter().map(|a| (a.home.as_str(), a.label.as_str())).collect::<Vec<_>>(),
            [("", "a@example.com"), ("/h/.codex-b", "Second")]);
        // Só a padrão: o seletor não tem o que escolher.
        let only = json!([{"id": "codex:/h/.codex", "tipo": "codex", "codex_account": "default", "ativa": true, "nome": "d"}]);
        assert_eq!(codex_accounts(&only).len(), 1);
    }

    #[test]
    fn chosen_home_falls_back_when_missing_or_gone() {
        let accounts = vec![CodexAccount { home: "/h/.codex-b".into(), label: "b".into(), id: "b".into() }];
        assert_eq!(chosen_home(&accounts, Some("/h/.codex-b")), Some(std::path::PathBuf::from("/h/.codex-b")));
        assert_eq!(chosen_home(&accounts, Some("/h/.codex-gone")), None);
        assert_eq!(chosen_home(&accounts, Some("")), None);
        assert_eq!(chosen_home(&accounts, None), None);
    }

    #[test]
    fn saved_voice_defaults_and_reads_organizer_choice() {
        // Arquivo antigo, sem as chaves do organizador: modelo do config e esforço padrão.
        let old = parse_saved_voice(&json!({"voice": "ash", "codex_home": "/h/.codex-b"}));
        assert_eq!(old, SavedVoice { voice: Some("ash".into()), account: Some("/h/.codex-b".into()), organizer: ModeModels::default() });
        assert_eq!(parse_saved_voice(&json!({})), SavedVoice::default());
        assert_eq!(ModeModels::default().plan, ModeModel { model: None, effort: "low".into(), tier: None });
        // Um par só (antes dos modos) vale para os dois.
        let single = parse_saved_voice(&json!({"organizer_model": "gpt-x", "organizer_effort": "high", "organizer_tier": "priority"})).organizer;
        let pair = ModeModel { model: Some("gpt-x".into()), effort: "high".into(), tier: Some("priority".into()) };
        assert_eq!(single, ModeModels { direct: pair.clone(), plan: pair.clone() });
        // Planejar gravado vence, inclusive "modelo do config" (null) e a velocidade da conta.
        let both = parse_saved_voice(&json!({"organizer_model": "gpt-x", "organizer_effort": "high", "organizer_tier": "priority",
            "organizer_plan": {"model": null, "effort": "xhigh"}})).organizer;
        assert_eq!(both, ModeModels { direct: pair, plan: ModeModel { model: None, effort: "xhigh".into(), tier: None } });
    }

    #[test]
    fn fast_is_offered_unless_the_catalog_says_no() {
        let models: Vec<OrganizerModel> = serde_json::from_value(json!([
            {"id": "a", "service_tiers": [{"id": "priority", "name": "Fast"}]}, {"id": "b", "service_tiers": []}])).unwrap();
        assert!(!fast_known_unavailable(&models, Some("a")));
        assert!(fast_known_unavailable(&models, Some("b")));
        assert!(!fast_known_unavailable(&models, None), "modelo do config: não se sabe");
        assert!(!fast_known_unavailable(&models, Some("fora")), "fora do catálogo: não se sabe");
    }

    #[test]
    fn organizer_effort_follows_the_model() {
        let models: Vec<OrganizerModel> = serde_json::from_value(json!([{"id": "a", "efforts": ["medium", "xhigh"]}, {"id": "b"}])).unwrap();
        let basic = ["low", "medium", "high"].map(str::to_owned).to_vec();
        assert_eq!(organizer_efforts(&models, None), basic, "modelo do config: lista básica");
        assert_eq!(organizer_efforts(&models, Some("b")), basic, "catálogo sem esforços: lista básica");
        let a = organizer_efforts(&models, Some("a"));
        assert_eq!(a, ["medium", "xhigh"]);
        assert_eq!(fit_effort(&basic, None), "low");
        assert_eq!(fit_effort(&a, Some("xhigh")), "xhigh");
        assert_eq!(fit_effort(&a, Some("low")), "medium", "sem o gravado nem o padrão, o primeiro");
    }

    #[test]
    fn session_match_prefers_exact_and_ignores_separators() {
        let names = ["hangar", "hangar-5", "shop-web", "Shop_Api"];
        let active = [true; 4];
        assert_eq!(match_session("hangar", &names, &active), SessionMatch::One(0), "igual vence parcial");
        assert_eq!(match_session("shop web", &names, &active), SessionMatch::One(2));
        assert_eq!(match_session("SHOP-API", &names, &active), SessionMatch::One(3), "caixa e traço não contam");
        assert_eq!(match_session("hangar 5", &names, &active), SessionMatch::One(1));
        assert_eq!(match_session("shop", &names, &active), SessionMatch::Many(vec![2, 3]));
        assert_eq!(match_session("cloudflare", &names, &active), SessionMatch::None);
        assert_eq!(match_session(" - ", &names, &active), SessionMatch::None, "consulta vazia não casa tudo");
        let names = ["grupos-rust-plano", "rust-parte5-claude", "gpt-sol"];
        let active = [true; 3];
        assert_eq!(match_session("grupos", &names, &active), SessionMatch::One(0), "pedaço do nome");
        assert_eq!(match_session("rust grupos", &names, &active), SessionMatch::One(0), "palavras em qualquer ordem");
        assert_eq!(match_session("plano do rust", &names, &active), SessionMatch::One(0), "palavra curta não conta");
        assert_eq!(match_session("plano do claude", &names, &active), SessionMatch::None, "palavra que não está no nome não casa");
        assert_eq!(match_session("rust", &names, &active), SessionMatch::Many(vec![0, 1]));
    }

    #[test]
    fn folder_pick_prefers_exact_and_lists_ambiguity() {
        let f = |n: &str, p: &str| (n.to_owned(), p.to_owned());
        let folders = [f("Projetos", "/h/Projetos"), f("hangar", "/h/Projetos/hangar"), f("hangar-5", "/h/Projetos/hangar-5"),
            f("hangar", "/h/Projetos/hangar"), f("shop-web", "/h/Projetos/shop-web"), f("shop-api", "/h/Work/shop-api")];
        assert_eq!(pick_folder("hangar", &folders, 0), Ok("/h/Projetos/hangar".into()), "igual vence parcial; caminho repetido conta uma vez");
        assert_eq!(pick_folder("shop web", &folders, 0), Ok("/h/Projetos/shop-web".into()));
        let many = pick_folder("shop", &folders, 0).unwrap_err();
        assert!(many.contains("/h/Projetos/shop-web") && many.contains("/h/Work/shop-api"));
        assert!(pick_folder("loja", &folders, 2).unwrap_err().contains("2 raiz"), "raiz não lida aparece");
        let twins = [f("api", "/a/api"), f("api", "/b/api")];
        assert!(pick_folder("api", &twins, 0).is_err(), "mesmo nome em duas raízes não é palpite");
    }

    #[test]
    fn paths_skip_the_folder_search() {
        for path in ["/home/x/p", "~/p", "C:\\Users\\x", "D:/w", "\\\\nas\\share"] { assert!(looks_like_path(path), "{path}"); }
        for name in ["hangar", "minha loja", "a:b"] { assert!(!looks_like_path(name), "{name}"); }
    }

    #[test]
    fn sessions_text_marks_screen_machine_and_unreachable() {
        let row = |name: &str, machine: Option<&str>, on_screen: bool| Listed { name: name.into(), machine: machine.map(str::to_owned),
            provider: "codex".into(), state: "working".into(), folder: "hangar".into(), on_screen, followed: !on_screen };
        let text = sessions_text(&[row("a", None, true), row("b", Some("casa"), false)], &["vps".into()]);
        assert_eq!(text, "- a: codex, working, pasta hangar, na tela\n- b (máquina casa): codex, working, pasta hangar, acompanhada\n\
            Máquina vps sem resposta; as sessões dela não estão aqui.");
        assert_eq!(sessions_text(&[], &[]), "Nenhuma sessão aberta.");
    }

    /// Duas `hangar`: a do Notebook (máquina ativa) e a do PC `jefferson-felizardo`.
    fn two_hangars() -> ([&'static str; 3], Vec<Vec<String>>, [bool; 3]) {
        let notebook = machine_aliases("http://127.0.0.1:8765", "Notebook");
        let pc = machine_aliases("https://jefferson-felizardo.tailcac351.ts.net", "jefferson-felizardo");
        (["hangar", "hangar", "pm18920-api"], vec![notebook.clone(), pc, notebook], [true, false, true])
    }

    #[test]
    fn same_session_name_on_two_machines_resolves_by_machine() {
        let (names, machines, active) = two_hangars();
        let pick = |spoken: &str| match_qualified(spoken, &names, &machines, &active);
        assert_eq!(pick("hangar"), SessionMatch::One(0), "sem máquina: a da máquina ativa, como antes");
        assert_eq!(pick("Hangar lá no PC Jefferson-Felizardo"), SessionMatch::One(1));
        assert_eq!(pick("jefferson-felizardo::hangar"), SessionMatch::One(1));
        assert_eq!(pick("hangar (máquina jefferson-felizardo)"), SessionMatch::One(1));
        assert_eq!(pick("hangar em jefferson-felizardo"), SessionMatch::One(1), "o formato que a pergunta de ambiguidade mostra");
        assert_eq!(pick("hangar no notebook"), SessionMatch::One(0));
        assert_eq!(pick("vps::hangar"), SessionMatch::None, "máquina dita e inexistente não cai noutra");
        assert_eq!(pick("pm18920 no PC jefferson-felizardo"), SessionMatch::None, "a sessão não está naquela máquina");
        // Sem preferência pela ativa, o nome sozinho continua ambíguo e quem chama pergunta.
        assert_eq!(match_qualified("hangar", &names, &machines, &[false, false, false]), SessionMatch::Many(vec![0, 1]));
    }

    #[test]
    fn machine_aliases_take_label_and_host() {
        assert_eq!(machine_aliases("https://jefferson-felizardo.tailcac351.ts.net", "PC"),
            vec!["jeffersonfelizardotailcac351tsnet".to_owned(), "jeffersonfelizardo".to_owned()], "rótulo curto demais fica de fora");
    }

    #[test]
    fn switch_offer_takes_the_yes_for_an_offered_session_only() {
        let now = std::time::Instant::now();
        let mut offer = SwitchOffer::default();
        assert!(!offer.take("pc", "hangar", now), "sem pergunta, nada vale");
        offer.offer(vec![("notebook".into(), "hangar".into()), ("pc".into(), "hangar".into())], now);
        assert!(!offer.take("pc", "outra", now), "só as oferecidas");
        assert!(offer.take("pc", "hangar", now + Duration::from_secs(5)), "o 'isso' depois da pergunta");
        assert!(!offer.take("notebook", "hangar", now + Duration::from_secs(6)), "usada, acabou");
        offer.offer(vec![("pc".into(), "hangar".into())], now);
        assert!(!offer.take("pc", "hangar", now + SWITCH_OFFER_WINDOW), "fora da janela não vale");
    }

    #[test]
    fn switch_offers_a_summary_unless_the_voice_talked_about_it_recently() {
        let (mut talked, now) = (HashMap::new(), std::time::Instant::now());
        let pc = ("delphi-02".to_owned(), "PWorkStation".to_owned());
        assert!(offer_summary(&mut talked, pc.clone(), now), "primeira vez: pergunta");
        assert!(!offer_summary(&mut talked, pc.clone(), now + Duration::from_secs(60)), "vai e volta: não pergunta de novo");
        assert!(offer_summary(&mut talked, ("notebook".into(), "voz".into()), now), "outra sessão: pergunta");
        assert!(offer_summary(&mut talked, pc, now + TALKED_WINDOW), "passado o tempo, pergunta de novo");
    }

    #[test]
    fn jev_acts_only_with_a_sure_target_and_says_why_not() {
        use crate::voice::jev::{Decision, Intent};
        let d = |intent: Intent, ip: f64, session: Option<(usize, f64)>, action: Option<(usize, f64)>| Decision { intent: Some((intent, ip)), session, action };
        assert_eq!(jev_switch_choice(&d(Intent::Switch, 0.74, Some((14, 0.98)), None)), Ok(14), "'volta pra voz': destino certo basta com troca provável");
        assert_eq!(jev_switch_choice(&d(Intent::Switch, 0.91, Some((18, 0.94)), None)), Ok(18), "PWorkStation em outra máquina");
        assert_eq!(jev_switch_choice(&d(Intent::Switch, 0.89, Some((14, 0.58)), None)), Err("low-session"));
        assert_eq!(jev_switch_choice(&d(Intent::Switch, 0.98, Some((14, 0.89)), None)), Ok(14), "'vai pro voz' com destino 0,89 agora troca");
        assert_eq!(jev_switch_choice(&d(Intent::Switch, 1.0, Some((14, 0.63)), None)), Err("low-session"), "abaixo de 0,65 segue pelo organizador");
        assert_eq!(jev_switch_choice(&d(Intent::Switch, 0.64, Some((14, 0.99)), None)), Err("low-intent"));
        assert_eq!(jev_switch_choice(&d(Intent::Talk, 0.9, Some((1, 0.99)), None)), Err("not-switch"));
        assert_eq!(jev_switch_choice(&Decision::default()), Err("not-switch"), "sem resposta");
        assert_eq!(jev_action_choice(&d(Intent::Screen, 1.0, None, Some((10, 0.95)))), Ok(10));
        assert_eq!(jev_action_choice(&d(Intent::Screen, 0.95, None, Some((10, 0.6)))), Err("low-action"));
        assert_eq!(jev_action_choice(&d(Intent::Screen, 0.6, None, Some((10, 0.99)))), Err("low-intent"), "ação de tela exige as duas certezas");
        assert_eq!(jev_action_choice(&d(Intent::Screen, 0.75, None, Some((10, 0.72)))), Ok(10));
    }

    #[test]
    fn risky_click_asks_before_destructive_buttons() {
        assert!(!risky_click("migration-open", Some("Ver migração para Rust")));
        assert!(!risky_click("settings-tab-advanced", None));
        assert!(risky_click("row-menu-delete", None));
        assert!(risky_click("composer-send", Some("Enviar")));
        assert!(risky_click("menu-item-7", Some("Fechar sessão")));
        assert!(risky_click("voice-stop", Some("Encerrar")));
        assert!(!risky_click("settings-back", Some("Fechar")), "fechar uma tela não é fechar sessão");
    }

    #[test]
    fn session_match_folds_accents() {
        assert_eq!(match_session("sao", &["são-x", "outra"], &[true, true]), SessionMatch::One(0));
        assert_eq!(match_session("AÇÃO", &["acao"], &[true]), SessionMatch::One(0));
    }

    #[test]
    fn session_match_same_name_prefers_active_machine() {
        let names = ["api", "api", "api-docs"];
        assert_eq!(match_session("api", &names, &[false, true, true]), SessionMatch::One(1));
        assert_eq!(match_session("api", &names, &[false, false, true]), SessionMatch::Many(vec![0, 1]));
        assert_eq!(match_session("api", &names, &[true, true, true]), SessionMatch::Many(vec![0, 1]));
    }

    #[test]
    fn question_text_is_marked_and_bounded() {
        let text = question_text("Qual banco vocês usam?");
        assert!(text.starts_with("[Pergunta da conversa de voz] Qual banco vocês usam?"));
        assert!(text.contains("não execute"));
    }

    #[test]
    fn ask_session_times_out() {
        let now = std::time::Instant::now();
        assert!(question_expired(now - ASK_TIMEOUT - Duration::from_secs(1), now));
        assert!(!question_expired(now, now));
    }

    #[test]
    fn question_answer_follows_the_marked_message() {
        let q = question_text("Qual banco?");
        let events = vec![ev("1", "user_msg", "outro pedido"), ev("2", "assistant_msg", "feito"), ev("3", "user_msg", &q)];
        assert_eq!(question_answer(&events), None, "ainda sem resposta; o turno anterior não conta");
        assert!(question_marked(&events), "marca sem resposta é espera, não outro pedido");
        assert!(!question_marked(&events[..2]));
        let mut events = events;
        events.extend([ev("4", "assistant_msg", "vou ler"), ev("5", "assistant_msg", "Postgres"), ev("6", "user_msg", "depois")]);
        assert_eq!(question_answer(&events), Some(("5".into(), "vou ler\n\nPostgres".into())));
    }

    #[test]
    fn speaker_label_holds_300ms() {
        let t0 = std::time::Instant::now();
        let (s, since) = settled_speaker(Speaker::Idle, t0, Speaker::You, t0 + Duration::from_millis(100));
        assert_eq!(s, Speaker::Idle, "100 ms não troca");
        let (s, _) = settled_speaker(s, since, Speaker::You, t0 + Duration::from_millis(450));
        assert_eq!(s, Speaker::You);
    }

    #[test]
    fn last_reply_is_after_last_user_message() {
        let events = vec![ev("1", "user_msg", "a"), ev("2", "assistant_msg", "velha"), ev("3", "user_msg", "b"),
            ev("4", "assistant_msg", "nova 1"), ev("5", "assistant_msg", "nova 2")];
        assert_eq!(last_reply(&events), Some(("5".into(), "nova 1\n\nnova 2".into())));
        assert_eq!(last_reply(&[ev("1", "user_msg", "só pergunta")]), None);
    }

    #[test]
    fn equal_texts_with_different_ids_are_both_spoken() {
        // Dedup é por id: "Pronto." duas vezes em turnos diferentes fala duas vezes.
        let first = last_reply(&[ev("1", "user_msg", "a"), ev("2", "assistant_msg", "Pronto.")]).unwrap();
        let second = last_reply(&[ev("3", "user_msg", "b"), ev("4", "assistant_msg", "Pronto.")]).unwrap();
        assert_ne!(first.0, second.0);
    }

    #[test]
    fn send_reply_names_session_and_status() {
        let ok = send_reply("demo-session", &Ok(Delivery { ok: true, delivered: true }));
        let text = ok["contentItems"][0]["text"].as_str().unwrap();
        assert!(text.contains("demo-session") && text.contains("sent"));
        let queued = send_reply("demo-session", &Ok(Delivery { ok: true, delivered: false }));
        assert!(queued["contentItems"][0]["text"].as_str().unwrap().contains("queued"));
    }

    #[test]
    fn final_reply_after_intermediate_text_is_a_new_id() {
        // O passo do meio já falado não cobre a resposta final: ela tem outro id e ainda é falada.
        let middle = last_reply(&[ev("1", "user_msg", "a"), ev("2", "assistant_msg", "Vou ler o arquivo…")]).unwrap();
        let end = last_reply(&[ev("1", "user_msg", "a"), ev("2", "assistant_msg", "Vou ler o arquivo…"), ev("3", "assistant_msg", "Pronto, corrigi.")]).unwrap();
        assert_ne!(middle.0, end.0);
        assert!(end.1.ends_with("Pronto, corrigi."));
    }

    #[test]
    fn waiting_text_prefers_question_then_reply_tail() {
        assert_eq!(waiting_text(&["Qual branch?"], Some("texto")), "A sessão está esperando sua resposta: Qual branch?");
        assert_eq!(waiting_text(&[" "], Some("Posso apagar o arquivo?")), "A sessão está esperando sua resposta: Posso apagar o arquivo?");
        let long = format!("{}fim", "x".repeat(900));
        let text = waiting_text(&[], Some(&long));
        assert!(text.ends_with("fim") && text.chars().count() < 660);
        assert!(waiting_text(&[], None).contains("no chat"));
    }

    #[test]
    fn bar_height_moves_in_whole_pixels() {
        assert_eq!(bar_height(0.), 4.);
        assert_eq!(bar_height(1.5), 16.);
        assert_eq!(bar_height(0.40), bar_height(0.41), "ruído pequeno não repinta");
    }

    #[test]
    fn level_gain_scales_like_the_web() {
        assert_eq!(level_gain(0.), 0.);
        assert!((level_gain(0.1) - 0.5).abs() < 1e-6);
        assert_eq!(level_gain(0.5), 1.);
    }

    #[test]
    fn speaker_picks_the_louder_side() {
        assert_eq!(speaker(0.5, 0.2, false), Speaker::You);
        assert_eq!(speaker(0.3, 0.3, false), Speaker::You, "empate fica com você");
        assert_eq!(speaker(0.2, 0.6, false), Speaker::Voice);
        assert_eq!(speaker(0.05, 0.07, false), Speaker::Idle, "abaixo do limiar é silêncio");
        assert_eq!(speaker(0.9, 0.0, true), Speaker::Idle, "mudo não fala");
        assert_eq!(speaker(0.9, 0.5, true), Speaker::Voice);
    }

    #[test]
    fn call_clock_formats_minutes_and_hours() {
        assert_eq!(call_clock(Duration::from_secs(0)), "00:00");
        assert_eq!(call_clock(Duration::from_secs(75)), "01:15");
        assert_eq!(call_clock(Duration::from_secs(3599)), "59:59");
        assert_eq!(call_clock(Duration::from_secs(3600 + 62)), "1:01:02");
    }

    #[test]
    fn equalizer_stays_in_bounds_and_idles_at_min() {
        assert_eq!(equalizer(0., 7, 3., 16.), [3.; 5]);
        let full = equalizer(1., 0, 3., 16.);
        assert!(full.iter().all(|h| (3. ..=16.).contains(h) && h.fract() == 0.));
        assert!(full[2] > full[0], "o perfil sobe no meio");
        assert_eq!(equalizer(0.6, 5, 3., 16.), equalizer(0.6, 5, 3., 16.), "determinístico");
    }

    #[test]
    fn went_idle_only_from_working() {
        assert!(went_idle(true, "idle"));
        assert!(went_idle(true, "awaiting_input"));
        assert!(!went_idle(false, "idle"));
        assert!(!went_idle(true, "working"));
    }
}
