use gpui_kit::ImageFormat;
use crate::api::dto::{CommandInfo, Uploaded};

// Os parsers do core e do backend só reconhecem o marcador em português, qualquer que seja o idioma da tela.
pub const IMAGE_MARK: &str = "📎 imagem: ";
pub const FILE_MARK: &str = "📎 arquivo: ";
const SUGGEST_MAX: usize = 8;

/// `encodeURIComponent`: o backend decodifica o X-Filename com a mesma regra.
pub fn encode_component(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) { out.push(byte as char); }
        else { out.push_str(&format!("%{byte:02X}")); }
    }
    out
}

fn extension(name: &str) -> String {
    name.rsplit_once('.').map(|(_, ext)| ext.to_ascii_lowercase()).unwrap_or_default()
}

pub fn image_format(name: &str) -> Option<ImageFormat> {
    Some(match extension(name).as_str() {
        "png" => ImageFormat::Png, "jpg" | "jpeg" => ImageFormat::Jpeg, "webp" => ImageFormat::Webp,
        "gif" => ImageFormat::Gif, "bmp" => ImageFormat::Bmp, "tif" | "tiff" => ImageFormat::Tiff,
        _ => return None,
    })
}

pub fn mime_for(name: &str) -> &'static str {
    if let Some(format) = image_format(name) { return format.mime_type(); }
    match extension(name).as_str() {
        "svg" => "image/svg+xml", "pdf" => "application/pdf", "txt" | "log" => "text/plain", "md" => "text/markdown",
        "json" => "application/json", "csv" => "text/csv", "html" | "htm" => "text/html",
        "mp4" | "m4v" => "video/mp4", "mov" => "video/quicktime", "webm" => "video/webm", "mkv" => "video/x-matroska",
        "mp3" => "audio/mpeg", "wav" => "audio/wav", "ogg" => "audio/ogg", "m4a" => "audio/mp4",
        "zip" => "application/zip", _ => "application/octet-stream",
    }
}

pub fn is_audio(name: &str) -> bool { mime_for(name).starts_with("audio/") }

/// Abrir com o programa do sistema só para tipos passivos; o resto (html, svg, script, atalho) só salva.
pub fn openable(name: &str) -> bool {
    image_format(name).is_some() || matches!(extension(name).as_str(),
        "pdf" | "txt" | "log" | "md" | "json" | "csv" | "mp4" | "m4v" | "mov" | "webm" | "mkv" | "mp3" | "wav" | "ogg" | "m4a" | "flac" | "aac")
}

/// Nome de arquivo seguro para gravar no disco local: só o último componente, sem controle nem ponto inicial.
pub fn safe_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("");
    let clean: String = base.chars().filter(|c| !c.is_control() && !matches!(c, ':' | '*' | '?' | '"' | '<' | '>' | '|')).collect();
    let clean = clean.trim().trim_start_matches('.').to_owned();
    // O corte preserva a extensão: é por ela que se decide se o arquivo pode abrir.
    let clean = match clean.rsplit_once('.') {
        _ if clean.chars().count() <= 120 => clean,
        Some((stem, ext)) if !ext.is_empty() && ext.chars().count() <= 16 => {
            format!("{}.{ext}", stem.chars().take(120 - ext.chars().count() - 1).collect::<String>())
        }
        _ => clean.chars().take(120).collect(),
    };
    if clean.is_empty() { "arquivo".into() } else { clean }
}

pub fn basename(path: &str) -> &str { path.rsplit(['/', '\\']).find(|part| !part.is_empty()).unwrap_or(path) }

/// Mesma linha que o compositor web monta: legenda, " — ", e os marcadores separados por espaço.
pub fn compose_prompt(caption: &str, uploads: &[(bool, Uploaded)], speech: impl Fn(&str) -> String) -> String {
    let mut parts = Vec::new();
    for (image, upload) in uploads {
        parts.push(format!("{}{}", if *image { IMAGE_MARK } else { FILE_MARK }, upload.path));
        for frame in &upload.frames { parts.push(format!("{IMAGE_MARK}{frame}")); }
        if let Some(text) = upload.transcript.as_deref().filter(|t| !t.is_empty()) { parts.push(speech(text)); }
    }
    let caption = caption.trim();
    let attached = parts.join(" ");
    if caption.is_empty() { attached } else if attached.is_empty() { caption.to_owned() } else { format!("{caption} — {attached}") }
}

/// Legenda canônica (porta de `_cap` do core): o transcript pode gravar só ela.
pub fn caption(text: &str) -> &str {
    let at = text.find("📎").filter(|&i| text[i..].trim_start_matches("📎").trim_start().starts_with("imagem:")
        || text[i..].trim_start_matches("📎").trim_start().starts_with("arquivo:"));
    let Some(at) = at else { return text.trim(); };
    let head = text[..at].trim_end();
    head.strip_suffix('—').unwrap_or(head).trim()
}

/// Anexos de uma mensagem do usuário: legenda e nomes no cofre (porta de `parseImageMessage`, com arquivos também).
pub struct Marked { pub caption: String, pub files: Vec<(bool, String)>, pub image_marks: usize }

pub fn parse_marked(text: &str) -> Option<Marked> {
    let mut found = Vec::new();
    for (mark, image) in [("imagem:", true), ("arquivo:", false)] {
        let mut from = 0;
        while let Some(i) = text[from..].find("📎").map(|i| i + from) {
            let rest = text[i + "📎".len()..].trim_start();
            if rest.starts_with(mark) { found.push((i, image, text.len() - rest.len() + mark.len())); }
            from = i + "📎".len();
        }
    }
    if found.is_empty() { return None; }
    found.sort_by_key(|f| f.0);
    let mut files = Vec::new();
    for (n, &(_, image, start)) in found.iter().enumerate() {
        let end = found.get(n + 1).map(|f| f.0).unwrap_or(text.len());
        let value = text[start..end].trim();
        // O Claude Code apaga o caminho da foto que absorveu como anexo: marcador sem nome fica sem arquivo.
        let name = value.split_whitespace().next().map(basename).unwrap_or("");
        if !name.is_empty() { files.push((image, name.to_owned())); }
    }
    let mut caption = text[..found[0].0].trim().to_owned();
    while let Some(rest) = caption.strip_prefix("[Image #").and_then(|r| r.split_once(']')).map(|(_, r)| r.trim_start().to_owned()) { caption = rest; }
    if let Some(head) = caption.strip_suffix('—') { caption = head.trim().to_owned(); }
    let image_marks = found.iter().filter(|f| f.1).count();
    Some(Marked { caption, files, image_marks })
}

const EXTS: [&str; 36] = ["png", "jpg", "jpeg", "gif", "webp", "svg", "avif", "bmp", "mp4", "mov", "webm", "mkv", "m4v", "avi",
    "mp3", "wav", "m4a", "ogg", "flac", "aac", "html", "htm", "pdf", "tiff", "tif", "json",
    "doc", "docx", "odt", "rtf", "xls", "xlsx", "ods", "ppt", "pptx", "odp"];

fn ends_path(text: &str, end: usize) -> bool {
    let rest = &text[end..];
    match rest.chars().next() {
        None => true,
        Some('.') => rest[1..].chars().next().is_none_or(char::is_whitespace),
        Some(c) => c.is_whitespace() || ")]\"'`,;:*>".contains(c),
    }
}

// Candidato termina numa extensão conhecida seguida de fim, espaço ou delimitador (regras de `parseFilePaths`).
fn close_path(text: &str, start: usize, spaces: bool) -> Option<usize> {
    for (i, ch) in text[start..].char_indices() {
        let at = start + i;
        if ch == '\n' || ch == '`' || (!spaces && ch.is_whitespace()) { return None; }
        if ch != '.' || at == start { continue; }
        let tail = &text[at + 1..];
        for ext in EXTS {
            if tail.get(..ext.len()).is_some_and(|t| t.eq_ignore_ascii_case(ext)) && ends_path(text, at + 1 + ext.len()) {
                return Some(at + 1 + ext.len());
            }
        }
    }
    None
}

/// Caminhos citados com extensão conhecida: absolutos (`/`, `~/`) e relativos com diretório.
pub fn cited_paths(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |path: &str| if !out.iter().any(|p| p == path) { out.push(path.to_owned()); };
    let prev = |i: usize| text[..i].chars().next_back();
    let mut skip_to = 0;
    for (i, ch) in text.char_indices() {
        if i < skip_to { continue; }
        let absolute = ch == '/' || (ch == '~' && text[i + 1..].starts_with('/'));
        // `*` antes vale: caminho em negrito (`**/tmp/x.jpg**`) também é citado.
        if absolute && prev(i).is_none_or(|p| !(p.is_alphanumeric() || "_.~:/".contains(p))) {
            if let Some(end) = close_path(text, i, true) { push(&text[i..end]); skip_to = end; continue; }
        }
        let word = |c: char| c.is_alphanumeric() || "_.-".contains(c);
        if word(ch) && prev(i).is_none_or(|p| !(p.is_alphanumeric() || "_/~.:*-".contains(p))) {
            if let Some(end) = close_path(text, i, false) {
                let candidate = &text[i..end];
                if candidate.contains('/') && candidate.chars().all(|c| word(c) || c == '/') && !candidate.contains("//") {
                    push(candidate);
                    skip_to = end;
                }
            }
        }
    }
    out
}

const CODE_EXTS: &str = "svelte|tsx|ts|jsx|js|mjs|cjs|py|pas|dfm|cs|dart|md|json|yaml|yml|toml|scss|css|html|sql|sh|fish|ps1|env|lock|txt|csv|xml|ini|cfg|conf";

#[derive(Clone, Debug, PartialEq)]
pub struct CodeReference { pub path: String, pub line: Option<u32>, pub start: usize, pub end: usize }

fn code_path(path: &str, absolute: bool) -> bool {
    if path.contains("://") || path.split('/').any(|part| matches!(part, ".git" | "node_modules")) { return false; }
    let name = basename(path);
    if matches!(name, "Dockerfile" | "Makefile") { return true; }
    let Some((stem, ext)) = name.rsplit_once('.') else { return false; };
    ((absolute || !stem.is_empty()) && CODE_EXTS.split('|').any(|known| ext == known)) || (absolute && !stem.is_empty() && !stem.ends_with('.')
        && (1..=12).contains(&ext.len()) && ext.starts_with(|c: char| c.is_ascii_alphabetic())
        && ext.chars().all(|c| c.is_ascii_alphanumeric() || "_-".contains(c)))
}

/// Posições e ocorrências do core; nome puro só é reconhecido em código ou destino de link.
pub fn code_references(text: &str) -> Vec<CodeReference> {
    let mut out = Vec::new();
    let mut consumed = 0;
    for (start, ch) in text.char_indices() {
        if start < consumed { continue; }
        // Absoluto entre aspas pode ter espaço ("/home/x/Área de trabalho/a.sql"); sem espaço, a regra de baixo já pega.
        if matches!(ch, '"' | '\'') && let Some(len) = text[start + 1..].find(ch) {
            let path = &text[start + 1..start + 1 + len];
            if (path.starts_with('/') || path.starts_with("~/")) && path.contains(' ') && !path.contains(['\n', '\t']) && code_path(path, true) {
                out.push(CodeReference { path: path.to_owned(), line: None, start: start + 1, end: start + 1 + len });
                consumed = start + 1 + len;
                continue;
            }
        }
        let previous = text[..start].chars().next_back();
        let absolute = ch == '/' || (ch == '~' && text[start..].starts_with("~/"));
        let word = |c: char| c.is_ascii_alphanumeric() || "_.-".contains(c);
        if absolute {
            if previous.is_some_and(|c| c.is_alphanumeric() || "_.~:/*".contains(c)) { continue; }
        } else if !word(ch) || previous.is_some_and(|c| c.is_alphanumeric() || "_/~.:*-".contains(c)) { continue; }
        for (offset, next) in text[start..].char_indices().skip(1).chain(std::iter::once((text.len() - start, ' '))) {
            let end = start + offset;
            let path = &text[start..end];
            if ends_path(text, end) && code_path(path, absolute)
                && (absolute || (path.contains('/') && !path.contains("//") && path.chars().all(|c| word(c) || c == '/'))) {
                let (line, suffix) = citation_line(&text[end..]);
                consumed = end + suffix;
                out.push(CodeReference { path: path.to_owned(), line, start, end: consumed });
                break;
            }
            if next.is_whitespace() || "\"'`)]".contains(next) { break; }
        }
    }
    out
}

/// Caminhos absolutos sem aspas que terminam em `tail` e têm espaço no meio ("/home/j/Área de trabalho/x.sql"): o
/// leitor normal corta no espaço. Um candidato por começo possível na linha, do mais longo ao mais curto; quem confere
/// qual existe é o disco ou o backend.
pub fn spaced_paths(text: &str, tail: &str) -> Vec<String> {
    let mut out = Vec::new();
    if tail.is_empty() { return out; }
    for (at, _) in text.match_indices(tail) {
        let end = at + tail.len();
        let line_start = text[..at].rfind('\n').map_or(0, |n| n + 1);
        for (start, ch) in text[line_start..at].char_indices().map(|(ix, c)| (line_start + ix, c)) {
            let opens = ch == '/' || (ch == '~' && text[start..].starts_with("~/"));
            let previous = text[..start].chars().next_back();
            if !opens || previous.is_some_and(|c| !c.is_whitespace() && !"\"'(=`".contains(c)) { continue; }
            let path = &text[start..end];
            if path.contains(' ') && !out.iter().any(|p: &String| p == path) { out.push(path.to_owned()); }
        }
    }
    out
}

/// Pastas absolutas dos `cd` de um comando (`cd "/a b/c" && …`, `cd /x;`): caminho relativo citado depois dele é dali.
pub fn cd_dirs(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (at, _) in text.match_indices("cd ") {
        if text[..at].chars().next_back().is_some_and(|c| c.is_alphanumeric() || c == '_') { continue; }
        let rest = text[at + 3..].trim_start();
        let dir = match rest.chars().next() {
            Some(quote @ ('"' | '\'')) => rest[1..].find(quote).map(|end| &rest[1..1 + end]),
            _ => Some(rest.split(|c: char| c.is_whitespace() || ";&|)".contains(c)).next().unwrap_or("")),
        };
        if let Some(dir) = dir.map(|d| d.trim_end_matches('/')).filter(|d| (d.starts_with('/') || d.starts_with("~/")) && !d.contains('\n')) {
            if !out.iter().any(|d2: &String| d2 == dir) { out.push(dir.to_owned()); }
        }
    }
    out
}

fn citation_line(tail: &str) -> (Option<u32>, usize) {
    let digits = |s: &str| s.bytes().take_while(u8::is_ascii_digit).count();
    let Some(rest) = tail.strip_prefix(':') else { return (None, 0); };
    let count = digits(rest);
    if count == 0 { return (None, 0); }
    let mut used = count + 1;
    if let Some(column) = tail[used..].strip_prefix(':') {
        let count = digits(column);
        if count > 0 { used += count + 1; }
    }
    (rest[..count].parse::<u32>().ok().filter(|line| *line > 0), used)
}

fn inline_reference(value: &str, link: bool) -> Option<CodeReference> {
    let value = value.strip_prefix('<').and_then(|s| s.strip_suffix('>')).unwrap_or(value);
    if value.is_empty() || value.starts_with(['#', '?']) || value.contains("://") { return None; }
    let suffix = value.rfind(':').and_then(|_| value.char_indices().find_map(|(at, c)| {
        let (line, used) = citation_line(&value[at..]);
        (c == ':' && used > 0 && at + used == value.len()).then_some((at, line))
    }));
    let (path, line) = suffix.map_or((value, None), |(at, line)| (&value[..at], line));
    if path.contains(':') || (path.starts_with('.') && !path.contains('/') && path != ".env") { return None; }
    if !link && path.contains(char::is_whitespace) && !path.starts_with('/') && !path.starts_with("~/") { return None; }
    if !link {
        let candidate = format!("{}{}", if path.starts_with('/') || path.starts_with("~/") { "" } else { "/" }, path.replace(' ', "%20"));
        if !code_references(&candidate).first().is_some_and(|r| r.start == 0 && r.end == candidate.len()) { return None; }
    }
    Some(CodeReference { path: path.to_owned(), line, start: 0, end: value.len() })
}

/// Isola as citações para o plugin inline do kit sem transformar código cercado ou URLs.
pub fn citation_markdown(source: &str) -> String { citation_markdown_with(source, &|_| false) }

/// Os caminhos que viraram chip num texto já passado por `citation_markdown`.
pub fn citation_paths(markdown: &str) -> Vec<String> {
    markdown.match_indices("](hangar-file:?").filter_map(|(at, _)| {
        let rest = &markdown[at + 2..];
        let url = url::Url::parse(&rest[..rest.find(')')?]).ok()?;
        url.query_pairs().find(|(key, _)| key == "path").map(|(_, value)| value.into_owned())
    }).collect()
}

/// Como `citation_markdown`, mas o caminho que `dead` diz que não abre fica como texto: chip que não abre não se clica.
pub fn citation_markdown_with(source: &str, dead: &dyn Fn(&str) -> bool) -> String {
    let mut output = String::with_capacity(source.len());
    let mut fence = None;
    for line in source.split_inclusive('\n') {
        let trimmed = line.trim_start();
        let marker = trimmed.chars().next().filter(|c| matches!(c, '`' | '~'));
        if let Some(marker) = marker {
            let count = trimmed.chars().take_while(|c| *c == marker).count();
            if count >= 3 && line.len() - trimmed.len() <= 3 {
                match fence {
                    None => fence = Some((marker, count)),
                    Some((open, length)) if marker == open && count >= length && trimmed[count..].trim().is_empty() => fence = None,
                    _ => {},
                }
                output.push_str(line); continue;
            }
        }
        if fence.is_some() || line.starts_with("    ") || line.starts_with('\t') { output.push_str(line); continue; }
        let refs = code_references(line);
        let mut at = 0;
        while at < line.len() {
            let rest = &line[at..];
            let code = rest.starts_with('`').then(|| {
                let run = rest.bytes().take_while(|b| *b == b'`').count();
                rest[run..].find(&rest[..run]).map(|end| (run + end + run, &rest[run..run + end]))
            }).flatten();
            let link = rest.starts_with('[').then(|| {
                let close = rest.find(']')?;
                if !rest[close..].starts_with("](") { return None; }
                let end = rest[close + 2..].find(')')? + close + 2;
                Some((end + 1, &rest[close + 2..end]))
            }).flatten();
            let protected = code.or(link);
            let reference = match protected {
                Some((_, value)) => inline_reference(value, link.is_some()),
                None => refs.iter().find(|reference| reference.start == at).cloned(),
            };
            let used = protected.map(|(used, _)| used).or_else(|| reference.as_ref().map(|reference| reference.end - at));
            if let Some(used) = used {
                if let Some(reference) = reference.filter(|r| {
                    let ext = extension(&r.path);
                    !EXTS.contains(&ext.as_str()) || matches!(ext.as_str(), "json" | "tif" | "tiff")
                })
                    .filter(|_| at == 0 || !line[..at].ends_with('!'))
                    .filter(|r| !dead(&r.path)) {
                    let suffix = reference.line.map(|n| format!(":{n}")).unwrap_or_default();
                    output.push_str(&format!("[{}{}](hangar-file:?path={}&line={})", basename(&reference.path).replace('[', "\\[").replace(']', "\\]"), suffix,
                        encode_component(&reference.path).replace('(', "%28").replace(')', "%29"), reference.line.unwrap_or(0)));
                } else { output.push_str(&rest[..used]); }
                at += used;
            } else if rest.starts_with("http://") || rest.starts_with("https://") {
                let used = rest.find(|c: char| c.is_whitespace() || c == '<').unwrap_or(rest.len());
                output.push_str(&rest[..used]); at += used;
            } else {
                let ch = rest.chars().next().unwrap(); output.push(ch); at += ch.len_utf8();
            }
        }
    }
    output
}

/// URLs http(s) de imagem (`parseMediaUrls` do core, só imagem): a miniatura é buscada sem o token do servidor.
pub fn image_urls(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut skip_to = 0;
    for (i, _) in text.match_indices("http") {
        let rest = &text[i..];
        if i < skip_to || !(rest.starts_with("http://") || rest.starts_with("https://")) { continue; }
        let end = rest.find(|c: char| c.is_whitespace() || "<>\"'`])".contains(c)).unwrap_or(rest.len());
        skip_to = i + end;
        let url = rest[..end].trim_end_matches(['.', ',', ';', ':', '!', '?']);
        if image_format(url_name(url)).is_some() && !out.iter().any(|u| u == url) { out.push(url.to_owned()); }
    }
    out
}

/// Nome do arquivo de uma URL, sem consulta nem âncora.
pub fn url_name(url: &str) -> &str { basename(url.split(['?', '#']).next().unwrap_or(url)) }

/// Onde começa a palavra que termina no fim de `before`.
fn word_start(before: &str) -> usize { before.trim_end_matches(|c: char| !c.is_whitespace()).len() }

/// `/nome` sob o cursor, em qualquer ponto do texto (gêmea do `slashTokenAt` do core). `query` vai da barra até o
/// cursor e `range` é a palavra inteira; `whole` diz se a palavra é a mensagem toda, o único caso em que escolher
/// roteia o comando. Segunda barra (caminho, URL) não é nome de comando.
pub struct SlashToken<'a> { pub range: std::ops::Range<usize>, pub query: &'a str, pub whole: bool }

pub fn slash_token(text: &str, cursor: usize) -> Option<SlashToken<'_>> {
    let start = word_start(text.get(..cursor)?);
    if start >= cursor || !text[start..].starts_with('/') { return None; }
    let end = text[cursor..].find(char::is_whitespace).map_or(text.len(), |at| cursor + at);
    if !text[start + 1..end].chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | ':' | '.' | '-')) { return None; }
    let whole = text[..start].trim().is_empty() && text[end..].trim().is_empty();
    Some(SlashToken { range: start..end, query: &text[start + 1..cursor], whole })
}

/// O campo é só um `/nome` sendo digitado (nem argumento nem outro texto).
pub fn only_command(text: &str) -> bool { slash_token(text, text.len()).is_some_and(|token| token.whole) }

/// Trecho a trocar e texto que entra para completar o token com `name` (gêmea do `replaceSlashToken` do core): o espaço
/// que já vinha depois é reaproveitado, para o cursor sair da palavra e a lista não reabrir com o nome pronto.
pub fn slash_replacement(text: &str, token: &SlashToken, name: &str) -> (std::ops::Range<usize>, String) {
    let end = token.range.end + usize::from(text[token.range.end..].starts_with(' '));
    (token.range.start..end, format!("/{name} "))
}

/// Menção sob o cursor; e-mail não abre a busca de arquivos.
pub fn mention_query(text: &str, cursor: usize) -> Option<(std::ops::Range<usize>, &str)> {
    let before = text.get(..cursor)?;
    let start = word_start(before);
    let query = before.get(start..)?.strip_prefix('@')?;
    if query.contains('@') { return None; }
    Some((start..cursor, query))
}

pub fn suggestions<'a>(commands: &'a [CommandInfo], query: &str) -> Vec<&'a CommandInfo> {
    let token = query.to_lowercase();
    let mut ranked: Vec<(u8, &CommandInfo)> = commands.iter().filter_map(|c| {
        let name = c.name.to_lowercase();
        if token.is_empty() { Some((1, c)) } else if name.starts_with(&token) { Some((0, c)) } else if name.contains(&token) { Some((1, c)) } else { None }
    }).collect();
    ranked.sort_by_key(|(rank, _)| *rank);
    ranked.into_iter().take(SUGGEST_MAX).map(|(_, c)| c).collect()
}

/// Comando que o texto enviado dispara, para exigir a confirmação dos destrutivos mesmo quando digitados.
pub fn typed_command<'a>(commands: &'a [CommandInfo], text: &str) -> Option<&'a CommandInfo> {
    let name = text.trim_start().strip_prefix('/')?.split_whitespace().next()?;
    commands.iter().find(|c| c.name == name)
}

/// `/btw` (ou o `/hangar-btw` que vai à sessão sem terminal), com ou sem pergunta: respondido num painel, fora
/// da conversa.
pub fn side_question(text: &str) -> bool {
    let rest = text.trim_start();
    let rest = rest.strip_prefix("/hangar-btw").or_else(|| rest.strip_prefix("/btw"));
    rest.is_some_and(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace))
}

/// Sem terminal o Claude Code responde o `/btw` embutido como comando local antes de qualquer hook: vai como
/// `/hangar-btw`, que nenhum comando atende e o plugin do Hangar pega e descarta antes do transcript.
pub fn surface_side_question(text: &str) -> String {
    let start = text.len() - text.trim_start().len();
    match text[start..].strip_prefix("/btw") {
        Some(rest) if rest.is_empty() || rest.starts_with(char::is_whitespace) => format!("{}/hangar-btw{rest}", &text[..start]),
        _ => text.to_owned(),
    }
}

/// Comando que esta janela não atende aqui: a chave do aviso, ou `None` para enviar. `/model` e `/effort`
/// abrem tela só do terminal; `/btw` só vai com o plugin da sessão anunciando que o responde num painel,
/// senão abriria no terminal um overlay que o app não mostra.
pub fn blocked_command(provider: &str, command: &CommandInfo, btw_ready: bool) -> Option<&'static str> {
    let builtin = provider != "codex" || command.source == "builtin";
    if builtin && matches!(command.name.as_str(), "model" | "effort") { return Some("command_other_surface"); }
    (provider == "claude" && command.name == "btw" && !btw_ready).then_some("command_btw_unavailable")
}

#[cfg(test)]
mod tests {
    use super::*;
    // O glob pode trazer o `test` da gpui, que colide com o atributo padrão; o nome explícito vence o glob.
    use core::prelude::v1::test;

    #[test]
    fn citations_preserve_web_boundaries_lines_and_markdown() {
        let text = "é /tmp/a.rs:12:3, src/a.ts:7 e src/a.rs; /tmp/b.service.";
        let refs = code_references(text);
        assert_eq!(refs.iter().map(|r| (r.path.as_str(), r.line)).collect::<Vec<_>>(),
            [("/tmp/a.rs", Some(12)), ("src/a.ts", Some(7)), ("/tmp/b.service", None)]);
        assert_eq!(&text[refs[0].start..refs[0].end], "/tmp/a.rs:12:3");
        assert!(code_references("https://h/repo.git /a/node_modules/x.ts /a/.git/config.py ~/.hangar config.py").is_empty());
        let source = "Veja `main.rs:12`, /tmp/a.rs:7 e [arquivo](<src/a b.ts:3>). `settings.json`";
        let converted = citation_markdown(source);
        assert_eq!(converted.matches("hangar-file:").count(), 4);
        assert!(converted.contains("path=main.rs&line=12"));
        assert!(converted.contains("path=src%2Fa%20b.ts&line=3"));
        for source in ["```rust\n/tmp/a.rs\n```", "~~~\n/tmp/a.rs\n~~~", "    /tmp/a.rs\n",
            "https://h/a.ts", "[site](https://h/a.ts)", "![foto](/tmp/a.png)", "`foto.png`", "`word two.txt`", "`.gitignore`"] {
            assert_eq!(citation_markdown(source), source);
        }
        assert_eq!(citation_markdown("`/a/(b).rs` /a/x.ts /a/x.ts").matches("hangar-file:").count(), 2);
        assert!(citation_markdown("[run](bin/run) `.env`").contains("path=bin%2Frun"));
        assert!(citation_markdown("[nota] veja [arquivo](src/a.ts)").starts_with("[nota] veja [a.ts]"));
        let fenced = "````rust\n```\n/tmp/a.rs\n````\n";
        assert_eq!(citation_markdown(fenced), fenced);
    }

    #[test]
    fn quoted_absolute_path_keeps_its_spaces() {
        let refs = code_references("scp -q \"/home/j/Área de trabalho/ddl/02-x.sql\" vm:C:/temp && cat '~/a b/c.rs'");
        assert_eq!(refs.iter().map(|r| r.path.as_str()).collect::<Vec<_>>(), ["/home/j/Área de trabalho/ddl/02-x.sql", "~/a b/c.rs"]);
        assert!(code_references("diz \"olá mundo\" e 'it is'").is_empty());
        let found = "ok\n/home/j/Área de trabalho/ddl/02-x.sql\n";
        assert_eq!(spaced_paths(found, "/02-x.sql"), ["/home/j/Área de trabalho/ddl/02-x.sql"]);
        assert_eq!(spaced_paths(found, "trabalho/ddl/02-x.sql"), ["/home/j/Área de trabalho/ddl/02-x.sql"]);
        assert!(spaced_paths("/a/b/02-x.sql", "/02-x.sql").is_empty());
        assert_eq!(cd_dirs("cd \"/home/j/Área de trabalho/P\" && sed x ../a.pas; cd /tmp/x/; abcd /no; cd rel"),
            ["/home/j/Área de trabalho/P", "/tmp/x"]);
    }

    fn command(name: &str) -> CommandInfo { CommandInfo { name: name.into(), ..Default::default() } }

    #[test]
    fn btw_goes_only_with_the_plugin_answering_it() {
        assert_eq!(blocked_command("claude", &command("btw"), false), Some("command_btw_unavailable"));
        assert_eq!(blocked_command("claude", &command("btw"), true), None);
        assert_eq!(blocked_command("claude", &command("model"), true), Some("command_other_surface"));
        assert_eq!(blocked_command("codex", &command("btw"), false), None);
        assert_eq!(blocked_command("claude", &command("clear"), false), None);
        assert!(side_question("/btw") && side_question("  /btw o que mudou?") && side_question("/btw\nlinha"));
        assert!(!side_question("/btwx") && !side_question("fala /btw") && !side_question("/clear"));
        assert!(side_question("/hangar-btw o que mudou?"));
        assert_eq!(surface_side_question("/btw o que mudou?"), "/hangar-btw o que mudou?");
        assert_eq!(surface_side_question("  /btw"), "  /hangar-btw");
        assert_eq!(surface_side_question("/btwx a"), "/btwx a");
    }

    #[test]
    fn filename_header_matches_encode_uri_component() {
        assert_eq!(encode_component("foto 1 (cópia).png"), "foto%201%20(c%C3%B3pia).png");
        assert_eq!(encode_component("a+b&c/d.txt"), "a%2Bb%26c%2Fd.txt");
    }

    #[test]
    fn prompt_uses_backend_paths_frames_and_speech_like_web() {
        let image = Uploaded { path: "/srv/up/1-a.png".into(), frames: vec![], transcript: None };
        let video = Uploaded { path: "/srv/up/2-b.mp4".into(), frames: vec!["/srv/up/2-b-1.jpg".into()], transcript: Some("oi".into()) };
        let speech = |t: &str| format!("fala do vídeo: \"{t}\"");
        assert_eq!(compose_prompt(" olha ", &[(true, image.clone()), (false, video)], speech),
            "olha — 📎 imagem: /srv/up/1-a.png 📎 arquivo: /srv/up/2-b.mp4 📎 imagem: /srv/up/2-b-1.jpg fala do vídeo: \"oi\"");
        assert_eq!(compose_prompt("", &[(true, image)], speech), "📎 imagem: /srv/up/1-a.png");
        assert_eq!(caption("olha — 📎 imagem: /x.png"), "olha");
        assert_eq!(caption("sem anexo"), "sem anexo");
    }

    #[test]
    fn received_markers_keep_caption_and_basenames() {
        let marked = parse_marked("[Image #1] veja — 📎 imagem: /a/1.png 📎 arquivo: C:\\u\\2.zip 📎 imagem:").unwrap();
        assert_eq!(marked.caption, "veja");
        assert_eq!(marked.files, vec![(true, "1.png".into()), (false, "2.zip".into())]);
        assert!(parse_marked("texto").is_none());
    }

    #[test]
    fn cited_paths_follow_core_rules() {
        let text = "Veja /tmp/a b.png, **/tmp/x.jpg** e ./out/r.pdf. Não: https://h/x.png nem nome.png nem ~/d/v.MP4";
        assert_eq!(cited_paths(text), vec!["/tmp/a b.png", "/tmp/x.jpg", "./out/r.pdf", "~/d/v.MP4"]);
        assert_eq!(cited_paths("sub/dir/f.html fim"), vec!["sub/dir/f.html"]);
        assert!(cited_paths("veja /tmp/a.óculos e ./b.çã e /x/y.ó").is_empty());
        // Imagem em markdown: o caminho local vira anexo; a URL fica para `image_urls`.
        assert_eq!(cited_paths("![a](/tmp/a.png) ![](out/b.gif) ![r](https://h/r.png)"), vec!["/tmp/a.png", "out/b.gif"]);
    }

    #[test]
    fn cited_documents_use_save_actions_and_preserve_code_links() {
        for ext in ["pdf", "doc", "docx", "odt", "rtf", "xls", "xlsx", "ods", "ppt", "pptx", "odp"] {
            let absolute = format!("/tmp/relatório final.{ext}");
            let relative = format!("out/report.{ext}");
            let home = format!("~/out/report.{}", ext.to_uppercase());
            let source = format!("[Documento](<{absolute}>) `{relative}` {home}");
            assert_eq!(cited_paths(&source), [absolute, relative, home]);
            assert_eq!(citation_markdown(&source), source);
            assert_eq!(openable(&format!("report.{ext}")), ext == "pdf");
            let ignored = format!("https://h/report.{ext} [remoto](https://h/report.{ext}) report.{ext}");
            assert!(cited_paths(&ignored).is_empty());
        }
        let code = "[fonte](src/main.rs) `/tmp/config.py:12`";
        assert!(cited_paths(code).is_empty());
        assert_eq!(citation_markdown(code).matches("hangar-file:").count(), 2);
    }

    #[test]
    fn image_urls_take_only_remote_images() {
        let text = "![r](https://h.io/a/r.png?x=1). Veja http://h.io/b.GIF, https://h.io/doc.pdf e /tmp/c.png; de novo https://h.io/a/r.png?x=1";
        assert_eq!(image_urls(text), vec!["https://h.io/a/r.png?x=1", "http://h.io/b.GIF"]);
        assert!(image_urls("httpx://h/a.png e http:/a.png").is_empty());
    }

    #[test]
    fn slash_suggestions_rank_prefix_first_and_stop_at_arguments() {
        let list = vec![command("review"), command("clear"), command("compact")];
        assert!(only_command("  /co"));
        assert!(!only_command("/compact agora") && !only_command("/home/x/y.py") && !only_command("oi /co"));
        let names: Vec<_> = suggestions(&list, "c").iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["clear", "compact"]);
        assert_eq!(typed_command(&list, "/clear já").map(|c| c.name.as_str()), Some("clear"));
    }

    #[test]
    fn slash_token_finds_the_command_under_the_cursor_anywhere() {
        let at = |text: &str, cursor: usize| slash_token(text, cursor).map(|t| (t.range, t.query.to_owned(), t.whole));
        assert_eq!(at("/co", 3), Some((0..3, "co".into(), true)));
        assert_eq!(at("  /co", 5), Some((2..5, "co".into(), true)));
        assert_eq!(at("revise com /sim", 15), Some((11..15, "sim".into(), false)));
        assert_eq!(at("ação\n/", "ação\n/".len()), Some((7..8, "".into(), false)));
        assert_eq!(at("/pmedico:help", 13), Some((0..13, "pmedico:help".into(), true)));
        assert_eq!(at("/compact agora", 3), Some((0..8, "co".into(), false)));
        for (text, cursor) in [("abre /home/user/x", 17), ("veja https://a/b", 16), ("src/app", 7), ("/compact agora", 14),
            ("/compact ", 9), ("a /co", 2), ("", 0)] {
            assert!(slash_token(text, cursor).is_none(), "{text:?} em {cursor}");
        }
        let replace = |text: &str, cursor: usize| slash_replacement(text, &slash_token(text, cursor).unwrap(), "compact");
        assert_eq!(replace("use /co e depois", 7), (4..8, "/compact ".into()));
        assert_eq!(replace("use /co\nfim", 7), (4..7, "/compact ".into()));
    }

    #[test]
    fn mentions_preserve_unicode_and_surrounding_text() {
        let text = "ação @src depois";
        let cursor = text.find(" depois").unwrap();
        let (range, query) = mention_query(text, cursor).unwrap();
        assert_eq!(query, "src");
        let mut replaced = text.to_owned();
        replaced.replace_range(range, "@src/main.rs");
        assert_eq!(replaced, "ação @src/main.rs depois");
        assert_eq!(mention_query("@", 1), Some((0..1, "")));
        assert!(mention_query("a@b", 3).is_none());
        assert!(mention_query("/co", 3).is_none());
        assert!(mention_query("@src ", 5).is_none());
        assert!(mention_query("é", 1).is_none());
    }

    #[test]
    fn local_names_never_escape_the_folder_and_active_docs_do_not_open() {
        assert_eq!(safe_name("../../.bashrc"), "bashrc");
        assert_eq!(safe_name("C:\\x\\a<b>.txt"), "ab.txt");
        assert!(openable("r.pdf") && !openable("p.html") && !openable("i.svg") && !openable("run.desktop"));
        // O corte nunca troca a extensão: o nome gravado é o mesmo tipo que o botão decidiu abrir.
        let tricky = safe_name(&format!("{}.html.pdf", "a".repeat(115)));
        assert!(tricky.ends_with(".pdf") && tricky.chars().count() == 120);
        let odd = safe_name(&format!("{}.html.{}", "a".repeat(115), "p".repeat(20)));
        assert!(!openable(&odd));
        let long = safe_name(&format!("{}.pdf", "b".repeat(200)));
        assert!(long.ends_with(".pdf") && long.chars().count() == 120);
    }
}
