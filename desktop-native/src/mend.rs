//! Fecha, só no que é desenhado, a marcação que o streaming deixou aberta no fim da resposta.
//!
//! Enquanto a resposta chega, um `**negrito`, `` `código ``, `~~risco` ou `[link](url…` ainda sem o
//! fechamento aparece com os marcadores à mostra; quando o fechamento chega eles somem e a linha
//! reflui. Fechar provisoriamente o fim deixa o estilo estável desde o primeiro caractere. O transcript
//! e a mensagem final não passam por aqui.
//!
//! A leitura é aproximada de propósito: prefere ficar perto do resultado final a resolver cada caso de
//! CommonMark, porque o próximo pedaço (ou a mensagem final) corrige qualquer engano.

use std::borrow::Cow;

/// Destino do link cuja URL ainda está chegando: não é http(s), então o clique não abre nada.
pub const PENDING_LINK: &str = "hangar-pending:";

/// O texto com a marcação aberta do último bloco fechada; o próprio texto quando nada está aberto.
pub fn close_hanging(text: &str) -> Cow<'_, str> {
    let Some(start) = tail_start(text) else { return Cow::Borrowed(text) };
    match mend_block(&text[start..]) {
        Some(block) => Cow::Owned(format!("{}{block}", &text[..start])),
        None => Cow::Borrowed(text),
    }
}

/// Início do último bloco, onde a marcação pode estar aberta: uma linha em branco encerra o bloco e a
/// marcação dele fica literal. `None` quando o fim está num bloco que não tem o que fechar: cerca de
/// código aberta (já desenha como código), tabela ou código recuado.
fn tail_start(text: &str) -> Option<usize> {
    let mut fence: Option<(char, usize)> = None;
    let mut start = 0;
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start();
        let run = |ch: char| trimmed.chars().take_while(|&c| c == ch).count();
        match fence {
            Some((ch, len)) => if run(ch) >= len && trimmed.trim_start_matches(ch).trim().is_empty() { fence = None; start = offset + line.len(); },
            None => {
                if let Some(ch) = ['`', '~'].into_iter().find(|&ch| run(ch) >= 3) { fence = Some((ch, run(ch))); }
                else if line.trim().is_empty() { start = offset + line.len(); }
            }
        }
        offset += line.len();
    }
    let block = &text[start..];
    let first = block.lines().next().unwrap_or("");
    let indented = first.starts_with("    ") || first.starts_with('\t');
    (fence.is_none() && !first.trim_start().starts_with('|') && !indented).then_some(start)
}

/// Um marcador de ênfase ainda sem par: o caractere, quantos restam e onde o conteúdo dele começa.
struct Open { ch: char, len: usize, at: usize }

fn mend_block(block: &str) -> Option<String> {
    if let Some(split) = setext_line(block) {
        // Um `-` ou `=` sozinho embaixo de texto vira sublinhado de título até o resto da linha chegar;
        // o espaço de largura zero desfaz essa leitura sem aparecer.
        let (head, last) = block.split_at(split);
        let head = mend_block(head).unwrap_or_else(|| head.to_owned());
        return Some(format!("{head}{last}\u{200B}"));
    }
    let chars: Vec<(usize, char)> = block.char_indices().collect();
    let at = |i: usize| chars.get(i).map(|&(_, c)| c);
    let run = |i: usize| chars[i..].iter().take_while(|&&(_, c)| c == chars[i].1).count();
    let mut opens: Vec<Open> = Vec::new();
    let mut brackets: Vec<usize> = Vec::new();
    let mut code: Option<(usize, usize)> = None;
    // Último caractere que conta como conteúdo: só se fecha um marcador que já tem algo depois dele.
    let mut content: Option<usize> = None;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i].1;
        if let Some((ticks, _)) = code {
            if c == '`' {
                let n = run(i);
                if n == ticks { code = None; } else { content = Some(i + n - 1); }
                i += n;
            } else { content = Some(i); i += 1; }
            continue;
        }
        match c {
            '\\' => { if i + 1 < chars.len() { content = Some(i + 1); } i += 2; }
            '`' => { let n = run(i); code = Some((n, i + n)); i += n; }
            '*' | '_' | '~' => { let n = run(i); emphasis(&mut opens, &mut content, &chars, i, n); i += n; }
            '[' => { brackets.push(i); i += 1; }
            ']' => {
                if let Some(open) = brackets.pop() {
                    // Ênfase aberta dentro do texto do link e não fechada ali fica literal.
                    opens.retain(|o| o.at < open);
                    if at(i + 1) == Some('(') {
                        let Some(close) = url_end(&chars, i + 2) else {
                            // A URL ainda está chegando: some da vista e o texto já aparece como link.
                            let linked = format!("{}]({PENDING_LINK})", &block[..chars[i].0]);
                            return Some(mend_block(&linked).unwrap_or(linked));
                        };
                        content = Some(close);
                        i = close + 1;
                        continue;
                    }
                }
                content = Some(i);
                i += 1;
            }
            c if c.is_whitespace() => i += 1,
            _ => { content = Some(i); i += 1; }
        }
    }
    let followed = |from: usize| content.is_some_and(|last| last >= from);
    let mut closers: Vec<(usize, String)> = opens.iter().filter(|o| followed(o.at)).map(|o| (o.at, o.ch.to_string().repeat(o.len))).collect();
    if let Some((ticks, from)) = code.filter(|&(_, from)| followed(from)) { closers.push((from, "`".repeat(ticks))); }
    if let Some(&open) = brackets.last().filter(|&&open| followed(open + 1)) { closers.push((open, format!("]({PENDING_LINK})"))); }
    if closers.is_empty() { return None; }
    // O último aberto fecha primeiro; o fechamento vai antes do espaço final, senão não fecha.
    closers.sort_by(|a, b| b.0.cmp(&a.0));
    let end = block.trim_end().len();
    Some(format!("{}{}{}", &block[..end], closers.into_iter().map(|(_, s)| s).collect::<String>(), &block[end..]))
}

/// Uma sequência de `*`, `_` ou `~` em `i`: fecha o último aberto do mesmo caractere (em parte, se o
/// fechamento chegou pela metade) e abre o que sobrar.
fn emphasis(opens: &mut Vec<Open>, content: &mut Option<usize>, chars: &[(usize, char)], i: usize, n: usize) {
    let c = chars[i].1;
    let end = i + n;
    let before = i.checked_sub(1).map(|p| chars[p].1);
    let after = chars.get(end).map(|&(_, c)| c);
    let word = |c: Option<char>| c.is_some_and(char::is_alphanumeric);
    // Risco é só `~~`; `_` e `*` sozinho dentro de palavra (`snake_case`, `2*3`) não marcam nada.
    if (c == '~' && n > 2) || (word(before) && word(after) && (c == '_' || n == 1)) { *content = Some(end - 1); return; }
    let mut rest = n;
    if before.is_some_and(|c| !c.is_whitespace()) && let Some(k) = opens.iter().rposition(|o| o.ch == c) {
        let used = rest.min(opens[k].len);
        opens[k].len -= used;
        rest -= used;
        // O que abriu depois ficou dentro do trecho fechado e segue literal.
        opens.truncate(if opens[k].len == 0 { k } else { k + 1 });
    }
    if rest == 0 { return; }
    if after.is_some_and(|c| !c.is_whitespace()) && (c != '~' || rest == 2) { opens.push(Open { ch: c, len: rest, at: end }); }
    else { *content = Some(end - 1); }
}

/// Índice do `)` que fecha a URL começada em `from`, com parênteses internos equilibrados.
fn url_end(chars: &[(usize, char)], from: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (j, &(_, c)) in chars.iter().enumerate().skip(from) {
        match c {
            '(' => depth += 1,
            ')' if depth == 0 => return Some(j),
            ')' => depth -= 1,
            _ => {}
        }
    }
    None
}

/// Byte onde começa a última linha quando ela é só `-`, `--`, `=` ou `==` embaixo de uma linha com texto.
fn setext_line(block: &str) -> Option<usize> {
    let newline = block.rfind('\n')?;
    let last = block[newline + 1..].trim_start();
    let underline = !last.is_empty() && last.len() <= 2 && (last.chars().all(|c| c == '-') || last.chars().all(|c| c == '='));
    let above = block[..newline].lines().last().is_some_and(|line| !line.trim().is_empty());
    (underline && above).then_some(newline)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[track_caller]
    fn mends(input: &str, expected: &str) { assert_eq!(close_hanging(input), expected, "{input:?}"); }

    #[track_caller]
    fn keeps(input: &str) { assert!(matches!(close_hanging(input), Cow::Borrowed(_)), "{input:?} → {:?}", close_hanging(input)); }

    #[test]
    fn closed_text_is_untouched() {
        keeps("texto sem marcação");
        keeps("**negrito** e *itálico* e `código` e ~~risco~~ e [link](https://a.b)");
        keeps("");
    }

    #[test]
    fn emphasis_closes() {
        mends("um **negr", "um **negr**");
        mends("um *itál", "um *itál*");
        mends("um __negr", "um __negr__");
        mends("um ~~risc", "um ~~risc~~");
        mends("**a *b", "**a *b***");
        mends("**a*", "**a**");
    }

    #[test]
    fn opener_without_content_waits() {
        keeps("um **");
        keeps("um ** ");
        keeps("lista:\n* ");
    }

    #[test]
    fn intraword_markers_are_literal() {
        keeps("snake_case_name");
        keeps("2*3 = 6");
        keeps("~~~ não é risco");
    }

    #[test]
    fn inline_code_closes() {
        mends("rode `cargo bu", "rode `cargo bu`");
        mends("``a ` b", "``a ` b``");
        keeps("rode `cargo`");
        // Dentro do código, `*` não abre nada.
        mends("`a **b", "`a **b`");
    }

    #[test]
    fn closer_goes_before_trailing_space() {
        mends("um **negr ", "um **negr** ");
        mends("um **negr\n", "um **negr**\n");
    }

    #[test]
    fn links_show_as_links_while_the_url_streams() {
        mends("veja [a doc](https://exa", &format!("veja [a doc]({PENDING_LINK})"));
        mends("veja [a do", &format!("veja [a do]({PENDING_LINK})"));
        mends("**veja [a](http", &format!("**veja [a]({PENDING_LINK})**"));
        keeps("veja [a](https://a.b/(x))");
        keeps("veja [1] e [2]");
    }

    #[test]
    fn only_the_last_block_is_mended() {
        keeps("um **aberto\n\nsegundo parágrafo");
        mends("um **aberto\n\nsegundo *it", "um **aberto\n\nsegundo *it*");
    }

    #[test]
    fn code_fences_and_tables_are_left_alone() {
        keeps("```rust\nlet a = **b");
        keeps("texto\n\n```\ncódigo\n```\n");
        mends("```\na\n```\nfim **ne", "```\na\n```\nfim **ne**");
        keeps("| a | **b");
        keeps("    recuado **b");
    }

    #[test]
    fn escaped_markers_are_literal() {
        keeps(r"um \*não abre");
        keeps(r"um \`não abre");
    }

    #[test]
    fn lone_dash_under_text_is_not_a_heading() {
        mends("item\n-", "item\n-\u{200B}");
        mends("**a\n-", "**a**\n-\u{200B}");
        keeps("item\n- lista");
    }

    #[test]
    fn every_prefix_is_safe() {
        let answer = "Texto com **negrito**, `código`, ~~risco~~, *ênfase* e [link](https://exemplo.com/a_b).\n\n- item **um**\n";
        for (end, _) in answer.char_indices() {
            let prefix = &answer[..end];
            let mended = close_hanging(prefix);
            // Só a URL que ainda chega sai da vista; o resto do texto recebido fica inteiro.
            let kept = mended.split_once(PENDING_LINK).map_or(prefix.trim_end(), |_| &prefix[..prefix.rfind("](").map_or(0, |at| at + 2)]);
            assert!(mended.starts_with(kept), "{prefix:?} → {mended:?}");
        }
        keeps(answer);
    }
}
