//! O seletor do `/model` do Claude Code com terminal: leitura do pane e o percurso de teclas sobre ele.
//! Porte do `model_picker.py` e do `TerminalInput.set_model_effort`; o golden
//! `backend/tests/fixtures/contract/golden/model_picker.json` prende as teclas e as respostas.
pub mod default_model;

use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;
use serde_json::{Value, json};
use tokio::time::{Instant, sleep};

use crate::terminal_input::{IoFailure, PermissionSequence, TerminalDriver};
use crate::terminal_state::{analyze, lines, trim, whitespace};

/// Fraco → forte, como o `/effort`; modelos menores mostram só uma parte (lida ao vivo).
pub const EFFORT_ORDER: [&str; 6] = ["low", "medium", "high", "xhigh", "max", "ultracode"];
/// Reserva para a linha-alvo rolada para fora da tela, não a verdade (a lista real vem do pane).
const MODEL_NUMBERS: [(&str, i64); 5] = [("default", 1), ("opus", 2), ("fable", 3), ("sonnet", 4), ("haiku", 5)];

const SETTLE: Duration = Duration::from_millis(300);
const OPEN_SETTLE: Duration = Duration::from_millis(700);
const NAV_GAP: Duration = Duration::from_millis(120);
const OPEN_LIMIT: Duration = Duration::from_secs(2);
/// Duas capturas: spinner vivo e marcador de turno concluído se desenham igual; quem decide é a animação.
const SPIN_GAP: Duration = Duration::from_millis(1200);

const MENU: &str = "ha um menu aberto no terminal da sessao";
const WORKING: &str = "a sessao esta trabalhando — espere ela terminar";

struct Patterns { row: Regex, effort: Regex, confirm: Regex, keyword: Regex }
static P: LazyLock<Patterns> = LazyLock::new(|| {
    // As classes do `re` do Python, como no `terminal_state`.
    let r = |s: &str| Regex::new(&s.replace(r"\s", r"[\s\x1c-\x1f]").replace(r"\S", r"[^\s\x1c-\x1f]").replace(r"\w", r"[\p{L}\p{N}_]")).unwrap();
    Patterns {
        row: r(r"^\s*([❯↑↓]?)\s*(\d+)\.\s+(.+?)\s{2,}(.*)$"),
        effort: r(r"^\S\s+(\w+)\s+effort\b"),
        // O `re.I` do Python reúne também as duas formas turcas de i.
        confirm: r(r"(?i)Yes,\s+sw[iIıİ]tch\s+to\s+\w+"),
        keyword: Regex::new(r"\A[a-z0-9]+(\[[a-z0-9]+\])?\z").unwrap(),
    }
});

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelRow { pub number: i64, pub label: String, pub name: String, pub desc: String, pub keyword: String, pub id: String, pub cursor: bool, pub active: bool }

/// Só o bloco do seletor (do último "Select model" até "Esc to cancel"): lista numerada do histórico não vale.
fn region(pane: &str) -> Vec<&str> {
    let all = lines(pane);
    let Some(start) = all.iter().rposition(|l| l.contains("Select model")) else { return vec![] };
    let end = all[start..].iter().position(|l| l.contains("Esc to cancel")).map_or(all.len(), |at| start + at + 1);
    all[start..end].to_vec()
}

pub fn picker_open(pane: &str) -> bool { !region(pane).is_empty() }
/// O rodapé vem depois das linhas de modelo: só com ele a lista está inteira.
pub fn picker_drawn(pane: &str) -> bool { region(pane).iter().any(|l| l.contains("Esc to cancel")) }

pub fn parse_model_rows(pane: &str) -> Vec<ModelRow> {
    region(pane).into_iter().filter_map(|line| {
        let m = P.row.captures(line)?;
        // ponytail: dígito fora do ASCII ou número maior que i64 descarta a linha (o `int()` do Python aceitaria).
        let number = m[2].parse::<i64>().ok()?;
        let label = trim(&m[3]).to_owned();
        let keyword = label.split(whitespace).find(|p| !p.is_empty()).map(str::to_lowercase).unwrap_or_default();
        // O sufixo `[1m]` é a notação do Claude Code para a janela; só o rótulo decide, nunca a descrição.
        let id = if label.to_lowercase().contains("1m context") { format!("{keyword}[1m]") } else { keyword.clone() };
        Some(ModelRow { number, name: trim(&label.replace('✔', "").replace("(recommended)", "")).to_owned(), desc: trim(&m[4]).to_owned(),
            cursor: &m[1] == "❯", active: label.contains('✔'), label, keyword, id })
    }).collect()
}

/// Passos do cursor até o alvo (>0 Down, <0 Up): pelo `id` antes da palavra-chave; o alvo com sufixo
/// fora da tela falha em vez de ir à linha do número, que não identifica a variante.
pub fn model_nav_steps(rows: &[ModelRow], target: &str) -> Result<i64, String> {
    let base = rows.iter().find(|r| r.cursor).or_else(|| rows.iter().find(|r| r.active)).ok_or("cursor row not found in picker")?;
    let number = rows.iter().find(|r| r.id == target).or_else(|| rows.iter().find(|r| r.keyword == target)).map(|r| r.number)
        .or_else(|| MODEL_NUMBERS.iter().find(|(k, _)| *k == target).map(|(_, n)| *n))
        .ok_or_else(|| format!("target model {} not in picker", py_repr(target)))?;
    Ok(number.saturating_sub(base.number))
}

/// Esforço atual em minúsculas; `None` sem marcador ou com "not supported" (Haiku).
pub fn parse_current_effort(pane: &str) -> Option<String> {
    for line in region(pane) {
        let s = trim(line);
        if s.to_lowercase().contains("not supported") { return None; }
        if let Some(m) = P.effort.captures(s) && s.contains("to adjust") { return Some(m[1].to_lowercase()); }
    }
    None
}

/// O "Change effort level?" que a troca de esforço com cache dispara: fica para a pessoa responder.
pub fn effort_confirm_open(pane: &str) -> bool { P.confirm.is_match(pane) }

pub fn parse_result_line(pane: &str) -> Option<String> {
    // A borda `│` é do painel do plugin ao lado da conversa, na mesma linha do pane.
    lines(pane).into_iter().rev().map(|l| trim(trim(l.split('│').next().unwrap_or(l)).trim_start_matches('⎿'))).find(|s| s.starts_with("Set model to") || s.starts_with("Kept model")).map(str::to_owned)
}

/// A validação do Python antes da trava, com o mesmo texto: palavra-chave e esforço em minúsculas sem
/// as bordas. Só espaço conta como pedido (o `if model` do Python) e chega vazio ao seletor.
pub fn normalize(model: Option<&str>, effort: Option<&str>) -> Result<(Option<String>, Option<String>), String> {
    let keyword = |value: Option<&str>| value.filter(|v| !v.is_empty()).map(|v| trim(v).to_lowercase());
    let (model_kw, effort_kw) = (keyword(model), keyword(effort));
    if model_kw.is_none() && effort_kw.is_none() { return Err("must provide model or effort".into()); }
    if model_kw.as_deref().is_some_and(|k| !k.is_empty() && !P.keyword.is_match(k)) {
        return Err(format!("unknown model {}", py_repr(model.unwrap_or_default())));
    }
    if effort_kw.as_deref().is_some_and(|k| !k.is_empty() && !EFFORT_ORDER.contains(&k)) {
        return Err(format!("unknown effort {}", py_repr(effort.unwrap_or_default())));
    }
    Ok((model_kw, effort_kw))
}

/// O `repr()` de uma str do Python, que as mensagens de recusa citam.
fn py_repr(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') { '"' } else { '\'' };
    let mut out = String::from(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => { out.push('\\'); out.push(c); }
            c if printable(c) => out.push(c),
            c if (c as u32) < 0x100 => out.push_str(&format!("\\x{:02x}", c as u32)),
            c if (c as u32) < 0x10000 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push_str(&format!("\\U{:08x}", c as u32)),
        }
    }
    out.push(quote);
    out
}
// ponytail: `str.isprintable` sem a tabela Unicode; ponto de código não atribuído (Cn) passa como imprimível.
fn printable(c: char) -> bool {
    c == ' ' || !(c.is_control() || whitespace(c) || matches!(c as u32, 0xad | 0x600..=0x605 | 0x61c | 0x6dd | 0x70f | 0x180e
        | 0x200b..=0x200f | 0x202a..=0x202e | 0x2060..=0x2064 | 0x2066..=0x206f | 0xfeff | 0xfff9..=0xfffb | 0xe000..=0xf8ff | 0xf0000..=0x10ffff))
}

/// Recusa do seletor: o texto do Python (`PickerError.detail`) e, quando a falha veio do terminal, o código.
/// `uncertain`: uma tecla pode ter chegado sem a prova do efeito.
#[derive(Debug)]
pub struct Refusal { pub detail: String, pub code: Option<&'static str>, pub uncertain: bool, pub typed: bool }

fn refused(detail: impl Into<String>) -> Refusal { Refusal { detail: detail.into(), code: None, uncertain: false, typed: false } }
fn coded(code: &'static str, detail: impl Into<String>) -> Refusal { Refusal { detail: detail.into(), code: Some(code), uncertain: false, typed: false } }
fn rank(level: &str) -> Option<usize> { EFFORT_ORDER.iter().position(|l| *l == level) }
fn io(failure: IoFailure, typed: bool) -> Refusal {
    let detail = match failure.code {
        "terminal_panel_active" | "terminal_panel_open" => "Terminal aberto nesta sessao. Feche o painel pra responder por aqui.",
        _ => "não consegui dirigir o terminal da sessão",
    };
    Refusal { detail: detail.into(), code: Some(failure.code), uncertain: typed || failure.may_have_written, typed: typed || failure.may_have_written }
}

struct Picker<'a> { sequence: PermissionSequence<'a>, typed: bool }
impl Picker<'_> {
    async fn screen(&self) -> Result<String, Refusal> { self.sequence.screen().await.map_err(|f| io(f, self.typed)) }
    /// Pane ilegível na conferência de antes não trava a troca (como o Python); guarda recusada, sim.
    async fn readable(&self) -> Result<Option<String>, Refusal> {
        match self.sequence.screen().await {
            Ok(pane) => Ok(Some(pane)),
            Err(failure) if matches!(failure.code, "capture_failed" | "capture_utf8") => Ok(None),
            Err(failure) => Err(io(failure, self.typed)),
        }
    }
    async fn press(&mut self, key: &str) -> Result<(), Refusal> {
        let sent = self.sequence.press(key).await.map_err(|f| io(f, self.typed));
        self.typed = true;
        sent
    }
    async fn abort(&self, detail: impl Into<String>) -> Refusal { let _ = self.sequence.escape().await; Refusal { typed: self.typed, ..refused(detail) } }
    async fn abort_coded(&self, code: &'static str, detail: impl Into<String>) -> Refusal { let _ = self.sequence.escape().await; Refusal { typed: self.typed, ..coded(code, detail) } }

    /// Com o Claude trabalhando o `/model` viraria mensagem na fila; com menu aberto, navegaria o menu errado.
    async fn require_drivable(&self) -> Result<(), Refusal> {
        let Some(pane) = self.readable().await? else { return Ok(()) };
        let first = analyze(&pane);
        if first.overlay { return Err(refused(MENU)); }
        let Some(spinner) = first.spinner else { return Ok(()) };
        sleep(SPIN_GAP).await;
        let Some(after) = self.readable().await? else { return Ok(()) };
        let second = analyze(&after);
        if second.overlay { return Err(refused(MENU)); }
        if second.spinner.as_deref() != Some(spinner.as_str()) { return Err(refused(WORKING)); }
        Ok(())
    }

    /// O seletor inteiro na tela. O 2º Enter só sem seletor nenhum: sobre um aberto ele confirmaria a
    /// linha do cursor como padrão.
    async fn open(&mut self) -> Result<String, Refusal> {
        self.require_drivable().await?;
        let typed = self.sequence.type_text("/model").await.map_err(|f| io(f, false));
        self.typed = true;
        typed?;
        sleep(SETTLE).await;
        self.press("Enter").await?;
        if let Some(pane) = self.wait_drawn().await? { return Ok(pane); }
        self.press("Enter").await?;
        if let Some(pane) = self.wait_drawn().await? { return Ok(pane); }
        Err(self.abort_coded("erro_seletor_modelo_nao_abriu", "o seletor do /model não abriu no terminal da sessão").await)
    }

    /// Título sem rodapé no prazo ainda é seletor aberto: devolve assim mesmo, nunca outro Enter.
    async fn wait_drawn(&self) -> Result<Option<String>, Refusal> {
        let deadline = Instant::now() + OPEN_LIMIT;
        loop {
            sleep(SETTLE).await;
            let pane = self.screen().await?;
            if picker_drawn(&pane) { return Ok(Some(pane)); }
            if Instant::now() >= deadline { return Ok(picker_open(&pane).then_some(pane)); }
        }
    }
}

/// Modelo e esforço pelo seletor do `/model`, já validados por `normalize`. `default_scope`: Enter (vira o
/// padrão da conta); senão `s` (só esta sessão). Só setas no modelo: tecla de número confirma como padrão.
pub async fn drive(driver: &TerminalDriver, model: Option<&str>, effort: Option<&str>, default_scope: bool) -> Result<Value, Refusal> {
    let mut picker = Picker { sequence: driver.permission_sequence().await, typed: false };
    let mut pane = picker.open().await?;
    if let Some(model) = model.filter(|m| !m.is_empty()) {
        let steps = match model_nav_steps(&parse_model_rows(&pane), model) {
            Ok(steps) => steps,
            Err(error) => return Err(picker.abort(format!("cannot navigate model picker: {error}")).await),
        };
        let key = if steps > 0 { "Down" } else { "Up" };
        for _ in 0..steps.unsigned_abs() {
            picker.press(key).await?;
            sleep(NAV_GAP).await;
        }
        sleep(SETTLE).await;
        pane = picker.screen().await?;
    }
    // O slider não dá a volta (Right no Max fica Max) e o conjunto depende do modelo: anda na direção do
    // alvo, lê o marcador a cada passo e recusa quando ele para ou pula o alvo, sem confirmar outro nível.
    if let Some(target) = effort.filter(|e| !e.is_empty())
        && let Some(start) = parse_current_effort(&pane).filter(|c| c != target)
    {
        let unavailable = || format!("o esforço {target} não está disponível para este modelo no seletor do /model");
        let (Some(from), Some(to)) = (rank(&start), rank(target)) else { return Err(picker.abort_coded("erro_esforco_indisponivel", unavailable()).await) };
        let key = if to > from { "Right" } else { "Left" };
        let mut previous = from;
        loop {
            picker.press(key).await?;
            sleep(SETTLE).await;
            pane = picker.screen().await?;
            let mut current = parse_current_effort(&pane);
            // Sem marcador ou ainda no anterior: o redesenho pode só ter atrasado.
            if current.as_deref().and_then(rank).is_none_or(|now| now == previous) {
                sleep(SETTLE).await;
                pane = picker.screen().await?;
                current = parse_current_effort(&pane);
            }
            let Some(now) = current.as_deref().and_then(rank) else {
                return Err(picker.abort_coded("erro_esforco_ilegivel", "não consegui ler o esforço no seletor do /model").await);
            };
            if now == to { break; }
            let toward = if to > from { now > previous && now < to } else { now < previous && now > to };
            if !toward { return Err(picker.abort_coded("erro_esforco_indisponivel", unavailable()).await); }
            previous = now;
        }
    }
    let scope = if default_scope { "default" } else { "session" };
    picker.press(if default_scope { "Enter" } else { "s" }).await?;
    sleep(OPEN_SETTLE).await;
    pane = picker.screen().await?;
    // O esforço só vale quando a pessoa responder o diálogo; nada de confirmar por ela.
    if effort_confirm_open(&pane) { return Ok(json!({"ok": true, "scope": scope, "pending_confirm": effort, "result": null})); }
    if picker_open(&pane) { return Err(picker.abort("model picker did not close after confirm").await); }
    let result = parse_result_line(&pane);
    if let Some(line) = &result {
        let lower = line.to_lowercase();
        if !default_scope && !lower.contains("session only") && lower.contains("default") {
            return Err(refused(format!("expected session-only switch, got: {line}")));
        }
    }
    Ok(json!({"ok": true, "scope": scope, "result": result}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const OPUS: &str = include_str!("../../../backend/tests/fixtures/pane_model_picker_opus.txt");
    const HAIKU: &str = include_str!("../../../backend/tests/fixtures/pane_model_picker_haiku.txt");
    const FABLE: &str = include_str!("../../../backend/tests/fixtures/pane_model_picker_fable.txt");
    const DUAS_OPUS: &str = "   Select model\n   Switch between Claude models.\n\n     1. Default (recommended)  Sonnet 5 · Efficient for routine tasks\n     2. Sonnet                 Sonnet 5 · Efficient for routine tasks\n     3. Fable                  Fable 5 · Most capable\n   ❯ 4. Opus                   Opus 5 · Best for everyday, complex tasks\n     5. Haiku                  Haiku 4.5 · Fastest for quick answers\n     6. Opus (1M context) ✔    Opus 5 with 1M context · Best for everyday, complex tasks\n\n   ◉ High effort ←/→ to adjust\n\n   Enter to set as default · s to use this session only · Esc to cancel\n";
    const CONFIRM: &str = "   Change effort level?\n   Your next response will be slower and use more tokens\n\n   This conversation is cached for the current effort level. Switching to max means the\n   full history gets re-read on your next message.\n\n   ❯ 1. Yes, switch to max\n     2. No, go back\n";

    fn keywords(rows: &[ModelRow]) -> Vec<&str> { rows.iter().map(|r| r.keyword.as_str()).collect() }
    fn row(number: i64, keyword: &str, cursor: bool) -> ModelRow {
        ModelRow { number, label: keyword.into(), name: keyword.into(), desc: String::new(), keyword: keyword.into(), id: keyword.into(), cursor, active: false }
    }

    /// B14 (`test_parse_model_rows_opus_fixture`, `test_parse_model_rows_haiku_cursor_distinct_from_active`,
    /// `test_parse_model_rows_reads_fable_and_descriptions`).
    #[test]
    fn rows_of_the_captured_pickers() {
        let rows = parse_model_rows(OPUS);
        assert_eq!(rows.iter().map(|r| r.number).collect::<Vec<_>>(), [1, 2, 3, 4]);
        assert_eq!(keywords(&rows), ["default", "opus", "sonnet", "haiku"]);
        assert_eq!(rows.iter().map(|r| r.cursor).collect::<Vec<_>>(), [false, true, false, false]);
        assert_eq!(rows.iter().map(|r| r.active).collect::<Vec<_>>(), [false, true, false, false]);
        let haiku = parse_model_rows(HAIKU);
        assert_eq!(haiku.iter().find(|r| r.cursor).map(|r| r.keyword.as_str()), Some("haiku"));
        assert_eq!(haiku.iter().find(|r| r.active).map(|r| r.keyword.as_str()), Some("opus"));
        let fable = parse_model_rows(FABLE);
        assert_eq!(keywords(&fable), ["default", "opus", "fable", "sonnet", "haiku"]);
        assert_eq!(fable[2].name, "Fable");
        assert!(fable[2].desc.starts_with("Fable 5 ·"));
        assert_eq!((fable[1].name.as_str(), fable[1].active), ("Opus (1M context)", true));
        assert_eq!(fable[0].name, "Default");
    }

    /// B14 (`test_parse_model_rows_da_id_unico_as_duas_linhas_opus`, `test_id_nao_muda_quando_so_a_DESCRICAO_fala_de_1m`,
    /// `test_linha_unica_com_1m_no_rotulo_tambem_ganha_o_sufixo`, `test_nav_steps_leva_a_linha_certa_das_duas_opus`).
    #[test]
    fn unique_ids_and_the_1m_variant() {
        let rows = parse_model_rows(DUAS_OPUS);
        assert_eq!(keywords(&rows), ["default", "sonnet", "fable", "opus", "haiku", "opus"]);
        assert_eq!(rows.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["default", "sonnet", "fable", "opus", "haiku", "opus[1m]"]);
        assert_eq!(model_nav_steps(&rows, "opus[1m]"), Ok(2));
        assert_eq!(model_nav_steps(&rows, "opus"), Ok(0));
        assert_eq!(model_nav_steps(&rows, "fable"), Ok(-1));
        assert_eq!(parse_model_rows(OPUS).iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["default", "opus", "sonnet", "haiku"]);
        let fable = parse_model_rows(FABLE);
        assert_eq!(fable.iter().find(|r| r.keyword == "opus").unwrap().id, "opus[1m]");
        assert_eq!((model_nav_steps(&fable, "opus[1m]"), model_nav_steps(&fable, "opus")), (Ok(0), Ok(0)));
    }

    /// B14 (`test_model_nav_steps_from_opus_cursor`, `test_model_nav_steps_unknown_target_raises`,
    /// `test_model_nav_steps_offscreen_target_uses_fallback_number`, `test_variante_fora_da_tela_falha_em_vez_de_ir_pra_linha_errada`).
    #[test]
    fn navigation_steps_and_fallbacks() {
        let rows = parse_model_rows(OPUS);
        assert_eq!([model_nav_steps(&rows, "haiku"), model_nav_steps(&rows, "sonnet"), model_nav_steps(&rows, "default"), model_nav_steps(&rows, "opus")],
            [Ok(2), Ok(1), Ok(-1), Ok(0)]);
        assert_eq!(model_nav_steps(&rows, "gpt"), Err("target model 'gpt' not in picker".into()));
        let offscreen = [row(5, "opus", true), row(4, "haiku", false)];
        assert_eq!(model_nav_steps(&offscreen, "default"), Ok(-4));
        let short = [row(1, "default", true), row(2, "sonnet", false)];
        assert_eq!(model_nav_steps(&short, "opus[1m]"), Err("target model 'opus[1m]' not in picker".into()));
        assert_eq!(model_nav_steps(&short, "haiku"), Ok(4));
        assert_eq!(model_nav_steps(&[row(1, "default", false)], "opus"), Err("cursor row not found in picker".into()));
    }

    /// B13 (`test_picker_open_detection`, `test_parse_model_rows_ignores_chat_scrollback_numbered_list`,
    /// `test_espera_o_picker_INTEIRO_antes_de_ler_a_lista`).
    #[test]
    fn open_and_drawn_picker() {
        assert!(picker_open(OPUS));
        assert!(!picker_open("apenas chat sem picker\n❯ \n"));
        assert!(parse_model_rows("assistant: passos:\n  1. abrir\n  2. fechar\n❯ \n").is_empty());
        let partial = FABLE.replace("     5. Haiku", "").replace("   Enter to set as default · s to use this session only · Esc to cancel", "");
        assert!(picker_open(&partial) && !picker_drawn(&partial));
        assert!(picker_drawn(FABLE));
    }

    /// B15 (`test_parse_current_effort_opus_is_xhigh`, `..._haiku_not_supported_is_none`, `..._handles_default_suffix`).
    #[test]
    fn current_effort() {
        assert_eq!(parse_current_effort(OPUS).as_deref(), Some("xhigh"));
        assert_eq!(parse_current_effort(HAIKU), None);
        let pane = OPUS.replace("◉ xHigh effort ←/→ to adjust", "● High effort (default) ←/→ to adjust");
        assert_eq!(parse_current_effort(&pane).as_deref(), Some("high"));
    }

    /// B17 (`test_effort_confirm_open_detects_dialog`).
    #[test]
    fn effort_confirmation_dialog() {
        assert!(effort_confirm_open(CONFIRM));
        assert!(!effort_confirm_open(OPUS));
    }

    /// B19 (`test_parse_result_line_session_only`, `_default`, `_absent`).
    #[test]
    fn result_line() {
        assert_eq!(parse_result_line("  ⎿  Set model to Sonnet 4.6 for this session only with high effort\n❯ \n").as_deref(),
            Some("Set model to Sonnet 4.6 for this session only with high effort"));
        assert!(parse_result_line("  ⎿  Set model to Opus 4.8 and saved as your default for new sessions\n❯ \n").unwrap().contains("saved as your default"));
        assert_eq!(parse_result_line("❯ \nsem resultado\n"), None);
    }

    /// Painel do plugin à direita da conversa: a borda dele e o espaço até ela não são do resultado.
    #[test]
    fn result_line_stops_at_the_side_panel_border() {
        assert_eq!(parse_result_line("  ⎿  Set model to Opus 4.8 for this session only with medium effort      │ hangar-btw   \n❯ \n").as_deref(),
            Some("Set model to Opus 4.8 for this session only with medium effort"));
    }

    /// B9 (`test_set_model_effort_requires_model_or_effort`, `test_set_model_effort_rejects_malformed_model`,
    /// `test_apply_aceita_a_variante_de_1m_e_recusa_lixo`) e as mensagens do Python executado.
    #[test]
    fn validation_before_the_terminal() {
        assert_eq!(normalize(None, None), Err("must provide model or effort".into()));
        assert_eq!(normalize(Some(""), Some("")), Err("must provide model or effort".into()));
        assert_eq!(normalize(Some("opus[1m]"), None), Ok((Some("opus[1m]".into()), None)));
        assert_eq!(normalize(Some(" OPUS "), Some(" Max ")), Ok((Some("opus".into()), Some("max".into()))));
        // Só espaço conta como pedido (o `if model` do Python) e chega vazio ao seletor.
        assert_eq!(normalize(Some("  "), None), Ok((Some(String::new()), None)));
        for bad in ["opus 5", "opus;rm -rf /", "--model", "opus[1m]x", "opus[]", "opus[1m"] {
            assert_eq!(normalize(Some(bad), None), Err(format!("unknown model '{bad}'")), "{bad}");
        }
        assert_eq!(normalize(Some("Opus; rm"), None), Err("unknown model 'Opus; rm'".into()));
        assert_eq!(normalize(None, Some(" Turbo ")), Err("unknown effort ' Turbo '".into()));
        assert_eq!(normalize(Some("it's"), None), Err("unknown model \"it's\"".into()));
        assert_eq!(normalize(Some("a\nb"), None), Err("unknown model 'a\\nb'".into()));
    }

    proptest! {
        /// Texto de pane qualquer não derruba a leitura.
        #[test]
        fn pane_text_never_panics(pane in "(?s).{0,400}", picker in proptest::sample::select(vec![OPUS, HAIKU, FABLE, DUAS_OPUS, CONFIRM]), cut in 0usize..1200) {
            let at = picker.char_indices().map(|(i, _)| i).chain([picker.len()]).nth(cut % (picker.chars().count() + 1)).unwrap_or(0);
            for text in [pane.clone(), format!("{}{pane}", &picker[..at]), format!("{pane}\n{picker}")] {
                let rows = parse_model_rows(&text);
                let _ = (picker_open(&text), picker_drawn(&text), parse_current_effort(&text), effort_confirm_open(&text), parse_result_line(&text));
                let _ = model_nav_steps(&rows, "opus");
                let _ = normalize(Some(&text), Some(&text));
            }
        }
    }
}

pub fn normalize_engine(model: &str) -> Result<String, String> {
    let target = trim(model);
    if target.is_empty() || target.chars().any(|c| whitespace(c) || (c as u32) < 32) {
        return Err(format!("model invalido: {}", py_repr(model)));
    }
    Ok(target.to_owned())
}

/// Só a confirmação do token completo prova a troca: `k3-256k` não confirma `k3`.
pub fn result_model(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("Set model to")?;
    if !rest.chars().next().is_some_and(whitespace) { return None; }
    rest.split(whitespace).find(|part| !part.is_empty())
}

/// Motores aceitam IDs arbitrários por argumento; o seletor só oferece aliases da Anthropic.
pub async fn drive_engine(driver: &TerminalDriver, model: &str) -> Result<Value, Refusal> {
    let model = normalize_engine(model).map_err(|detail| coded("erro_corpo_invalido", detail))?;
    let model = model.as_str();
    let mut picker = Picker { sequence: driver.permission_sequence().await, typed: false };
    picker.require_drivable().await?;
    let typed = picker.sequence.type_text(&format!("/model {model}")).await.map_err(|f| io(f, false));
    picker.typed = true;
    typed?;
    sleep(SETTLE).await;
    picker.press("Enter").await?;
    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        sleep(SETTLE).await;
        let pane = picker.screen().await?;
        if picker_open(&pane) {
            return Err(picker.abort(format!("o Claude Code nao aceitou `/model {model}`")).await);
        }
        let result = parse_result_line(&pane);
        if result.as_deref().and_then(result_model) == Some(model) {
            return Ok(json!({"ok":true,"result":result}));
        }
        if Instant::now() >= deadline {
            return Err(Refusal { typed: true, ..refused(format!("sem confirmacao do `/model {model}` no terminal")) });
        }
    }
}
