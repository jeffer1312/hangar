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

const EXTS: [&str; 26] = ["png", "jpg", "jpeg", "gif", "webp", "svg", "avif", "bmp", "mp4", "mov", "webm", "mkv", "m4v", "avi",
    "mp3", "wav", "m4a", "ogg", "flac", "aac", "html", "htm", "pdf", "tiff", "tif", "json"];

fn ends_path(text: &str, end: usize) -> bool {
    let rest = &text[end..];
    match rest.chars().next() {
        None => true,
        Some('.') => rest[1..].chars().next().is_none_or(char::is_whitespace),
        Some(c) => c.is_whitespace() || ")]\"'`,;:*".contains(c),
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

/// Nome do comando digitado enquanto ainda não há argumento (`/nom` → `nom`).
pub fn slash_query(text: &str) -> Option<&str> {
    let rest = text.trim_start().strip_prefix('/')?;
    (!rest.chars().any(char::is_whitespace)).then_some(rest)
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

/// Comandos cuja tela existe só no terminal ou noutro painel; a janela nativa avisa em vez de simular.
pub fn needs_other_surface(provider: &str, command: &CommandInfo) -> bool {
    let builtin = provider != "codex" || command.source == "builtin";
    (builtin && matches!(command.name.as_str(), "model" | "effort")) || (provider == "claude" && command.name == "btw")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(name: &str) -> CommandInfo { CommandInfo { name: name.into(), ..Default::default() } }

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
    }

    #[test]
    fn slash_suggestions_rank_prefix_first_and_stop_at_arguments() {
        let list = vec![command("review"), command("clear"), command("compact")];
        assert_eq!(slash_query("  /co"), Some("co"));
        assert_eq!(slash_query("/compact agora"), None);
        let names: Vec<_> = suggestions(&list, "c").iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["clear", "compact"]);
        assert_eq!(typed_command(&list, "/clear já").map(|c| c.name.as_str()), Some("clear"));
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
