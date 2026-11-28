use std::{collections::BTreeSet, sync::LazyLock};

use hangar_transcript::Provider;
use regex::Regex;

static PLACEHOLDER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[(?:Pasted\s*text|paste)\s*#(\d+)|\[Image\s*#(\d+)\]").unwrap());
/// Colagem longa que o Codex recolhe na caixa: o texto não aparece, só o marcador com o tamanho.
static CODEX_PASTE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[Pasted Content (\d+) chars\]").unwrap());
static AGENT_ROW: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(?:❯\s+)?[●◯]\s+(\S+)").unwrap());
static PI_NOTICE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"Subagentasyncgroupedresultintercomdeliverywasnotacknowledgedfor.*?/tmp/pi-subagents-[^']*'?\.?").unwrap()
});
/// A caixa vazia do `agy` mostra a dica do modo (`Auto mode: … (shift+tab to cycle)`), não um rascunho.
/// Até onde a caixa do `agy` pode estar do fim da tela: abaixo dela ficam o rodapé e o painel de tarefas de fundo.
const AGY_BOX_FROM_END: usize = 24;
static AGY_MODE_HINT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\A[A-Z][\w-]* mode: .*\(shift\+tab to cycle\)\z").unwrap());
const CODEX_PLACEHOLDER: &str = "Ask Codex to do anything";

/// As versões da mesma captura e o editor nativo do Pi/omp, quando disponível.
pub struct CapturedPane<'a> {
    pub text: &'a str,
    pub typed: Option<&'a str>,
    pub styled: Option<&'a str>,
    pub editor: Option<&'a str>,
}

/// Conteúdo lido da caixa de digitação, com os marcadores de colagem e anexo.
pub struct Composer {
    pub content: String,
    pub placeholders: BTreeSet<String>,
}

/// O rascunho na caixa do `agy` (vazio = caixa vazia ou só a dica do modo); `None` sem caixa legível.
pub fn agy_draft(screen: &str) -> Option<String> {
    let frame = CapturedPane {
        text: screen,
        typed: None,
        styled: None,
        editor: None,
    };
    composer(Provider::Antigravity, frame).map(|composer| composer.content)
}
pub fn composer(provider: Provider, frame: CapturedPane<'_>) -> Option<Composer> {
    if provider == Provider::Codex {
        return codex_composer(frame);
    }
    if matches!(provider, Provider::Pi | Provider::Omp)
        && let Some(editor) = frame.editor
    {
        return Some(Composer {
            content: editor.to_owned(),
            placeholders: BTreeSet::new(),
        });
    }
    let styled;
    let text = if let Some(typed) = frame.typed {
        typed
    } else if let Some(value) = frame.styled {
        styled = unstyle(value, false);
        &styled
    } else {
        frame.text
    };
    let mut lines: Vec<_> = text.lines().collect();
    while lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }
    let panel = lines
        .iter()
        .rev()
        .take_while(|line| AGENT_ROW.is_match(line))
        .count();
    let rows = &lines[lines.len() - panel..];
    if rows.iter().any(|line| line.contains('◯'))
        && rows
            .iter()
            .any(|line| AGENT_ROW.captures(line).is_some_and(|c| &c[1] == "main"))
    {
        lines.truncate(lines.len() - panel);
        while lines.last().is_some_and(|line| line.trim().is_empty()) {
            lines.pop();
        }
    }
    let rules: Vec<_> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.matches('─').count() >= 20)
        .map(|(n, _)| n)
        .collect();
    let last = *rules.last()?;
    let (top, bottom) = if provider == Provider::Antigravity {
        // O `agy` desenha o painel de tarefas em segundo plano entre linhas, abaixo da caixa: a caixa é a que abre com `>`.
        rules
            .windows(2)
            .rev()
            .map(|pair| (pair[0], pair[1]))
            // A caixa é o último par ou o de cima do painel de tarefas: um pedido antigo repetido mais acima não conta.
            .take(2)
            .take_while(|(_, bottom)| lines.len() - bottom <= AGY_BOX_FROM_END)
            .find(|(top, bottom)| {
                *bottom > top + 1
                    && lines[top + 1]
                        .trim()
                        .trim_start_matches(['│', '┃', '║'])
                        .trim_start()
                        .starts_with(['>', '❯'])
            })?
    } else {
        (*rules.get(rules.len().checked_sub(2)?)?, last)
    };
    if lines.len() - last > 8 || bottom - top > 15 {
        return None;
    }
    let mut content = lines[top + 1..bottom]
        .iter()
        .map(|line| {
            let line = line
                .trim()
                .trim_start_matches(['│', '┃', '║'])
                .trim_end_matches(['│', '┃', '║'])
                .trim();
            line.strip_prefix('>')
                .or_else(|| line.strip_prefix('❯'))
                .unwrap_or(line)
                .trim()
        })
        .collect::<Vec<_>>()
        .join("\n");
    if matches!(provider, Provider::Pi | Provider::Omp) {
        content = PI_NOTICE.replace_all(&compact(&content), "").into_owned();
    }
    if provider == Provider::Antigravity && AGY_MODE_HINT.is_match(content.trim()) {
        content.clear();
    }
    let placeholders = PLACEHOLDER
        .captures_iter(&content)
        .map(|captures| match captures.get(1) {
            Some(paste) => paste.as_str().to_owned(),
            None => format!("img{}", &captures[2]),
        })
        .collect();
    Some(Composer {
        content,
        placeholders,
    })
}

/// A caixa de digitar do Codex: a última linha `›` e as que continuam o texto. Parágrafo deixa linha em
/// branco dentro da caixa, então ela vai até a última linha em branco: o bloco depois dessa é a barra de
/// estado. A sugestão vem esmaecida e sai do `typed`; a captura sem estilo traz a sugestão padrão por extenso.
fn codex_composer(frame: CapturedPane<'_>) -> Option<Composer> {
    let text = frame.typed.unwrap_or(frame.text);
    let lines: Vec<&str> = text.lines().collect();
    let at = lines
        .iter()
        .rposition(|line| line.trim_start().starts_with('›'))?;
    let below = &lines[at + 1..];
    let below = &below[..below
        .iter()
        .rposition(|line| !line.trim().is_empty())
        .map_or(0, |last| last + 1)];
    let below = &below[..below
        .iter()
        .rposition(|line| line.trim().is_empty())
        .unwrap_or(below.len())];
    let mut content = vec![lines[at].trim_start().trim_start_matches('›').trim()];
    content.extend(below.iter().map(|line| line.trim()));
    let mut content = content.join("\n").trim_end().to_owned();
    if content == CODEX_PLACEHOLDER {
        content.clear();
    }
    let placeholders = CODEX_PASTE
        .captures_iter(&content)
        .map(|found| format!("pasted{}", &found[1]))
        .collect();
    Some(Composer {
        content,
        placeholders,
    })
}

pub fn own_residue(
    content: &str,
    placeholders: &BTreeSet<String>,
    text: &str,
    before: &BTreeSet<String>,
) -> bool {
    let visible = compact(content);
    let wanted = compact(text);
    if !visible.is_empty()
        && (visible == wanted || visible.chars().count() >= 12 && wanted.starts_with(&visible))
    {
        return true;
    }
    let mut rest = content.to_owned();
    for marker in PLACEHOLDER.find_iter(content) {
        if let Some(end) = content[marker.start()..].find(']') {
            rest = rest.replace(&content[marker.start()..marker.start() + end + 1], "");
        }
    }
    !placeholders.is_empty() && placeholders.is_disjoint(before) && compact(&rest).is_empty()
}

pub fn compact(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_whitespace() && !"│┃║".contains(*c))
        .collect()
}

/// Tira as sequências SGR da captura `-e`. Sem `keep_dim`, some também o texto esmaecido: é a
/// sugestão que o Claude desenha no composer vazio, e ninguém a digitou.
pub fn unstyle(styled: &str, keep_dim: bool) -> String {
    let mut out = String::with_capacity(styled.len());
    let mut dim = false;
    let mut chars = styled.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            if keep_dim || !dim || c == '\n' {
                out.push(c);
            }
            continue;
        }
        if chars.next_if_eq(&'[').is_none() {
            continue;
        }
        let mut params = String::new();
        let mut last = None;
        for n in chars.by_ref() {
            if ('@'..='~').contains(&n) {
                last = Some(n);
                break;
            }
            params.push(n);
        }
        if last != Some('m') {
            continue;
        }
        let mut codes = params
            .split([';', ':'])
            .map(|p| p.parse::<u32>().unwrap_or(0));
        while let Some(code) = codes.next() {
            match code {
                0 | 22 => dim = false,
                2 => dim = true,
                // O `2` de `38;2;r;g;b` é truecolor, não esmaecido.
                38 | 48 | 58 => match codes.next() {
                    Some(5) => {
                        codes.next();
                    }
                    Some(2) => {
                        codes.nth(2);
                    }
                    _ => {}
                },
                _ => {}
            }
        }
    }
    out
}
