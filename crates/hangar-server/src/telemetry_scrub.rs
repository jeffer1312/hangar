//! A limpeza forte das linhas de aviso e erro antes da telemetria. Trecho entre aspas sai primeiro;
//! depois do `Scrubber` do relatório (credenciais, IP, `ts.net`, base64 e hex longos), caminho, URL,
//! e-mail, nome de sessão, de conta e de máquina, nome com ponto, `host:porta`, id e número longo
//! viram marcador: a linha sai sem nada da pessoa e agrupa entre máquinas.
use std::sync::LazyLock;

use hangar_telemetry::wire::MAX_LOG_CHARS;
use std::collections::HashMap;

use regex::{Captures, Regex, RegexBuilder};

use crate::diag_report::Scrubber;

/// Nome curto casaria palavra comum da mensagem.
const MIN_NAME_CHARS: usize = 3;
/// Nome igual à palavra de um marcador o desmancharia (`<url>` virando `<<sessão>>`).
const MARK_WORDS: &[&str] = &["url", "caminho", "email", "sessão", "máquina", "conta", "nome"];

/// Os nomes da pessoa nesta máquina: sessões, contas (e o usuário do sistema) e máquinas.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Names {
    pub(crate) sessions: Vec<String>,
    pub(crate) accounts: Vec<String>,
    pub(crate) machines: Vec<String>,
}

/// Os nomes num regex só, sem caixa (Unicode) e do maior para o menor, montado uma vez por conjunto.
pub(crate) struct Words {
    regex: Option<Regex>,
    /// Nome em minúsculas → marcador.
    marks: HashMap<String, &'static str>,
}

impl Names {
    /// `None` só quando o regex não se monta: quem chama mantém o conjunto anterior.
    pub(crate) fn words(&self) -> Option<Words> {
        let marked = [(&self.sessions, "<sessão>"), (&self.accounts, "<conta>"), (&self.machines, "<máquina>")];
        let mut marks = HashMap::new();
        for (names, mark) in marked {
            for name in names.iter().map(|name| name.trim().to_lowercase()) {
                if name.chars().count() >= MIN_NAME_CHARS && !MARK_WORDS.contains(&name.as_str()) {
                    marks.entry(name).or_insert(mark);
                }
            }
        }
        let mut names: Vec<&String> = marks.keys().collect();
        // O maior primeiro: na mesma posição `ajuste-x` vence `ajuste`.
        names.sort_by(|a, b| b.chars().count().cmp(&a.chars().count()).then_with(|| a.cmp(b)));
        let regex = match names.is_empty() {
            true => None,
            false => {
                let alternation = names.iter().map(|name| regex::escape(name)).collect::<Vec<_>>().join("|");
                Some(RegexBuilder::new(&alternation).case_insensitive(true).size_limit(64 << 20).build().ok()?)
            }
        };
        Some(Words { regex, marks })
    }
}

fn re(pattern: &str) -> Regex { Regex::new(pattern).unwrap() }

// Texto entre aspas é dado (resposta de serviço, nome, trecho da conversa), nunca a frase do código.
// Escape (`\"`) não fecha o trecho; trecho só de aspas escapadas (JSON dentro de texto) também conta.
static QUOTED: LazyLock<Regex> = LazyLock::new(|| {
    re(r#"\\"(?:[^"\\]|\\[^"])*\\"|"(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*'|`(?:[^`\\]|\\.)*`"#)
});
static URL: LazyLock<Regex> = LazyLock::new(|| re(r#"(?i)\b(?:https?|wss?)://[^\s"'`<>]+"#));
// Caminho para em espaço, aspas, `;`, `,` ou `)`. Se antes da próxima `; , )` ou aspas houver outro
// trecho com barra, o caminho vai até o fim dele: pasta com espaço (`C:\Program Files\X`, `cliente x/doc`).
const MORE: &str = r#"(?:[^;,)"'`\r\n]*[\\/][^\s"'`<>;,)]*)?"#;
// Casa (`~`), as formas já trocadas pelo `Scrubber`, letra de unidade e UNC.
static ROOTED_PATH: LazyLock<Regex> = LazyLock::new(|| {
    re(&format!(r#"(?:(?:<tmp-claude>|%LOCALAPPDATA%)[^\s"'`<>;,)]*|(?:~|\b[A-Za-z]:)[\\/][^\s"'`<>;,)]*|\\\\[^\s"'`<>\\]+[^\s"'`<>;,)]*){MORE}"#))
});
// Absoluto com dois segmentos ou mais; `/clear` sozinho é comando, não pasta.
static UNIX_PATH: LazyLock<Regex> =
    LazyLock::new(|| re(&format!(r#"(^|[\s"'(=\[,:`{{>;|])/[^\s"'`<>/]+/[^\s"'`<>;,)]*{MORE}"#)));
static EMAIL: LazyLock<Regex> = LazyLock::new(|| re(r"[A-Za-z0-9._%+*-]+@[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)*\.[A-Za-z]{2,}"));
static HOST_PORT: LazyLock<Regex> = LazyLock::new(|| re(r"\b[A-Za-z][A-Za-z0-9.-]*:[0-9]{2,5}\b"));
// Arquivo e domínio (`a.rs`, `casa.exemplo.dev`): ponto seguido de letra. Versão (`0.42.0`) e `1.5s` ficam,
// e o `::` do Rust não tem ponto.
static DOTTED: LazyLock<Regex> = LazyLock::new(|| re(r"[\p{L}\p{N}_-]+(?:\.[\p{L}\p{N}_-]+)+"));
// Pid, porta alta e id mudam a cada vez e partiriam a mesma linha em várias.
static NUMBER: LazyLock<Regex> = LazyLock::new(|| re(r"[0-9]{6,}"));
// Chave de sessão, id de conversa e UUID: identificam uma coisa da pessoa e também partiriam a linha.
static ID: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)\b(?:[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}|[0-9a-f]{16,})\b"));

/// A mensagem pronta para `Telemetry::log`, cortada em `MAX_LOG_CHARS`.
pub(crate) fn clean(scrubber: &Scrubber, text: &str, words: &Words) -> String {
    let out = QUOTED.replace_all(text, |c: &Captures| {
        let quote = if c[0].starts_with('\\') { &c[0][..2] } else { &c[0][..1] };
        format!("{quote}…{quote}")
    });
    let out = scrubber.scrub(&out);
    let out = URL.replace_all(&out, "<url>");
    let out = ROOTED_PATH.replace_all(&out, |c: &Captures| path_mark(&c[0]));
    let out = UNIX_PATH.replace_all(&out, |c: &Captures| format!("{}{}", &c[1], path_mark(&c[0][c[1].len()..])));
    let out = EMAIL.replace_all(&out, "<email>");
    let out = replace_names(&out, words);
    let out = HOST_PORT.replace_all(&out, "<nome>");
    let out = DOTTED.replace_all(&out, |c: &Captures| {
        let token = &c[0];
        let named = token.split('.').skip(1).any(|part| part.chars().next().is_some_and(char::is_alphabetic));
        if named { "<nome>".to_owned() } else { token.to_owned() }
    });
    let out = ID.replace_all(&out, "<id>");
    NUMBER.replace_all(&out, "<n>").chars().take(MAX_LOG_CHARS).collect()
}

/// A pontuação que fecha a frase fica fora do caminho.
fn path_mark(path: &str) -> String {
    let kept = path.trim_end_matches(['.', ',', ';', ':', ')', ']', '}']);
    format!("<caminho>{}", &path[kept.len()..])
}

fn is_word(c: char) -> bool { c.is_alphanumeric() || c == '_' || c == '-' }

/// Troca cada nome inteiro (sem letra, dígito, `_` ou `-` colado) pelo marcador dele. As posições são as
/// do próprio texto: nenhuma cópia em outra caixa, que pode ter outro tamanho.
fn replace_names(text: &str, words: &Words) -> String {
    let Some(regex) = &words.regex else { return text.to_owned() };
    regex.replace_all(text, |c: &Captures| {
        let found = c.get(0).expect("o regex casou");
        let alone = !text[..found.start()].chars().next_back().is_some_and(is_word) && !text[found.end()..].chars().next().is_some_and(is_word);
        if !alone {
            return found.as_str().to_owned();
        }
        words.marks.get(&found.as_str().to_lowercase()).copied().unwrap_or("<nome>").to_owned()
    }).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names() -> Names {
        Names {
            sessions: vec!["ajuste-login-Coordenadora".into(), "ajuste-login".into(), "url".into(), "ab".into()],
            accounts: vec!["claude-200-5".into(), "jefferson".into()],
            machines: vec!["estudio-02".into(), "notebook-fulano".into()],
        }
    }

    fn cleaned(text: &str) -> String { clean(&Scrubber::new(Some("/home/fulano".into()), None), text, &names().words().unwrap()) }

    #[test]
    fn real_lines_lose_what_identifies_the_person() {
        assert_eq!(cleaned("falha ao abrir /home/fulano/pessoal/appEsquadrias/x.rs"), "falha ao abrir <caminho>");
        assert_eq!(cleaned("sessão ajuste-login-Coordenadora caiu; ajuste-login segue"), "sessão <sessão> caiu; <sessão> segue");
        assert_eq!(cleaned("peer estudio-02: sem resposta"), "peer <máquina>: sem resposta");
        assert_eq!(cleaned("peer ESTUDIO-02 recusou"), "peer <máquina> recusou");
        assert_eq!(cleaned("conta claude-200-5 do jefferson sem login"), "conta <conta> do <conta> sem login");
        assert_eq!(cleaned("authorization: Bearer abc123def456"), "authorization: Bearer ***");
        assert_eq!(cleaned("conta a@b.com sem login"), "conta <email> sem login");
    }

    #[test]
    fn quoted_text_is_hidden_and_keeps_its_quotes() {
        assert_eq!(cleaned(r#"invalid type: string "texto da conversa", expected u64"#), r#"invalid type: string "…", expected u64"#);
        assert_eq!(cleaned("`/home/fulano/x/y` sumiu"), "`…` sumiu");
        assert_eq!(cleaned("sessão 'Meu Projeto' fechou"), "sessão '…' fechou");
        assert_eq!(cleaned(r#"login de "fulano.tal@empresa.com.br" recusado"#), r#"login de "…" recusado"#);
        assert_eq!(cleaned(r#"corpo "início \"conteúdo privado\" fim" recusado"#), r#"corpo "…" recusado"#);
        assert_eq!(cleaned(r#"corpo início \"conteúdo privado\" fim"#), r#"corpo início \"…\" fim"#);
    }

    #[test]
    fn paths_urls_and_long_numbers_become_marks() {
        assert_eq!(cleaned("leu ~/.hangar/config/.env, falhou"), "leu <caminho>, falhou");
        assert_eq!(cleaned("abrir /etc/hangar/x.json: negado"), "abrir <caminho>: negado");
        assert_eq!(cleaned("abrir ~/My Projects/cliente-x/a.rs"), "abrir <caminho>");
        assert_eq!(cleaned(r"C:\Program Files\X\y.exe falhou"), "<caminho> falhou");
        assert_eq!(cleaned(r"share \\servidor\pasta caiu"), "share <caminho> caiu");
        assert_eq!(cleaned("ler {/var/lib/x} e; >/tmp/a/b; |/opt/c/d"), "ler {<caminho>} e; ><caminho>; |<caminho>");
        assert_eq!(cleaned("/home/fulano/cliente projeto secreto/documento falhou"), "<caminho> falhou");
        assert_eq!(cleaned("abrir /srv/cliente x/a.rs, depois /srv/y/b.rs; fim"), "abrir <caminho>, depois <caminho>; fim");
        assert_eq!(cleaned("GET https://casa.exemplo.hangar.dev.br/api/x?y=1 deu 502"), "GET <url> deu 502");
        assert_eq!(cleaned("pid 1234567 na porta 45678 saiu"), "pid <n> na porta 45678 saiu");
        assert_eq!(cleaned("comando /clear recusado"), "comando /clear recusado", "um segmento só é comando");
        assert_eq!(cleaned("lista não lida dac3cb3913be3aafadbd3e77e32cb1ff"), "lista não lida ***", "o Scrubber já tira hex longo");
        assert_eq!(cleaned("chave a1b2c3d4e5f60718 sumiu"), "chave <id> sumiu");
        assert_eq!(cleaned("conversa 6f1c2a9e-1b2c-4d5e-8f90-123456789abc sumiu"), "conversa <id> sumiu");
        assert_eq!(cleaned("código deadbeef ficou"), "código deadbeef ficou", "hex curto é palavra comum");
    }

    #[test]
    fn dotted_names_and_host_port_become_marks() {
        assert_eq!(cleaned("abrir appEsquadrias.rs"), "abrir <nome>");
        assert_eq!(cleaned("notebook-tunel:8765 recusou"), "<nome> recusou");
        assert_eq!(cleaned("casa.exemplo.hangar.dev.br sem resposta"), "<nome> sem resposta");
        assert_eq!(cleaned("versão 0.42.0 levou 1.5s em std::io::Error"), "versão 0.42.0 levou 1.5s em std::io::Error");
    }

    #[test]
    fn names_change_only_whole_words() {
        assert_eq!(cleaned("arquivo estudio-02x e xestudio-02 ficam"), "arquivo estudio-02x e xestudio-02 ficam");
        assert_eq!(cleaned("ab e url ficam como estão"), "ab e url ficam como estão", "nome curto ou de marcador não é trocado");
        assert_eq!(cleaned("GET http://x/y falhou"), "GET <url> falhou");
    }

    #[test]
    fn names_ignore_case_in_any_alphabet_and_need_the_whole_word() {
        let names = Names { sessions: vec!["Ação".into(), "abc".into(), "ABC".into(), "abcdef".into()], accounts: vec!["João".into(), "xy".into()], machines: vec![] };
        let words = names.words().unwrap();
        let cleaned = |text: &str| clean(&Scrubber::new(None, None), text, &words);
        assert_eq!(cleaned("JOÃO e AÇÃO falharam; joãozinho e açãox ficam"), "<conta> e <sessão> falharam; joãozinho e açãox ficam");
        assert_eq!(cleaned("abcdef ABC Abc xy"), "<sessão> <sessão> <sessão> xy", "o maior primeiro, nome curto fica");
        // Maiúscula com outro tamanho em bytes antes do nome não desloca a troca.
        assert_eq!(cleaned("İİİ abc"), "İİİ <sessão>");
    }

    #[test]
    fn the_result_is_cut_at_the_wire_limit() {
        let long = format!("erro {}", "ç".repeat(400));
        assert_eq!(cleaned(&long).chars().count(), MAX_LOG_CHARS);
    }
}
