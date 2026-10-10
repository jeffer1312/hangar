//! Voz nativa na barra de cima: a pílula, o painel e a ponte entre a chamada do servidor (`crate::voice`) e a tela. O
//! servidor conduz a conversa; aqui ficam o desenho, os ajustes e as ferramentas de tela.
use super::*;
use serde::{Deserialize, Serialize};
use gpui_kit::component::{select::{Select, SelectEvent, SelectState}, searchable_list::SearchableListItem};
use crate::voice::{Mode, Phase, Voice, VoiceEvent};

#[derive(Clone)]
pub(super) struct VoiceChoice { id: String }

impl SearchableListItem for VoiceChoice {
    type Value = String;
    fn title(&self) -> SharedString { if self.id.is_empty() { tr_shared("codex_voice_default", &[]).into() } else { self.id.clone().into() } }
    fn value(&self) -> &String { &self.id }
}

const DEFAULT_ACCOUNT: &str = "default";

/// Conta Codex do servidor da voz; `id` é o `codex_account` que a voz grava (`default` na padrão).
#[derive(Clone, Debug, PartialEq)]
pub(super) struct CodexAccount { id: String, label: String }

impl SearchableListItem for CodexAccount {
    type Value = String;
    fn title(&self) -> SharedString {
        match (self.id == DEFAULT_ACCOUNT, self.label.is_empty()) {
            (false, _) => self.label.clone().into(),
            (true, true) => tr("voice_account_default").into(),
            (true, false) => tr("voice_account_default_named").replace("{account}", &self.label).into(),
        }
    }
    fn value(&self) -> &String { &self.id }
}

/// Primeiro item é sempre a conta padrão (rótulo da conta ativa quando a lista a traz), depois as outras contas Codex com
/// pasta própria; o rótulo é o da tela de contas (apelido, senão e-mail, senão nome).
fn codex_accounts(list: &Value) -> Vec<CodexAccount> {
    let text = |v: &Value| v.as_str().filter(|t| !t.is_empty()).map(str::to_owned);
    let mut default = CodexAccount { id: DEFAULT_ACCOUNT.into(), label: String::new() };
    let mut others = Vec::new();
    // `codex_account` só existe nas contas com pasta; as de cota avulsa não servem à voz.
    for c in list.as_array().into_iter().flatten().filter(|c| c["tipo"] == "codex") {
        let Some(id) = text(&c["codex_account"]) else { continue };
        let label = text(&c["apelido"]).or_else(|| text(&c["login"]["email"])).or_else(|| text(&c["nome"])).unwrap_or_else(|| id.clone());
        if c["ativa"].as_bool() == Some(true) { default.label = label; } else { others.push(CodexAccount { id, label }); }
    }
    std::iter::once(default).chain(others).collect()
}

/// Modelo do catálogo Codex da conta escolhida (`/api/model-options`, o mesmo do diálogo de criar).
#[derive(Clone, Debug, Deserialize)]
pub(super) struct OrganizerModel { id: String, name: Option<String>, #[serde(default)] efforts: Vec<String>, #[serde(default)] service_tiers: Vec<ServiceTier> }

#[derive(Clone, Debug, Deserialize)]
pub(super) struct ServiceTier { id: String }

const DEFAULT_EFFORT: &str = "low";
const TIER_FAST: &str = "priority";
const TIER_STANDARD: &str = "default";

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

fn default_effort() -> String { DEFAULT_EFFORT.to_owned() }
fn default_account() -> String { DEFAULT_ACCOUNT.to_owned() }

/// Modelo, esforço e velocidade do organizador num modo; `model`/`tier` `None` = o que o config da conta diz.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(super) struct Pair { #[serde(default)] model: Option<String>, #[serde(default = "default_effort")] effort: String, #[serde(default)] tier: Option<String> }

impl Default for Pair { fn default() -> Self { Self { model: None, effort: default_effort(), tier: None } } }

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(super) struct Organizer { #[serde(default)] direct: Pair, #[serde(default)] plan: Pair }

impl Organizer {
    fn get(&self, mode: Mode) -> &Pair { match mode { Mode::Direct => &self.direct, Mode::Plan => &self.plan } }
    fn get_mut(&mut self, mode: Mode) -> &mut Pair { match mode { Mode::Direct => &mut self.direct, Mode::Plan => &mut self.plan } }
}

/// As escolhas da voz que o servidor guarda (`/api/voice/settings`); o `PUT` leva o objeto inteiro.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(super) struct VoiceSettings {
    #[serde(default)] voice: Option<String>,
    #[serde(default = "default_account")] codex_account: String,
    #[serde(default)] organizer: Organizer,
}

/// Posição do modo nos seletores do cartão.
fn slot(mode: Mode) -> usize { match mode { Mode::Direct => 0, Mode::Plan => 1 } }

/// O que o organizador faz agora, como o servidor publica.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub(super) enum CallActivity { #[default] Idle, Thinking, Searching, Working }

#[derive(Clone, Debug, PartialEq)]
pub(super) enum OrganizerAction { Tool(String), Search(String), Command(String) }

/// Uma linha dos bastidores: o que chegou ao organizador e o que ele respondeu.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Backstage { Heard(String), Result(String), Answer(String) }

/// Modelo, esforço e velocidade que o organizador usa de fato.
#[derive(Clone, Debug, Default, Deserialize)]
pub(super) struct Effective { model: Option<String>, effort: Option<String>, tier: Option<String> }

/// Janela de limite: porcentagem usada e quando reseta (epoch em segundos).
type RateWindow = Option<(f64, Option<i64>)>;

fn kind_text(value: &Value) -> Option<(&str, String)> { Some((value["kind"].as_str()?, value["text"].as_str().unwrap_or_default().to_owned())) }

fn parse_action(value: &Value) -> Option<OrganizerAction> {
    match kind_text(value)? {
        ("tool", text) => Some(OrganizerAction::Tool(text)),
        ("search", text) => Some(OrganizerAction::Search(text)),
        ("command", text) => Some(OrganizerAction::Command(text)),
        _ => None,
    }
}

fn parse_backstage(value: &Value) -> Option<Backstage> {
    match kind_text(value)? {
        ("heard", text) => Some(Backstage::Heard(text)),
        ("result", text) => Some(Backstage::Result(text)),
        ("answer", text) => Some(Backstage::Answer(text)),
        _ => None,
    }
}

fn rate_window(value: &Value) -> RateWindow { Some((value["used_percent"].as_f64()?, value["resets_at"].as_i64())) }

/// Clique arriscado em duas chamadas: a primeira arma; só a segunda, do mesmo alvo, noutro turno falado e dentro do prazo,
/// executa. O turno diferente impede o organizador de confirmar sozinho sem ouvir o usuário.
#[derive(Default)]
pub(super) struct ConfirmGate { armed: Option<(String, String, std::time::Instant)> }

const CONFIRM_WINDOW: Duration = Duration::from_secs(60);

impl ConfirmGate {
    /// `true` = executar; `false` = ficou armado (ou rearmado) e falta o sim do usuário.
    fn check(&mut self, target: String, confirmed: bool, turn: &str, now: std::time::Instant) -> bool {
        let ok = confirmed && self.armed.as_ref().is_some_and(|(t, armed_turn, at)|
            *t == target && armed_turn != turn && now.saturating_duration_since(*at) < CONFIRM_WINDOW);
        self.armed = if ok { None } else { Some((target, turn.to_owned(), now)) };
        ok
    }
}

#[derive(Default)]
pub(super) struct VoiceUi {
    /// Trava do servidor conectado: voz ligada e Codex achado lá.
    pub(super) enabled: bool,
    /// Vozes que o realtime aceita, como o servidor diz; vazio no seletor é o padrão do Codex.
    pub(super) voices: Vec<String>,
    /// Escolhas gravadas no servidor; `None` até a primeira leitura boa.
    pub(super) settings: Option<VoiceSettings>,
    /// Aparelho com chamada viva no servidor, lido com a trava: conectar daqui assume a conversa.
    pub(super) busy: Option<String>,
    pub(super) accounts: Vec<CodexAccount>,
    pub(super) account_select: Option<(Entity<SelectState<Vec<CodexAccount>>>, Subscription)>,
    /// Catálogo da conta escolhida: `None` = lendo (ou nunca pedido, com `models_seq` 0).
    pub(super) organizer_models: Option<Result<Vec<OrganizerModel>, String>>,
    pub(super) models_seq: u64,
    /// Um seletor por modo, na ordem de `slot`.
    pub(super) model_select: [Option<(Entity<SelectState<Vec<OrganizerChoice>>>, Subscription)>; 2],
    pub(super) effort_select: [Option<(Entity<SelectState<Vec<OrganizerChoice>>>, Subscription)>; 2],
    pub(super) tier_select: [Option<(Entity<SelectState<Vec<OrganizerChoice>>>, Subscription)>; 2],
    pub(super) voice_select: Option<(Entity<SelectState<Vec<VoiceChoice>>>, Subscription)>,
    pub(super) call: Option<Voice>,
    /// Chave do servidor onde a chamada roda; trocar o servidor ativo não a move.
    pub(super) server: Option<String>,
    /// Sobe a cada chamada nova ou parada: eventos de uma chamada velha não mexem na atual.
    pub(super) generation: u64,
    pub(super) phase: Option<Phase>,
    /// Ganhos 0..1 (entrada, saída), já com o `level_gain`.
    pub(super) levels: (f32, f32),
    /// Quem aparece falando e desde quando a leitura crua concorda com isso: evita o rótulo piscar.
    pub(super) shown: Option<(Speaker, std::time::Instant)>,
    /// O que o organizador faz agora; só aparece quando ninguém está falando.
    pub(super) activity: CallActivity,
    /// Resumo do raciocínio do turno em curso e a ação que o organizador executa.
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
    /// Falha deste aparelho (conexão, áudio, ajustes).
    pub(super) error: Option<String>,
    /// Falha que o servidor publica no retrato da chamada.
    pub(super) call_error: Option<String>,
    pub(super) muted: bool,
    pub(super) open: bool,
    /// Sessão na tela que a chamada conhece: (chave da máquina, nome).
    pub(super) target: Option<(String, String)>,
    /// Sessões acompanhadas, pelo nome.
    pub(super) followed: Vec<String>,
    pub(super) effective: Option<Effective>,
    /// Do mais velho ao mais novo.
    pub(super) backstage: Vec<Backstage>,
    pub(super) backstage_open: bool,
    pub(super) backstage_scroll: ScrollHandle,
    pub(super) mode: Mode,
    /// Arquivo (no servidor da voz) e texto do plano; fica na tela depois da chamada, até a próxima começar.
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
    /// Clique por voz num botão arriscado: armado à espera do sim falado.
    pub(super) click_gate: ConfirmGate,
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

/// Só letras e números em minúsculas, sem acento comum do português: "minha loja" casa com `minha-loja`.
fn squash(text: &str) -> String {
    text.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).map(|c| match c {
        'á' | 'à' | 'â' | 'ã' | 'ä' => 'a', 'é' | 'è' | 'ê' | 'ë' => 'e', 'í' | 'ì' | 'î' | 'ï' => 'i',
        'ó' | 'ò' | 'ô' | 'õ' | 'ö' => 'o', 'ú' | 'ù' | 'û' | 'ü' => 'u', 'ç' => 'c', other => other,
    }).collect()
}

/// Palavras de botão que muda algo difícil de desfazer ou que fala por ele: o clique por voz pede o sim antes.
const RISKY_CLICK: [&str; 64] = ["apagar", "apague", "excluir", "exclua", "remover", "remova", "deletar", "delete", "remove", "descartar", "discard",
    "encerrar", "encerra", "kill", "matar", "sair", "logout", "desconectar", "disconnect", "resetar", "reset", "limpar", "clear", "desinstalar",
    "uninstall", "revogar", "revoke", "parar", "stop", "interromper", "enviar", "envie", "send", "submit", "publicar", "publish", "push", "commit",
    "merge", "confirmar", "confirm", "aplicar", "apply", "instalar", "install", "reiniciar",
    // Respostas a pedido de permissão e o que sobrescreve ou desfaz trabalho.
    "permitir", "allow", "aprovar", "approve", "negar", "deny", "recusar", "cancelar", "cancel", "arquivar", "archive", "salvar", "save",
    "sobrescrever", "overwrite", "reverter", "revert", "restore"];

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

/// Seção falada → a das Configurações: igual vence pedaço; nenhuma ou mais de uma não é palpite.
fn pick_section(spoken: &str, keys: &[&str]) -> Option<usize> {
    let want = squash(spoken);
    if want.is_empty() { return None; }
    let keys: Vec<String> = keys.iter().map(|k| squash(k)).collect();
    if let Some(at) = keys.iter().position(|k| *k == want) { return Some(at); }
    let hits: Vec<usize> = (0..keys.len()).filter(|&i| keys[i].contains(&want) || want.contains(keys[i].as_str())).collect();
    match hits[..] { [one] => Some(one), _ => None }
}

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

/// Texto do código de falha que o servidor ou este aparelho dá; `None` para os que só o servidor sabe descrever.
fn failure_text(code: &str) -> Option<String> {
    Some(match code {
        "microphone" => tr_shared("composer_sem_acesso_mic", &[]),
        "speaker" => tr("voice_speaker"),
        "app_server" => tr("voice_app_server"),
        "realtime" | "failed" => tr_shared("codex_voice_failed", &[]),
        "network" => tr("voice_network"),
        "timeout" => tr_shared("codex_voice_timeout", &[]),
        "organizer" => tr("voice_organizer"),
        "model_switch" => tr("voice_model_switch_failed"),
        "own_folder" => tr("voice_own_folder_failed"),
        "audio_stopped" => tr("voice_audio_stopped"),
        "closed" => tr("voice_server_closed"),
        "disabled" => tr("voice_gate_off"),
        "codex_missing" => tr("voice_codex_missing"),
        "account_missing" => tr("voice_account_missing"),
        "connection_lost" => tr("voice_connection_lost"),
        "taken" => tr("voice_taken"),
        "refused" | "bad_hello" => tr("voice_refused"),
        _ => return None,
    })
}

/// Nome curto para a pílula.
fn short(name: &str) -> String {
    if name.chars().count() <= 18 { return name.to_owned(); }
    format!("{}…", name.chars().take(17).collect::<String>())
}

impl Hangar {
    /// A conexão do servidor da voz: o da chamada viva, senão o ativo.
    fn voice_api(&self) -> Option<Api> {
        match &self.voice.server { Some(key) if self.voice.call.is_some() => self.machine_api(key), _ => self.api.clone() }
    }

    /// Trava, escolhas e contas do servidor conectado; a voz roda nele.
    pub(super) fn refresh_voice_gate(&mut self, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else {
            self.voice.enabled = false;
            // Sem a pílula, a chamada ficaria com o microfone aberto e nenhum controle na tela.
            self.stop_voice(cx);
            return;
        };
        let (server, connection, tx) = (self.active_key(), self.connection, self.tx.clone());
        self.runtime.spawn(async move {
            // Leitura que falhou é `None` = sem mudança; só uma resposta válida liga ou desliga.
            let gate = api.server_read(&["voice", "settings"], &[], 8).await;
            if let Err(error) = &gate { crate::voice::log(format!("gate read failed status={:?}", error.status)); }
            let accounts = api.server_read(&["credenciais"], &[], 15).await.ok().map(|v| codex_accounts(&v));
            let _ = tx.send(Envelope { connection, selection: None, payload: Payload::VoiceGate(server, gate.ok(), accounts) }).await;
        });
    }

    pub(super) fn receive_voice_gate(&mut self, server: String, body: Option<Value>, accounts: Option<Vec<CodexAccount>>, window: &mut Window,
        cx: &mut Context<Self>) {
        if server != self.active_key() { return; }
        let before = (self.voice.settings.clone(), self.voice.accounts.clone(), self.voice.voices.clone());
        if let Some(body) = &body {
            let codex = body["codex"] == true;
            self.voice.enabled = body["enabled"] == true && codex;
            self.voice.voices = body["voices"].as_array().into_iter().flatten().filter_map(|v| v.as_str().map(str::to_owned)).collect();
            self.voice.busy = (body["call"]["active"] == true).then(|| body["call"]["client"].as_str().unwrap_or_default().to_owned());
            // Escolha feita aqui no meio da chamada não volta atrás pela leitura.
            if self.voice.call.is_none() && let Ok(settings) = serde_json::from_value(body["settings"].clone()) { self.voice.settings = Some(settings); }
            // Sem a opção ou sem o Codex a pílula some; a chamada não pode seguir com o microfone aberto.
            if !self.voice.enabled && self.voice.call.is_some() && self.voice.server.as_deref() == Some(server.as_str()) {
                self.voice.error = Some(tr(if codex { "voice_gate_off" } else { "voice_codex_missing" }));
                self.stop_voice(cx);
            }
        }
        // Lista que não veio fica como estava: leitura falha não é "sem contas".
        if let Some(accounts) = accounts && self.voice.call.is_none() { self.voice.accounts = accounts; }
        if before != (self.voice.settings.clone(), self.voice.accounts.clone(), self.voice.voices.clone()) {
            if self.voice.open { self.build_voice_picks(window, cx); } else { self.drop_voice_picks(); }
        }
        cx.notify();
    }

    /// Posição da conta escolhida em `accounts`; 0 (padrão) quando não há escolha ou ela sumiu.
    fn account_index(&self) -> usize {
        let chosen = self.voice.settings.as_ref().map(|s| s.codex_account.as_str());
        self.voice.accounts.iter().position(|a| Some(a.id.as_str()) == chosen).unwrap_or(0)
    }

    pub(super) fn start_voice(&mut self, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { self.voice.error = Some(tr("connection_failed")); cx.notify(); return };
        if self.dictation.recording() { self.voice.error = Some(tr("voice_dictation_busy")); cx.notify(); return; }
        let (events_tx, events) = async_channel::unbounded();
        let screen = self.selected.as_ref().map(|s| (self.open_server(), s.name.clone()));
        let actions = Value::Array(HANGAR_ACTIONS.iter().map(|a| json!({"id": a.id, "label": a.label, "description": a.description})).collect());
        self.voice.generation += 1;
        self.voice.call = Some(Voice::start(self.runtime.handle(), &api, self.active_token.clone(), screen.clone(), actions, events_tx));
        (self.voice.server, self.voice.target) = (Some(self.active_key()), screen);
        self.voice.click_gate = ConfirmGate::default();
        self.voice.activity = CallActivity::Idle;
        (self.voice.thought, self.voice.action) = (String::new(), None);
        self.voice.shown = None;
        (self.voice.effective, self.voice.backstage, self.voice.followed) = (None, Vec::new(), Vec::new());
        (self.voice.error, self.voice.call_error) = (None, None);
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

    /// Volta o cartão ao estado sem chamada; o que o usuário ainda lê (plano, bastidores, contexto) fica.
    fn reset_call_view(&mut self) {
        self.voice.server = None;
        self.voice.phase = None;
        self.voice.mode = Mode::Direct;
        self.voice.draft = None;
        self.voice.levels = (0., 0.);
        (self.voice.live_since, self.voice.ticker) = (None, None);
        self.voice.activity = CallActivity::Idle;
        (self.voice.thought, self.voice.action) = (String::new(), None);
        self.voice.shown = None;
    }

    pub(super) fn stop_voice(&mut self, cx: &mut Context<Self>) {
        if let Some(mut call) = self.voice.call.take() { call.stop(); }
        self.voice.generation += 1; // eventos atrasados da chamada parada não mexem na próxima
        self.reset_call_view();
        cx.notify();
    }

    /// Um relógio só por chamada; acorda na virada do segundo do cronômetro para não pular número.
    fn start_call_clock(&mut self, cx: &mut Context<Self>) {
        let Some(since) = self.voice.live_since else { return };
        if self.voice.ticker.is_some() { return; }
        self.voice.ticker = Some(cx.spawn(async move |this, cx| loop {
            let into = since.elapsed().subsec_millis() as u64;
            cx.background_executor().timer(Duration::from_millis(1000 - into)).await;
            if this.update(cx, |_, cx| cx.notify()).is_err() { break; }
        }));
    }

    /// O selo só muda quando o retrato do servidor confirma o modo.
    fn set_voice_mode(&mut self, mode: Mode, cx: &mut Context<Self>) {
        if self.voice.mode == mode { return; }
        if let Some(call) = &self.voice.call { call.set_mode(mode); }
        cx.notify();
    }

    fn voice_reply(&self, call: u64, ok: bool, text: impl Into<String>) {
        if let Some(voice) = &self.voice.call { voice.reply(call, ok, text.into()); }
    }

    /// A sessão na tela mudou: a chamada passa a saber qual é.
    pub(super) fn voice_session_opened(&mut self) {
        if self.voice.call.is_none() { return; }
        let screen = self.selected.as_ref().map(|s| (self.open_server(), s.name.clone()));
        if screen == self.voice.target { return; }
        if let Some(voice) = &self.voice.call { voice.screen(screen.clone()); }
        self.voice.target = screen;
    }

    /// Máquina da lista que o servidor da voz chama de `server` (id do peer): o host do `base_url` ou o rótulo igual ao id.
    fn voice_machine(&self, server: &str, base_url: Option<&str>, cx: &App) -> Option<String> {
        let host = base_url.and_then(crate::voice::host_of);
        std::iter::once(self.active_key()).chain(self.remote.keys().cloned()).filter(|k| !k.is_empty())
            .find(|key| (host.is_some() && crate::voice::host_of(key) == host) || self.machine_label(key, cx).eq_ignore_ascii_case(server))
    }

    /// `switch_session` do servidor: ele já escolheu a sessão; aqui só abre. `server` vazio é a máquina da voz.
    fn voice_switch(&mut self, call: u64, args: &Value, window: &mut Window, cx: &mut Context<Self>) {
        let Some(name) = args["name"].as_str().filter(|n| !n.is_empty()).map(str::to_owned) else {
            self.voice_reply(call, false, "Sem nome de sessão.");
            return;
        };
        let server = args["server"].as_str().unwrap_or_default();
        let key = if server.is_empty() { self.voice.server.clone() } else { self.voice_machine(server, args["base_url"].as_str(), cx) };
        let Some(key) = key else {
            crate::voice::log("switch_session machine unknown");
            self.voice_reply(call, false, "A máquina dessa sessão não está na lista deste aparelho.");
            return;
        };
        if let Some(session) = self.sessions_of(&key).iter().find(|s| s.name == name).cloned() {
            let ok = self.select_on(&key, session, window, cx);
            crate::voice::log(format!("switch_session ok={ok}"));
            self.voice_reply(call, ok, if ok { String::new() } else { self.machine_error(&key) });
            return;
        }
        // Sessão que a voz acabou de criar ainda fora da lista: lida da máquina dela.
        let Some(api) = self.machine_api(&key) else { let reason = self.machine_error(&key); self.voice_reply(call, false, reason); return };
        let generation = self.voice.generation;
        let task = self.runtime.spawn(async move { api.sessions().await });
        cx.spawn_in(window, async move |this, cx| {
            let found = task.await.ok().and_then(Result::ok).and_then(|list| list.into_iter().find(|s| s.name == name));
            let _ = this.update_in(cx, |this, window, cx| {
                if generation != this.voice.generation { return; }
                let ok = found.is_some_and(|session| this.select_on(&key, session, window, cx));
                crate::voice::log(format!("switch_session fetched ok={ok}"));
                this.voice_reply(call, ok, if ok { "" } else { "A sessão não está na lista da máquina dela." });
            });
        }).detach();
    }

    /// Ferramenta de tela pedida pelo servidor; a resposta volta pela chamada.
    fn voice_tool(&mut self, call: u64, name: &str, args: Value, window: &mut Window, cx: &mut Context<Self>) {
        match name {
            "switch_session" => self.voice_switch(call, &args, window, cx),
            "hangar_actions" => {
                let sections: Vec<&str> = settings::Page::sections().map(settings::Page::key).collect();
                self.voice_reply(call, true, hangar_actions_text(&sections));
            }
            // Recusa de ação da tela é recado para o organizador, que conta ao usuário pela voz; no cartão ficaria como erro.
            "hangar_action" => {
                let result = self.run_hangar_action(args["id"].as_str().unwrap_or_default(), args["arg"].as_str(), window, cx);
                crate::voice::log(format!("hangar_action ok={}", result.is_ok()));
                match result { Ok(text) => self.voice_reply(call, true, text), Err(text) => self.voice_reply(call, false, text) }
            }
            "read_screen" => self.voice_read_screen(call, args["area"].as_str().map(str::to_owned), true, window, cx),
            "click_screen" => self.voice_click(call, args["id"].as_str().unwrap_or_default(), args["confirmed"].as_bool().unwrap_or(false),
                args["turn"].as_str().unwrap_or_default(), window, cx),
            other => {
                crate::voice::log(format!("tool unknown {other}"));
                self.voice_reply(call, false, format!("Este aparelho não atende {other}."));
            }
        }
    }

    /// Uma ação de `HANGAR_ACTIONS` pelo mesmo método do botão. Nenhuma troca a sessão ativa; `Err` é o motivo real.
    pub(super) fn run_hangar_action(&mut self, id: &str, arg: Option<&str>, window: &mut Window, cx: &mut Context<Self>) -> Result<String, String> {
        use crate::appearance::SideTab;
        if self.connection_dialog { return Err("A janela de conexão está aberta; feche-a antes.".into()); }
        let side_tab = |tab| match tab { SideTab::Files => "Arquivos", SideTab::Activity => "Atividade", SideTab::Git => "Git", _ => "Contexto" };
        match id {
            "topbar-settings" => {
                let page = match arg.filter(|a| !a.trim().is_empty()) {
                    None => settings::Page::Appearance,
                    Some(spoken) => {
                        let sections: Vec<settings::Page> = settings::Page::sections().collect();
                        let keys: Vec<&str> = sections.iter().map(|p| p.key()).collect();
                        match pick_section(spoken, &keys) {
                            Some(i) => sections[i],
                            None => return Err(format!("Seção desconhecida: {spoken}. Seções: {}.", keys.join(", "))),
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
    fn voice_read_screen(&mut self, call: u64, area: Option<String>, retry: bool, window: &mut Window, cx: &mut Context<Self>) {
        match read_screen(window, area.as_deref()) {
            Ok((root, text)) => {
                crate::voice::log(format!("read_screen area={} bytes={}", root.as_deref().unwrap_or("window"), text.len()));
                self.voice_reply(call, true, text);
            }
            Err(_) if retry => {
                cx.spawn_in(window, async move |this, cx| {
                    cx.background_executor().timer(SCREEN_RETRY).await;
                    let _ = this.update_in(cx, |this, window, cx| this.voice_read_screen(call, area, false, window, cx));
                }).detach();
            }
            Err(text) => {
                crate::voice::log(format!("read_screen failed area={}", clip(area.as_deref().unwrap_or("-"), 64)));
                self.voice_reply(call, false, text);
            }
        }
    }

    /// Clique pela árvore de acessibilidade, pelo mesmo caminho de um leitor de tela: só o elemento que está de fato sob o
    /// ponto recebe. Botão arriscado arma e pede o sim; o clique sai na chamada seguinte, noutro turno falado.
    fn voice_click(&mut self, call: u64, id: &str, confirmed: bool, turn: &str, window: &mut Window, cx: &mut Context<Self>) {
        let reason = |code: &str| match code {
            "not-found" | "no-tree" => format!("O id {id} não está na tela agora; leia a tela de novo com read_screen."),
            "ambiguous" => format!("Mais de um elemento tem o id {id}; leia uma área menor e use outro id."),
            "disabled" => format!("O elemento {id} está desabilitado."),
            "covered" => format!("O elemento {id} está coberto por outro (diálogo, menu) ou fora da área visível."),
            other => format!("Não cliquei em {id}: {other}."),
        };
        let label = match window.a11y_target(id) {
            Ok((_, label)) => label,
            Err(code) => { self.voice_reply(call, false, reason(code)); return; }
        };
        let name = label.clone().unwrap_or_else(|| id.to_owned());
        if risky_click(id, label.as_deref()) && !self.voice.click_gate.check(id.to_owned(), confirmed, turn, std::time::Instant::now()) {
            crate::voice::log("click_screen armed");
            self.voice_reply(call, true, format!("Nada foi clicado. «{name}» muda algo difícil de desfazer: confirme com o usuário e, só depois \
                do sim explícito, chame click_screen de novo com confirmed true."));
            return;
        }
        let result = window.a11y_click(id, cx);
        crate::voice::log(format!("click_screen ok={}", result.is_ok()));
        match result {
            Ok(()) => self.voice_reply(call, true, format!("Cliquei em «{name}». Para ver o resultado, leia a tela de novo.")),
            Err(code) => self.voice_reply(call, false, reason(code)),
        }
        cx.notify();
    }

    /// Retrato publicado pelo servidor. A fase fica com o áudio deste aparelho: o servidor pode dizer "ao vivo" antes de o
    /// áudio daqui conectar.
    fn voice_state(&mut self, state: &Value) {
        let text = |v: &Value| v.as_str().map(str::to_owned);
        self.voice.mode = if state["mode"] == "plan" { Mode::Plan } else { Mode::Direct };
        self.voice.activity = match state["activity"].as_str() {
            Some("thinking") => CallActivity::Thinking,
            Some("searching") => CallActivity::Searching,
            Some("working") => CallActivity::Working,
            _ => CallActivity::Idle,
        };
        self.voice.draft = text(&state["draft"]);
        self.voice.call_error = state["error"]["code"].as_str()
            .map(|code| failure_text(code).or_else(|| text(&state["error"]["text"])).unwrap_or_else(|| tr_shared("codex_voice_failed", &[])));
        self.voice.plan = state["plan"]["path"].as_str().map(|path| (path.into(), text(&state["plan"]["markdown"]).unwrap_or_default()));
        self.voice.effective = serde_json::from_value(state["effective"].clone()).ok().flatten();
        self.voice.context = state["context"]["used"].as_u64().map(|used| (used, state["context"]["window"].as_u64()));
        (self.voice.five_hour, self.voice.seven_day) = (rate_window(&state["limits"]["five_hour"]), rate_window(&state["limits"]["seven_day"]));
        self.voice.backstage = state["backstage"].as_array().into_iter().flatten().filter_map(parse_backstage).collect();
        self.voice.thought = text(&state["thought"]).unwrap_or_default();
        self.voice.action = parse_action(&state["action"]);
        self.voice.followed = state["followed"].as_array().into_iter().flatten().filter_map(|f| text(&f["name"])).collect();
    }

    pub(super) fn receive_voice(&mut self, generation: u64, event: VoiceEvent, window: &mut Window, cx: &mut Context<Self>) {
        if generation != self.voice.generation { return; }
        match event {
            VoiceEvent::Phase(Phase::Closed) => { self.voice.call = None; self.reset_call_view(); }
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
            VoiceEvent::State(state) => self.voice_state(&state),
            VoiceEvent::Tool { call, name, args } => self.voice_tool(call, &name, args, window, cx),
            VoiceEvent::Taken => { self.voice.error = Some(tr("voice_taken")); self.voice.phase = None; }
            VoiceEvent::Failed(code) => {
                self.voice.error = Some(failure_text(&code).unwrap_or_else(|| tr_shared("codex_voice_failed", &[])));
                self.voice.open = true;
            }
        }
        cx.notify();
    }

    fn toggle_voice_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let open = !self.voice.open;
        self.close_popups();
        self.voice.open = open;
        if open {
            // Outro aparelho pode ter mudado as escolhas: o cartão relê o servidor ao abrir.
            self.refresh_voice_gate(cx);
            if self.voice.voice_select.is_none() { self.build_voice_picks(window, cx); }
        }
        cx.notify();
    }

    fn drop_voice_picks(&mut self) {
        (self.voice.voice_select, self.voice.account_select) = (None, None);
        (self.voice.model_select, self.voice.effort_select, self.voice.tier_select) = Default::default();
        (self.voice.organizer_models, self.voice.models_seq) = (None, self.voice.models_seq + 1);
    }

    /// Seletores de voz e conta a partir das escolhas lidas; o catálogo de modelos é da conta.
    fn build_voice_picks(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.drop_voice_picks();
        let Some(settings) = &self.voice.settings else { return };
        let items: Vec<VoiceChoice> = std::iter::once(String::new()).chain(self.voice.voices.iter().cloned()).map(|id| VoiceChoice { id }).collect();
        let at = items.iter().position(|c| Some(&c.id) == settings.voice.as_ref()).unwrap_or(0);
        let picker = cx.new(|cx| SelectState::new(items, Some(gpui_kit::component::IndexPath::new(at)), window, cx));
        let sub = cx.subscribe_in(&picker, window, |this: &mut Hangar, _, event: &SelectEvent<Vec<VoiceChoice>>, _, cx| {
            let SelectEvent::Confirm(Some(id)) = event else { return };
            let Some(settings) = this.voice.settings.as_mut() else { return };
            settings.voice = (!id.is_empty()).then(|| id.clone());
            this.save_voice_settings(cx);
        });
        self.voice.voice_select = Some((picker, sub));
        if self.voice.accounts.len() > 1 {
            let (items, at) = (self.voice.accounts.clone(), self.account_index());
            let picker = cx.new(|cx| SelectState::new(items, Some(gpui_kit::component::IndexPath::new(at)), window, cx));
            let sub = cx.subscribe_in(&picker, window, |this: &mut Hangar, _, event: &SelectEvent<Vec<CodexAccount>>, _, cx| {
                let SelectEvent::Confirm(Some(id)) = event else { return };
                let Some(settings) = this.voice.settings.as_mut() else { return };
                settings.codex_account = id.clone();
                this.save_voice_settings(cx);
                // O catálogo é por conta.
                this.load_organizer_models(cx);
            });
            self.voice.account_select = Some((picker, sub));
        }
        self.load_organizer_models(cx);
    }

    fn load_organizer_models(&mut self, cx: &mut Context<Self>) {
        self.voice.models_seq += 1;
        (self.voice.organizer_models, self.voice.model_select, self.voice.effort_select) = (None, Default::default(), Default::default());
        self.voice.tier_select = Default::default();
        let Some(api) = self.voice_api() else { self.voice.organizer_models = Some(Err(String::new())); cx.notify(); return };
        let account = self.voice.settings.as_ref().map_or_else(default_account, |s| s.codex_account.clone());
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

    fn organizer_pair(&mut self, mode: Mode) -> Option<&mut Pair> { self.voice.settings.as_mut().map(|s| s.organizer.get_mut(mode)) }

    /// Grava as escolhas no servidor da voz; com a chamada no ar, o par do modo atual troca já no próximo turno.
    fn save_voice_settings(&mut self, cx: &mut Context<Self>) {
        cx.notify();
        let (Some(api), Some(settings)) = (self.voice_api(), self.voice.settings.as_ref()) else { return };
        let body = serde_json::to_value(settings).unwrap_or_default();
        let task = self.runtime.spawn(async move { api.server_send(reqwest::Method::PUT, &["voice", "settings"], Some(body), 15).await });
        cx.spawn(async move |this, cx| {
            let saved = task.await;
            let _ = this.update(cx, |this, cx| {
                let error = match saved { Ok(Ok(_)) => return, Ok(Err(error)) => Hangar::fetch_failure(&error), Err(_) => String::new() };
                crate::voice::log("settings save failed");
                this.voice.error = Some(format!("{} {error}", tr("voice_not_saved")).trim_end().to_owned());
                cx.notify();
            });
        }).detach();
    }

    fn build_model_pick(&mut self, mode: Mode, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(Ok(models)), Some(settings)) = (&self.voice.organizer_models, &self.voice.settings) else { return };
        let chosen = settings.organizer.get(mode).model.clone();
        let mut items: Vec<OrganizerChoice> = std::iter::once(OrganizerChoice { id: String::new(), label: tr("voice_organizer_default") })
            .chain(models.iter().map(|m| OrganizerChoice { id: m.id.clone(), label: m.name.clone().unwrap_or_else(|| m.id.clone()) })).collect();
        // Gravado que saiu do catálogo continua à vista: a próxima chamada ainda o usa.
        if let Some(model) = chosen.as_ref().filter(|m| !items.iter().any(|i| &i.id == *m)) { items.push(OrganizerChoice { id: model.clone(), label: model.clone() }); }
        let at = items.iter().position(|i| Some(&i.id) == chosen.as_ref()).unwrap_or(0);
        let picker = cx.new(|cx| SelectState::new(items, Some(gpui_kit::component::IndexPath::new(at)), window, cx));
        let sub = cx.subscribe_in(&picker, window, move |this: &mut Hangar, _, event: &SelectEvent<Vec<OrganizerChoice>>, window, cx| {
            let SelectEvent::Confirm(Some(id)) = event else { return };
            let Some(pair) = this.organizer_pair(mode) else { return };
            pair.model = (!id.is_empty()).then(|| id.clone());
            this.build_effort_pick(mode, window, cx);
            this.build_tier_pick(mode, window, cx);
            this.save_voice_settings(cx);
        });
        self.voice.model_select[slot(mode)] = Some((picker, sub));
    }

    /// Refeito a cada troca de modelo: os esforços são do modelo, e o que ele não aceita vira o padrão.
    fn build_effort_pick(&mut self, mode: Mode, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(Ok(models)), Some(settings)) = (&self.voice.organizer_models, &self.voice.settings) else { return };
        let pair = settings.organizer.get(mode);
        let efforts = organizer_efforts(models, pair.model.as_deref());
        let effort = fit_effort(&efforts, Some(&pair.effort));
        let at = efforts.iter().position(|e| *e == effort).unwrap_or(0);
        if let Some(pair) = self.organizer_pair(mode) { pair.effort = effort; }
        let items: Vec<OrganizerChoice> = efforts.into_iter().map(|e| OrganizerChoice { label: e.clone(), id: e }).collect();
        let picker = cx.new(|cx| SelectState::new(items, Some(gpui_kit::component::IndexPath::new(at)), window, cx));
        let sub = cx.subscribe_in(&picker, window, move |this: &mut Hangar, _, event: &SelectEvent<Vec<OrganizerChoice>>, _, cx| {
            let SelectEvent::Confirm(Some(id)) = event else { return };
            let Some(pair) = this.organizer_pair(mode) else { return };
            pair.effort = id.clone();
            this.save_voice_settings(cx);
        });
        self.voice.effort_select[slot(mode)] = Some((picker, sub));
    }

    /// Refeito a cada troca de modelo: Fast só aparece quando o catálogo não diz que o modelo não tem; o Fast gravado
    /// que o modelo novo não aceita volta para a velocidade da conta.
    fn build_tier_pick(&mut self, mode: Mode, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(Ok(models)), Some(settings)) = (&self.voice.organizer_models, &self.voice.settings) else { return };
        let pair = settings.organizer.get(mode);
        let no_fast = fast_known_unavailable(models, pair.model.as_deref());
        let mut chosen = tier_id(pair.tier.as_deref());
        if no_fast && chosen == TIER_FAST {
            chosen.clear();
            if let Some(pair) = self.organizer_pair(mode) { pair.tier = None; }
        }
        let mut items = vec![OrganizerChoice { id: String::new(), label: tr("voice_tier_account") },
            OrganizerChoice { id: TIER_STANDARD.into(), label: tr("voice_tier_standard") }];
        if !no_fast { items.push(OrganizerChoice { id: TIER_FAST.into(), label: tr("ctl_fast") }); }
        let at = items.iter().position(|i| i.id == chosen).unwrap_or(0);
        let picker = cx.new(|cx| SelectState::new(items, Some(gpui_kit::component::IndexPath::new(at)), window, cx));
        let sub = cx.subscribe_in(&picker, window, move |this: &mut Hangar, _, event: &SelectEvent<Vec<OrganizerChoice>>, _, cx| {
            let SelectEvent::Confirm(Some(id)) = event else { return };
            let Some(pair) = this.organizer_pair(mode) else { return };
            pair.tier = (!id.is_empty()).then(|| id.clone());
            this.save_voice_settings(cx);
        });
        self.voice.tier_select[slot(mode)] = Some((picker, sub));
    }

    /// Pílula da barra de cima; `None` sem a voz ligada no servidor conectado (ou sem Codex lá) e sem chamada viva, que
    /// pode estar noutro servidor depois de trocar o ativo.
    pub(super) fn render_voice_pill(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.voice.enabled && self.voice.call.is_none() { return None; }
        let title = tr_shared("codex_voice_title", &[]);
        let failed = self.voice.error.is_some() || self.voice.call_error.is_some();
        let button = Button::new("topbar-voice").ghost().small().h(px(26.)).px(px(8.)).rounded_full().selected(self.voice.open)
            .accessibility_label(title.clone())
            .on_click(cx.listener(|this, _, window, cx| this.toggle_voice_panel(window, cx)));
        let button = if self.voice.call.is_none() {
            button.tooltip(title).child(div().flex().items_center().gap(px(6.))
                .child(chrome::small_icon(IconName::Mic, 14., theme::muted()))
                .when(failed, |el| el.child(div().size(px(6.)).rounded_full().bg(theme::danger()))))
        } else {
            let target = self.voice.target.as_ref().map(|(_, name)| short(name));
            button.child(div().flex().items_center().gap(px(6.)).text_size(px(12.5))
                .child(self.render_equalizer(3., 16., 2.))
                .children(self.call_time().map(|time| div().text_color(theme::text()).child(time)))
                .child(div().text_color(self.voice_state_color()).child(self.voice_status()))
                .when(self.voice.mode == Mode::Plan, |el| el.child(div().flex_shrink_0().px(px(5.)).rounded(px(4.)).border_1().border_color(theme::border())
                    .text_size(px(10.)).text_color(theme::muted()).child(tr("voice_planning"))))
                .children(target.map(|name| div().text_color(theme::faint()).child(name)))
                .when(self.voice.draft.is_some(), |el| el.child(div().size(px(6.)).rounded_full().bg(theme::warning())))
                .when(failed, |el| el.child(div().size(px(6.)).rounded_full().bg(theme::danger())))
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

    /// Uma linha com o que os Ajustes fechados escondem: voz, conta e o par de cada modo.
    fn voice_settings_summary(&self) -> String {
        let Some(settings) = &self.voice.settings else { return String::new() };
        let mut parts = vec![settings.voice.clone().unwrap_or_else(|| tr_shared("codex_voice_default", &[]).to_string())];
        if self.voice.accounts.len() > 1 { parts.extend(self.voice.accounts.get(self.account_index()).map(|a| a.title().to_string())); }
        for (mode, title) in [(Mode::Direct, "voice_mode_direct"), (Mode::Plan, "voice_mode_plan")] {
            let pair = settings.organizer.get(mode);
            let model = pair.model.as_deref().map_or_else(|| tr("voice_organizer_default"), |id| self.model_name(id));
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
        let mut names: Vec<&str> = self.voice.followed.iter().map(String::as_str).collect();
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
        let target = self.voice.target.as_ref().map(|(_, name)| name.clone());
        if live {
            // O estado em destaque, e logo abaixo o que o organizador faz e pensa neste turno.
            let action = self.voice.action.as_ref().map(action_text);
            let thought = thought_tail(&self.voice.thought, 3, 140);
            // Texto solto não vira nó de acessibilidade: o estado inteiro vai no nome do bloco.
            let spoken = [Some(self.voice_status().to_string()), target.clone(), action.clone()]
                .into_iter().flatten().collect::<Vec<_>>().join(" · ");
            header = header.child(div().id("voice-status").role(Role::Status).aria_label(spoken).flex().flex_col().gap(px(8.)).p(px(12.)).rounded(px(10.)).border_1().border_color(theme::border())
                .child(div().flex().items_center().gap(px(12.))
                    .child(self.render_equalizer(4., 28., 4.).gap(px(3.)))
                    .child(div().flex_1().min_w_0().flex().flex_col().gap(px(2.))
                        .child(div().text_base().font_weight(FontWeight::SEMIBOLD).text_color(self.voice_state_color()).child(self.voice_status()))
                        .children(target.map(|name| div().text_xs().text_color(theme::faint()).truncate().child(name))))
                    .children(self.call_time().map(|time| div().flex_shrink_0().text_lg().text_color(theme::text()).child(time))))
                .children(action.map(|text| div().text_sm().text_color(theme::text()).truncate().child(text)))
                .when(!thought.is_empty(), |el| el.child(div().flex().flex_col().gap(px(2.))
                    .children(thought.into_iter().map(|line| div().text_xs().text_color(theme::muted()).truncate().child(line))))));
        } else if let Some(client) = &self.voice.busy {
            body = body.child(div().text_xs().text_color(theme::muted()).whitespace_normal().child(tr("voice_call_elsewhere").replace("{client}", client)));
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
            // O arquivo mora no servidor da voz: abrir só quando ele está neste disco.
            let local = path.exists().then(|| path.clone());
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
                    .children(local.map(|open| Button::new("voice-plan-open").ghost().small().flex_shrink_0().label(tr("voice_plan_open"))
                        .on_click(cx.listener(move |_, _, _, cx| cx.open_with_system(&open))))))
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
        for (id, error) in [("voice-error", &self.voice.error), ("voice-call-error", &self.voice.call_error)] {
            let Some(error) = error else { continue };
            footer = footer.child(div().id(id).role(Role::Alert).aria_label(error.clone()).text_xs().text_color(theme::danger()).whitespace_normal().child(error.clone()));
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
        match (&self.voice.organizer_models, &self.voice.settings) {
            (Some(Ok(models)), Some(settings)) => {
                let no_fast: Vec<String> = [(Mode::Direct, "voice_mode_direct"), (Mode::Plan, "voice_mode_plan")].into_iter()
                    .filter(|(mode, _)| fast_known_unavailable(models, settings.organizer.get(*mode).model.as_deref()))
                    .map(|(_, title)| tr(title)).collect();
                if !no_fast.is_empty() {
                    body = body.child(div().text_xs().text_color(theme::muted()).whitespace_normal()
                        .child(format!("{}: {}", no_fast.join(", "), tr("ctl_fast_unavailable"))));
                }
                body = body.child(div().text_xs().text_color(theme::faint()).whitespace_normal().child(tr("voice_organizer_hint")))
                    .child(div().text_xs().text_color(theme::faint()).whitespace_normal().child(tr("voice_tier_hint")));
            }
            (None, Some(_)) if self.voice.models_seq > 0 => body = body.child(div().text_xs().text_color(theme::muted()).child(tr("voice_models_loading"))),
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
    fn settings_section_is_picked_by_name_never_guessed() {
        let keys = ["general", "appearance", "voice", "jev", "shared_config", "servers"];
        assert_eq!(pick_section("Voice", &keys), Some(2));
        assert_eq!(pick_section("shared config", &keys), Some(4), "separador não conta");
        assert_eq!(pick_section("server", &keys), Some(5), "pedaço único");
        assert_eq!(pick_section("e", &keys), None, "várias casam: não é palpite");
        assert_eq!(pick_section("banana", &keys), None);
        assert_eq!(pick_section(" ", &keys), None);
    }

    #[test]
    fn thought_shows_the_last_lines() {
        assert_eq!(thought_tail("**Lendo a sessão**\nVou trocar de sessão", 3, 140), ["Lendo a sessão", "Vou trocar de sessão"]);
        let tail = thought_tail(&"é".repeat(3000), 1, 10);
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
    fn state_pieces_read_the_server_snapshot() {
        assert_eq!(parse_action(&json!({"kind": "search", "text": "gpui"})), Some(OrganizerAction::Search("gpui".into())));
        assert_eq!(parse_action(&Value::Null), None);
        assert_eq!(parse_backstage(&json!({"kind": "result", "text": "web"})), Some(Backstage::Result("web".into())));
        assert_eq!(rate_window(&json!({"used_percent": 12.5, "resets_at": 1000})), Some((12.5, Some(1000))));
        assert_eq!(rate_window(&Value::Null), None);
    }

    #[test]
    fn every_call_failure_code_has_a_voice_text() {
        for code in ["microphone", "speaker", "app_server", "realtime", "network", "timeout", "organizer", "closed", "audio_stopped", "disabled",
            "codex_missing", "account_missing", "connection_lost", "taken", "refused", "bad_hello", "failed"] {
            let text = failure_text(code).unwrap_or_default();
            assert!(!text.is_empty() && !text.starts_with("voice_") && !text.contains("terminal"), "{code}: {text}");
        }
        assert_eq!(failure_text("send_to_session"), None, "falha de ação: o texto do servidor vale");
    }

    #[test]
    fn settings_keep_the_server_shape() {
        let read: VoiceSettings = serde_json::from_value(json!({"voice": null, "codex_account": "b",
            "organizer": {"direct": {"model": "gpt-x", "effort": "high", "tier": "priority"}, "plan": {"effort": "xhigh"}}})).unwrap();
        assert_eq!(read.organizer.get(Mode::Plan), &Pair { model: None, effort: "xhigh".into(), tier: None });
        let sent = serde_json::to_value(&read).unwrap();
        assert_eq!(sent["organizer"]["direct"], json!({"model": "gpt-x", "effort": "high", "tier": "priority"}), "o PUT leva o objeto inteiro");
        assert_eq!(sent["codex_account"], "b");
        assert_eq!(Pair::default().effort, "low");
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
        assert_eq!(accounts.iter().map(|a| (a.id.as_str(), a.label.as_str())).collect::<Vec<_>>(), [("default", "a@example.com"), ("b", "Second")]);
        let only = json!([{"id": "codex:/h/.codex", "tipo": "codex", "codex_account": "default", "ativa": true, "nome": "d"}]);
        assert_eq!(codex_accounts(&only).len(), 1);
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
    fn risky_click_needs_a_yes_in_another_turn() {
        let (mut gate, now) = (ConfirmGate::default(), std::time::Instant::now());
        assert!(!gate.check("row-menu-delete".into(), true, "t1", now), "sem pedido armado, confirmed sozinho não clica");
        assert!(!gate.check("row-menu-delete".into(), true, "t1", now), "mesmo turno não confirma");
        assert!(gate.check("row-menu-delete".into(), true, "t2", now));
        assert!(!gate.check("row-menu-delete".into(), true, "t3", now), "usado, acabou");
        assert!(!gate.check("row-menu-delete".into(), true, "t4", now + CONFIRM_WINDOW), "fora do prazo");
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
}
