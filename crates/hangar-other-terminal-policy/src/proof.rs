//! Provas da digitação e da posse do compositor.
use std::collections::BTreeSet;
use std::sync::LazyLock;

use regex::Regex;

use crate::composer::compact;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Proof { Present, Absent, Unreadable }
#[derive(Clone, Debug)]
pub struct ComposerSnapshot { pub content: String, pub placeholders: BTreeSet<String>, pub stashed: bool }
static PLACEHOLDER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[(Pasted text|Image) #(\d+)").unwrap());
static PASTE_LINES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[Pasted text #\d+(?: \+(\d+) lines)?\]").unwrap());
static LINE_BREAK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\r\n|\r|\n").unwrap());
static AGENT_ROW: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*(?:❯\s+)?[●◯]\s+(\S+)").unwrap());
// Dica de argumento que o Claude desenha depois de um comando com barra completo (`/clear │ [name]`,
// `/compact <optional custom summarization instructions>`). Em alguns temas ela não vem esmaecida.
// Sem `#`: anexo (`[Image #1]`) nunca passa por dica.
static ARG_HINT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(?:\[[^\[\]#]*\]|<[^<>#]*>)$").unwrap());
/// O Claude marca `› stashed` na linha de dicas acima do composer enquanto guarda um rascunho.
pub fn stash_held(screen: &str) -> bool {
    let lines: Vec<_> = screen.split('\n').collect();
    let rules: Vec<_> = lines.iter().enumerate().filter(|(_, s)| s.matches('─').count() >= 20).map(|(n, _)| n).collect();
    rules.len().checked_sub(2).and_then(|n| rules[n].checked_sub(1)).is_some_and(|n| lines[n].contains("› stashed"))
}
impl ComposerSnapshot {
    /// As linhas da tela e as duas últimas réguas (topo e base do composer), sem conferir tamanho.
    fn frame(screen: &str) -> Option<(Vec<&str>, usize, usize)> {
        let mut lines: Vec<_> = screen.split('\n').collect();
        while lines.last().is_some_and(|s| s.trim().is_empty()) { lines.pop(); }
        // O painel de agentes ganha uma linha por subagente abaixo do rodapé: fora da conta da distância.
        // Só corta com a linha `main` e um `◯`: bloco só de `●` é conversa, e opção `◯` sem `main` é diálogo.
        let panel = lines.iter().rev().take_while(|s| AGENT_ROW.is_match(s)).count();
        let rows = &lines[lines.len() - panel..];
        if rows.iter().any(|s| s.contains('◯')) && rows.iter().any(|s| AGENT_ROW.captures(s).is_some_and(|c| &c[1] == "main")) {
            lines.truncate(lines.len() - panel);
            while lines.last().is_some_and(|s| s.trim().is_empty()) { lines.pop(); }
        }
        let rules: Vec<_> = lines.iter().enumerate().filter(|(_, s)| s.matches('─').count() >= 20).map(|(n, _)| n).collect();
        let bottom = *rules.last()?;
        let top = *rules.get(rules.len().checked_sub(2)?)?;
        Some((lines, top, bottom))
    }
    /// Composer no pé da tela e mais alto do que a leitura aceita: mensagem longa colada, não outra tela.
    pub fn too_tall(screen: &str) -> bool {
        Self::frame(screen).is_some_and(|(lines, top, bottom)| lines.len() - bottom <= 8 && bottom - top > 40)
    }
    pub fn parse(screen: &str) -> Option<Self> {
        let (lines, top, bottom) = Self::frame(screen)?;
        // Mensagem longa colada cresce o composer; acima disto as réguas já não são as dele.
        if lines.len() - bottom > 8 || bottom - top > 40 { return None; }
        let content = lines[top+1..bottom].iter().map(|s| s.trim().strip_prefix('❯').unwrap_or(s.trim()).trim()).collect::<Vec<_>>().join("\n");
        let placeholders = PLACEHOLDER.captures_iter(&content).map(|c| format!("{}:{}", &c[1], &c[2])).collect();
        Some(Self { content, placeholders, stashed: stash_held(screen) })
    }
    pub fn is_empty(&self) -> bool { self.content.trim().is_empty() }
    /// O composer compactado, sem a dica de argumento que segue o comando com barra digitado. Só
    /// com o composer vazio antes: um rascunho `/clear [nota]` do dono não vira dica.
    fn typed(&self, text: &str, before: &Self) -> String {
        let (visible, expected) = (compact(&self.content), compact(text));
        let hinted = before.is_empty() && expected.starts_with('/')
            && visible.strip_prefix(expected.as_str()).is_some_and(|rest| ARG_HINT.is_match(rest));
        if hinted { expected } else { visible }
    }
    pub fn proves(&self, text: &str, before: &Self) -> Proof {
        if self.placeholders.difference(&before.placeholders).next().is_some() { return Proof::Present; }
        let visible = self.typed(text, before);
        let expected = compact(text);
        if expected.is_empty() { return Proof::Unreadable; }
        if expected.chars().count() < 12 {
            return if visible == expected { Proof::Present } else if self.is_empty() { Proof::Absent } else { Proof::Unreadable };
        }
        let head: String = text.trim().chars().take(40).collect();
        let tail: String = text.trim().rsplit('\n').next().unwrap_or("").trim().chars().rev().take(40).collect::<String>().chars().rev().collect();
        if [head, tail].iter().map(|s| compact(s)).any(|s| s.chars().count() >= 12 && visible.contains(&s)) { Proof::Present } else { Proof::Absent }
    }
    /// O campo tem exatamente `text` e mais nada (nem anexo): só então um Enter de novo não leva texto alheio.
    fn only(&self, text: &str) -> bool { self.placeholders.is_empty() && !self.is_empty() && compact(&self.content) == compact(text) }
    /// Também vale o campo só com colagens novas: o Claude mostra o recado longo como `[Pasted text #N]`.
    /// O `+N lines` é a conta de quebras do Claude: colagem da pessoa, de outro tamanho, não leva o nosso Enter.
    pub fn resendable(&self, text: &str, before: &Self) -> bool {
        let lines: usize = PASTE_LINES.captures_iter(&self.content).map(|c| c.get(1).map_or(0, |n| n.as_str().parse().unwrap_or(usize::MAX))).sum();
        self.only(text) || !self.placeholders.is_empty() && self.placeholders.iter().all(|p| p.starts_with("Pasted text:"))
            && lines == LINE_BREAK.find_iter(text).count() && self.owned(text, before)
    }
    pub fn owned(&self, text: &str, before: &Self) -> bool {
        if self.typed(text, before) == compact(text) { return true; }
        let mut residual = self.content.clone();
        for hit in PLACEHOLDER.find_iter(&self.content) {
            let end = self.content[hit.start()..].find(']').map(|n| hit.start()+n+1);
            if let Some(end) = end { residual = residual.replace(&self.content[hit.start()..end], ""); }
        }
        !self.placeholders.is_empty() && self.placeholders.is_disjoint(&before.placeholders) && residual.trim().is_empty()
    }
}

#[cfg(test)]
mod repeat_enter_tests {
    use super::ComposerSnapshot;
    fn field(content: &str) -> ComposerSnapshot {
        ComposerSnapshot::parse(&format!("{rule}\n❯ {content}\n{rule}\n", rule = "─".repeat(32))).unwrap()
    }
    #[test]
    fn only_our_exact_text_allows_another_enter() {
        assert!(field("Olá de novo").only("Olá de novo"));
        assert!(field("Olá   de\nnovo").only("Olá de novo"), "quebra e espaço do desenho não contam");
        assert!(!field("Olá de novo e mais").only("Olá de novo"), "texto da pessoa junto");
        assert!(!field("Olá").only("Olá de novo"), "pedaço");
        assert!(!field("Olá de novo [Image #1]").only("Olá de novo"), "anexo junto");
        assert!(!field("").only("Olá de novo"));
    }
    #[test]
    fn our_long_message_shown_as_a_paste_allows_another_enter() {
        let long = "linha\n".repeat(30) + "fim";
        let empty = field("");
        assert!(field("[Pasted text #1 +30 lines]").resendable(&long, &empty), "o Claude resume o recado longo");
        assert!(field("Olá de novo").resendable("Olá de novo", &empty));
        assert!(!field("[Pasted text #1 +30 lines]").resendable(&long, &field("[Pasted text #1 +30 lines]")), "colagem que já estava lá");
        assert!(!field("[Pasted text #1 +30 lines] e mais").resendable(&long, &empty), "texto da pessoa junto");
        assert!(!field("[Pasted text #1 +30 lines] [Image #2]").resendable(&long, &empty), "anexo junto");
        assert!(!field("[Image #1]").resendable(&long, &empty));
        assert!(!field("[Pasted text #2 +5 lines]").resendable(&long, &empty), "colagem da pessoa, de outro tamanho");
        assert!(field("[Pasted text #1 +10 lines] [Pasted text #2 +20 lines]").resendable(&long, &empty), "o nosso recado em dois pedaços");
    }
}
