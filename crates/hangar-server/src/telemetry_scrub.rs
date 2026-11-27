//! A limpeza forte das linhas de aviso e erro antes da telemetria. Depois do `Scrubber` do relatório
//! (credenciais, IP, `ts.net`, base64 e hex longos), caminho, URL, e-mail, nome de sessão, nome de
//! máquina e número longo viram marcador: a linha sai sem nada da pessoa e agrupa entre máquinas.
use std::sync::LazyLock;

use hangar_telemetry::wire::MAX_LOG_CHARS;
use regex::{Captures, Regex};

use crate::diag_report::Scrubber;

/// Nome curto casaria palavra comum da mensagem.
const MIN_NAME_CHARS: usize = 3;
/// Nome igual à palavra de um marcador o desmancharia (`<url>` virando `<<sessão>>`).
const MARK_WORDS: &[&str] = &["url", "caminho", "email", "sessão", "máquina"];

/// Os nomes que mudam nesta máquina; quem chama relê a cada lote, não a cada linha.
#[derive(Default)]
pub(crate) struct Names {
    pub(crate) sessions: Vec<String>,
    pub(crate) machines: Vec<String>,
}

fn re(pattern: &str) -> Regex { Regex::new(pattern).unwrap() }

static URL: LazyLock<Regex> = LazyLock::new(|| re(r#"(?i)\b(?:https?|wss?)://[^\s"'<>]+"#));
// Casa (`~`), as formas já trocadas pelo `Scrubber`, letra de unidade e UNC.
static ROOTED_PATH: LazyLock<Regex> = LazyLock::new(|| {
    re(r#"(?:<tmp-claude>|%LOCALAPPDATA%)[^\s"'<>]*|(?:~|\b[A-Za-z]:)[\\/][^\s"'<>]*|\\\\[^\s"'<>\\]+[^\s"'<>]*"#)
});
// Absoluto com dois segmentos ou mais; `/clear` sozinho é comando, não pasta.
static UNIX_PATH: LazyLock<Regex> = LazyLock::new(|| re(r#"(^|[\s"'(=\[,:])/[^\s"'<>/]+/[^\s"'<>]*"#));
static EMAIL: LazyLock<Regex> = LazyLock::new(|| re(r"[A-Za-z0-9._%+*-]+@[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)*\.[A-Za-z]{2,}"));
// Pid, porta alta e id mudam a cada vez e partiriam a mesma linha em várias.
static NUMBER: LazyLock<Regex> = LazyLock::new(|| re(r"[0-9]{6,}"));

/// A mensagem pronta para `Telemetry::log`, cortada em `MAX_LOG_CHARS`.
pub(crate) fn clean(scrubber: &Scrubber, text: &str, names: &Names) -> String {
    let out = scrubber.scrub(text);
    let out = URL.replace_all(&out, "<url>");
    let out = ROOTED_PATH.replace_all(&out, |c: &Captures| path_mark(&c[0]));
    let out = UNIX_PATH.replace_all(&out, |c: &Captures| format!("{}{}", &c[1], path_mark(&c[0][c[1].len()..])));
    let mut out = EMAIL.replace_all(&out, "<email>").into_owned();
    let mut words: Vec<(&str, &str)> = names.sessions.iter().map(|n| (n.as_str(), "<sessão>"))
        .chain(names.machines.iter().map(|n| (n.as_str(), "<máquina>")))
        .filter(|(name, _)| name.chars().count() >= MIN_NAME_CHARS && !MARK_WORDS.iter().any(|w| name.eq_ignore_ascii_case(w)))
        .collect();
    // O maior primeiro: `ajuste-x` sai inteiro antes de `ajuste` casar dentro dele.
    words.sort_by_key(|(name, _)| std::cmp::Reverse(name.len()));
    for (name, mark) in words {
        out = replace_word(&out, name, mark);
    }
    NUMBER.replace_all(&out, "<n>").chars().take(MAX_LOG_CHARS).collect()
}

/// A pontuação que fecha a frase fica fora do caminho.
fn path_mark(path: &str) -> String {
    let kept = path.trim_end_matches(['.', ',', ';', ':', ')', ']']);
    format!("<caminho>{}", &path[kept.len()..])
}

fn is_word(c: char) -> bool { c.is_alphanumeric() || c == '_' || c == '-' }

/// Troca `word` inteiro (sem letra, dígito, `_` ou `-` colado), sem diferença de maiúscula ASCII.
fn replace_word(text: &str, word: &str, mark: &str) -> String {
    // Minúscula ASCII não muda o tamanho em bytes: as posições valem no texto original.
    let (hay, needle) = (text.to_ascii_lowercase(), word.to_ascii_lowercase());
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for (at, _) in hay.match_indices(&needle) {
        let end = at + needle.len();
        if text[..at].chars().next_back().is_some_and(is_word) || text[end..].chars().next().is_some_and(is_word) {
            continue;
        }
        out.push_str(&text[last..at]);
        out.push_str(mark);
        last = end;
    }
    out.push_str(&text[last..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names() -> Names {
        Names {
            sessions: vec!["ajuste-login-Coordenadora".into(), "ajuste-login".into(), "url".into(), "ab".into()],
            machines: vec!["estudio-02".into(), "notebook-fulano".into()],
        }
    }

    fn cleaned(text: &str) -> String { clean(&Scrubber::new(Some("/home/fulano".into()), None), text, &names()) }

    #[test]
    fn real_lines_lose_what_identifies_the_person() {
        assert_eq!(cleaned("falha ao abrir /home/fulano/pessoal/appEsquadrias/x.rs"), "falha ao abrir <caminho>");
        assert_eq!(cleaned("sessão ajuste-login-Coordenadora caiu; ajuste-login segue"), "sessão <sessão> caiu; <sessão> segue");
        assert_eq!(cleaned("peer estudio-02: sem resposta"), "peer <máquina>: sem resposta");
        assert_eq!(cleaned("peer ESTUDIO-02 recusou"), "peer <máquina> recusou");
        assert_eq!(cleaned("authorization: Bearer abc123def456"), "authorization: Bearer ***");
        assert_eq!(cleaned("conta a@b.com sem login"), "conta <email> sem login");
        assert_eq!(cleaned(r#"login de "fulano.tal@empresa.com.br" recusado"#), r#"login de "<email>" recusado"#);
    }

    #[test]
    fn paths_urls_and_long_numbers_become_marks() {
        assert_eq!(cleaned("leu ~/.hangar/config/.env, falhou"), "leu <caminho>, falhou");
        assert_eq!(cleaned("abrir /etc/hangar/x.json: negado"), "abrir <caminho>: negado");
        assert_eq!(cleaned(r"C:\Program\x.exe saiu"), "<caminho> saiu");
        assert_eq!(cleaned(r"share \\servidor\pasta caiu"), "share <caminho> caiu");
        assert_eq!(cleaned(r#"path="/var/lib/x" sumiu"#), r#"path="<caminho>" sumiu"#);
        assert_eq!(cleaned("GET https://casa.exemplo.hangar.dev.br/api/x?y=1 deu 502"), "GET <url> deu 502");
        assert_eq!(cleaned("pid 1234567 na porta 45678 saiu"), "pid <n> na porta 45678 saiu");
        assert_eq!(cleaned("comando /clear recusado"), "comando /clear recusado", "um segmento só é comando");
    }

    #[test]
    fn names_change_only_whole_words() {
        assert_eq!(cleaned("arquivo estudio-02x e xestudio-02 ficam"), "arquivo estudio-02x e xestudio-02 ficam");
        assert_eq!(cleaned("ab e url ficam como estão"), "ab e url ficam como estão", "nome curto ou de marcador não é trocado");
        assert_eq!(cleaned("GET http://x/y falhou"), "GET <url> falhou");
    }

    #[test]
    fn the_result_is_cut_at_the_wire_limit() {
        let long = format!("erro {}", "ç".repeat(400));
        assert_eq!(cleaned(&long).chars().count(), MAX_LOG_CHARS);
    }
}
