//! Leituras de mídia da sessão que o chat pede sob demanda: etapas ao vivo de uma ferramenta MCP
//! (`tool-progress`) e a imagem colada no terminal, guardada em base64 no transcript.
use std::{
    io::{BufRead, BufReader},
    net::SocketAddr,
    path::Path,
    sync::Arc,
};

use axum::{
    body::Body,
    extract::{ConnectInfo, Path as RoutePath, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::Response,
};
use serde_json::{Value, json};

use crate::{
    routes::{AppState, INFO_REASON, pass, route_failed, who},
    session_write::{detail_body, json_response},
    side::Binding,
};

const STEPS_KEPT: usize = 200;
const MESSAGE_CHARS: usize = 500;

/// `toolu_` + 1 a 80 de `[A-Za-z0-9_-]`, a linha inteira: o id vira nome de arquivo.
pub fn valid_tool_use_id(id: &str) -> bool {
    id.strip_prefix("toolu_").is_some_and(|rest| {
        (1..=80).contains(&rest.len()) && rest.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    })
}

/// `str.splitlines()` do Python: quebra também em `\r`, `\v`, `\f`, `\x1c`-`\x1e`, `\x85`, U+2028/9.
fn py_splitlines(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if matches!(c, '\n' | '\r' | '\x0b' | '\x0c' | '\x1c' | '\x1d' | '\x1e' | '\u{85}' | '\u{2028}' | '\u{2029}') {
            out.push(&text[start..i]);
            let mut end = i + c.len_utf8();
            if c == '\r' && chars.peek().is_some_and(|&(_, n)| n == '\n') {
                chars.next();
                end += 1;
            }
            start = end;
        }
    }
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}

/// As etapas gravadas: as últimas 200 linhas (contadas antes de descartar as inválidas), cada
/// mensagem cortada em 500 caracteres. Linha que não é UTF-8 conta como uma linha inválida.
pub fn parse_steps(bytes: &[u8]) -> Vec<Value> {
    let mut chunks: Vec<&[u8]> = bytes.split(|&b| b == b'\n').collect();
    if chunks.last().is_some_and(|c| c.is_empty()) {
        chunks.pop();
    }
    let mut lines: Vec<Option<&str>> = Vec::new();
    for chunk in chunks {
        match std::str::from_utf8(chunk) {
            Ok("") => lines.push(Some("")),
            Ok(text) => lines.extend(py_splitlines(text).into_iter().map(Some)),
            Err(_) => lines.push(None),
        }
    }
    let skip = lines.len().saturating_sub(STEPS_KEPT);
    lines[skip..]
        .iter()
        .flatten()
        .filter_map(|line| {
            let Ok(Value::Object(mut item)) = serde_json::from_str::<Value>(line) else { return None };
            let Some(Value::String(message)) = item.remove("message") else { return None };
            let message: String = message.chars().take(MESSAGE_CHARS).collect();
            Some(json!({"t": item.remove("t").unwrap_or(Value::Null), "message": message}))
        })
        .collect()
}

pub async fn tool_progress(
    State(st): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    RoutePath((name, id)): RoutePath<(String, String)>,
    req: Request,
) -> Response {
    let (fwd, caller) = match who(&st, peer, &req).await { Ok(value) => value, Err(e) => return crate::auth::unavailable(e) };
    if !caller.is_some_and(|c| c.can(&name, req.method())) {
        return pass(&st, req, &fwd).await;
    }
    serve_tool_progress(&st, &name, &id, req.headers()).await
}

async fn serve_tool_progress(st: &AppState, name: &str, id: &str, headers: &HeaderMap) -> Response {
    if !valid_tool_use_id(id) {
        return json_response(StatusCode::BAD_REQUEST, json!({"detail": "invalid tool_use_id"}));
    }
    let file = st.hangar_dir.join("tool-progress").join(format!("{id}.jsonl"));
    match tokio::fs::read(&file).await {
        Ok(bytes) => json_response(StatusCode::OK, Value::Array(parse_steps(&bytes))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => json_response(StatusCode::OK, json!([])),
        Err(_) => route_failed(st, headers, "rust.tool_progress_failed", name, "tool_progress_io", "as etapas da ferramenta não puderam ser lidas"),
    }
}

/// `base64.b64decode` sem `validate`: ignora o que não é do alfabeto; `=` só vale depois do último
/// caractere de dado, e falta de preenchimento é erro.
pub fn py_b64decode(text: &str) -> Option<Vec<u8>> {
    use base64::Engine;
    let mut data = Vec::with_capacity(text.len());
    let mut pads = 0;
    for b in text.bytes() {
        if b == b'=' {
            pads += 1;
        } else if b.is_ascii_alphanumeric() || b == b'+' || b == b'/' {
            data.push(b);
            pads = 0;
        }
    }
    let ok = match data.len() % 4 {
        0 => true,
        2 => pads >= 2,
        3 => pads >= 1,
        _ => false,
    };
    ok.then(|| base64::engine::general_purpose::STANDARD_NO_PAD.decode(&data).ok()).flatten()
}

/// Só imagem raster passa: SVG executa script e o resto não é imagem.
fn served_media(media: Option<&str>) -> &'static str {
    match media.map(str::to_ascii_lowercase).as_deref() {
        None => "image/png",
        Some("image/png") => "image/png",
        Some("image/jpeg") => "image/jpeg",
        Some("image/gif") => "image/gif",
        Some("image/webp") => "image/webp",
        Some(_) => "application/octet-stream",
    }
}

/// A `idx`-ésima imagem base64 de topo da mensagem `uuid`: a primeira linha com esse uuid decide.
pub fn find_image(jsonl: &Path, uuid: &str, idx: i64) -> std::io::Result<Option<(Vec<u8>, &'static str)>> {
    let file = match std::fs::File::open(jsonl) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let needle = memchr::memmem::Finder::new(uuid.as_bytes());
    let mut reader = BufReader::new(file);
    let mut line = Vec::new();
    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line)? == 0 {
            return Ok(None);
        }
        // Só a linha que contém o uuid vira JSON: decodificar todas custava segundos por imagem.
        if needle.find(&line).is_none() {
            continue;
        }
        let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(&String::from_utf8_lossy(&line)) else { continue };
        if obj.get("uuid").and_then(Value::as_str) != Some(uuid) {
            continue;
        }
        let Some(content) = obj.get("message").and_then(|m| m.get("content")).and_then(Value::as_array) else {
            return Ok(None);
        };
        let images: Vec<&Value> = content.iter().filter(|it| it.get("type").and_then(Value::as_str) == Some("image")).collect();
        let Some(image) = usize::try_from(idx).ok().and_then(|i| images.get(i)) else { return Ok(None) };
        let source = image.get("source");
        let Some(data) = source.and_then(|s| s.get("data")).and_then(Value::as_str) else { return Ok(None) };
        let media = served_media(source.and_then(|s| s.get("media_type")).and_then(Value::as_str));
        return Ok(py_b64decode(data).map(|raw| (raw, media)));
    }
}

fn int_parsing(input: &str) -> Response {
    json_response(StatusCode::UNPROCESSABLE_ENTITY, json!({"detail": [{"type": "int_parsing", "loc": ["path", "idx"],
        "msg": "Input should be a valid integer, unable to parse string as an integer", "input": input}]}))
}

pub async fn transcript_image(
    State(st): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    RoutePath((name, uuid, idx)): RoutePath<(String, String, String)>,
    req: Request,
) -> Response {
    let (fwd, caller) = match who(&st, peer, &req).await { Ok(value) => value, Err(e) => return crate::auth::unavailable(e) };
    // A imagem de uma conversa transferida é lida pelo Python, como o histórico dela.
    if !caller.is_some_and(|c| c.can(&name, req.method())) || uuid.starts_with("transfer:") {
        return pass(&st, req, &fwd).await;
    }
    serve_transcript_image(&st, &name, uuid, &idx, req.headers()).await
}

/// O 503 `erro_lista_indisponivel` do Python, com a causa no diário.
fn list_unavailable(st: &AppState, event: &'static str, name: &str, code: &'static str, reason: &'static str) -> Response {
    st.diag.report(event, name, code, reason);
    json_response(StatusCode::SERVICE_UNAVAILABLE, detail_body("erro_lista_indisponivel", "a lista de sessões está indisponível", json!({"detalhe": code})))
}

async fn serve_transcript_image(st: &AppState, name: &str, uuid: String, idx: &str, headers: &HeaderMap) -> Response {
    let Ok(idx) = idx.trim().parse::<i64>() else { return int_parsing(idx) };
    let info = match st.info(name).await {
        Ok(info) => info,
        Err(e) => return list_unavailable(st, "rust.transcript_image_failed", name, e.0, INFO_REASON),
    };
    let Some(jsonl) = info.as_ref().and_then(|i| i.jsonl.clone()) else {
        return json_response(StatusCode::NOT_FOUND, detail_body("erro_sessao_inexistente", "session or transcript not found", json!({})));
    };
    // Com o chat ao vivo aberto, vale o transcript que o hub já confirmou.
    let fresh = info.as_ref().and_then(Binding::from_info);
    let jsonl = st.side.hubs.bound(name)
        .filter(|b| fresh.as_ref().is_some_and(|f| !b.changes_at_once(f)))
        .map_or(jsonl, |b| b.jsonl);
    let found = tokio::task::spawn_blocking(move || find_image(&jsonl, &uuid, idx)).await;
    match found {
        Ok(Ok(Some((raw, media)))) => {
            let mut response = Response::new(Body::from(raw));
            let headers = response.headers_mut();
            headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(media));
            headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("max-age=31536000, immutable"));
            headers.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
            response
        }
        Ok(Ok(None)) => json_response(StatusCode::NOT_FOUND, detail_body("erro_imagem_nao_encontrada", "image not found", json!({}))),
        Ok(Err(_)) => route_failed(st, headers, "rust.transcript_image_failed", name, "transcript_image_io", "o transcript não pôde ser lido"),
        Err(_) => route_failed(st, headers, "rust.transcript_image_failed", name, "transcript_image_panic", "a leitura da imagem caiu no servidor"),
    }
}

const OUTPUT_TAIL: u64 = 16_000;
const COMMAND_CHARS: usize = 200_000;
/// Cada cartão de Bash aberto pergunta a cada 2 s: a árvore de processos é relida no máximo nisso.
const SCAN_TTL: std::time::Duration = std::time::Duration::from_secs(2);

/// `<tmp>/claude-<uid>/…/tasks/<id>.output` no Linux, `%TEMP%\claude\…\tasks\<id>.output` no Windows.
pub fn is_task_output(path: &Path) -> bool {
    path.extension().is_some_and(|e| e == "output")
        && path.parent().and_then(Path::file_name).is_some_and(|n| n == "tasks")
        && path.components().any(|c| c.as_os_str().to_string_lossy().starts_with("claude"))
}

/// Os últimos `max` bytes, com o caractere cortado no começo trocado por U+FFFD. Arquivo que sumiu
/// (o comando acabou e o Claude Code apagou) é `None`; outro erro de leitura é erro.
pub fn read_tail(path: &Path, max: u64) -> Result<Option<String>, &'static str> {
    use std::io::{Read, Seek, SeekFrom};
    let read = || -> std::io::Result<String> {
        let mut file = std::fs::File::open(path)?;
        let len = file.metadata()?.len();
        file.seek(SeekFrom::Start(len.saturating_sub(max)))?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    };
    match read() {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err("bash_output_file_unreadable"),
    }
}

/// Para onde vai a saída padrão do processo.
#[cfg(target_os = "linux")]
pub fn stdout_files(pid: i64) -> Result<Vec<std::path::PathBuf>, &'static str> {
    Ok(std::fs::read_link(format!("/proc/{pid}/fd/1")).into_iter().collect())
}

/// No Windows não há descritor 1 para ler: vale todo arquivo de disco que o processo tem aberto.
/// A pergunta pelo tipo e pelo nome de um handle de pipe pode não voltar nunca: ela roda numa thread
/// com prazo, uma por vez, e a que travou segura a vaga (as seguintes recusam com código).
#[cfg(windows)]
pub fn stdout_files(pid: i64) -> Result<Vec<std::path::PathBuf>, &'static str> {
    use std::sync::atomic::{AtomicBool, Ordering};
    static BUSY: AtomicBool = AtomicBool::new(false);
    if BUSY.swap(true, Ordering::AcqRel) {
        return Err("bash_output_handles_busy");
    }
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(open_files::of(pid));
        BUSY.store(false, Ordering::Release);
    });
    rx.recv_timeout(std::time::Duration::from_secs(2)).map_err(|_| "bash_output_handles_timeout")
}

// shortcut: no macOS a saída ao vivo do Bash fica vazia; ler os arquivos abertos por libproc quando houver servidor macOS.
#[cfg(not(any(target_os = "linux", windows)))]
pub fn stdout_files(_pid: i64) -> Result<Vec<std::path::PathBuf>, &'static str> {
    Ok(Vec::new())
}

/// A saída ao vivo de um Bash da sessão `sid`: só os processos abaixo do agente dela contam, e o
/// comando é o texto do `eval` da linha de comando do shell.
pub fn bash_output(dirs: &[std::path::PathBuf], sid: &str, command: &str, procs: &dyn crate::list::procs::ProcessView) -> Result<Option<String>, &'static str> {
    let wanted = command.trim();
    if wanted.is_empty() || sid.is_empty() {
        return Ok(None);
    }
    let roots = crate::list::facts_files::native_pids(dirs, sid);
    if roots.is_empty() {
        return Ok(None);
    }
    let children = procs.children(SCAN_TTL).map_err(|_| "bash_output_procs_unreadable")?;
    let mut queue: Vec<i64> = roots.iter().filter_map(|r| children.get(r)).flatten().copied().collect();
    let mut seen = std::collections::HashSet::new();
    while let Some(pid) = queue.pop() {
        if !seen.insert(pid) {
            continue;
        }
        queue.extend(children.get(&pid).into_iter().flatten().copied());
        if crate::state::shells::requested(&procs.argv(pid).join(" ")) != wanted {
            continue;
        }
        if let Some(file) = stdout_files(pid)?.into_iter().find(|f| is_task_output(f)) {
            return read_tail(&file, OUTPUT_TAIL);
        }
    }
    Ok(None)
}

/// `psutil.Process.open_files()`: os handles do processo, duplicados aqui, e só os de arquivo em
/// disco têm o caminho perguntado (pipe e console nunca chegam no `GetFinalPathNameByHandleW`).
#[cfg(windows)]
mod open_files {
    use std::{ffi::OsString, os::windows::ffi::OsStringExt, path::PathBuf, ptr::null_mut};
    use windows_sys::{
        Wdk::System::SystemInformation::NtQuerySystemInformation,
        Win32::{
            Foundation::{CloseHandle, DUPLICATE_SAME_ACCESS, DuplicateHandle, HANDLE, STATUS_INFO_LENGTH_MISMATCH},
            Storage::FileSystem::{FILE_NAME_NORMALIZED, FILE_TYPE_DISK, GetFileType, GetFinalPathNameByHandleW, VOLUME_NAME_DOS},
            System::Threading::{GetCurrentProcess, OpenProcess, PROCESS_DUP_HANDLE},
        },
    };

    const SYSTEM_EXTENDED_HANDLE_INFORMATION: i32 = 64;

    /// `SYSTEM_HANDLE_TABLE_ENTRY_INFO_EX`.
    #[repr(C)]
    struct Entry {
        object: *mut core::ffi::c_void,
        pid: usize,
        handle: usize,
        access: u32,
        backtrace: u16,
        type_index: u16,
        attributes: u32,
        reserved: u32,
    }

    struct Owned(HANDLE);
    impl Drop for Owned {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.0) };
        }
    }

    /// A tabela de handles da máquina; cresce até caber (ela muda entre uma chamada e outra).
    fn table() -> Option<Vec<u64>> {
        let mut buffer = vec![0u64; 1 << 17];
        for _ in 0..8 {
            let mut needed = 0u32;
            let bytes = u32::try_from(buffer.len() * 8).ok()?;
            let status = unsafe { NtQuerySystemInformation(SYSTEM_EXTENDED_HANDLE_INFORMATION, buffer.as_mut_ptr().cast(), bytes, &mut needed) };
            if status == STATUS_INFO_LENGTH_MISMATCH {
                buffer = vec![0u64; (needed as usize).max(buffer.len() * 8) / 8 + (1 << 14)];
                continue;
            }
            return (status >= 0).then_some(buffer);
        }
        None
    }

    fn path_of(handle: HANDLE) -> Option<PathBuf> {
        if unsafe { GetFileType(handle) } != FILE_TYPE_DISK {
            return None;
        }
        let mut wide = vec![0u16; 32768];
        let length = unsafe { GetFinalPathNameByHandleW(handle, wide.as_mut_ptr(), wide.len() as u32, FILE_NAME_NORMALIZED | VOLUME_NAME_DOS) } as usize;
        if length == 0 || length >= wide.len() {
            return None;
        }
        let text = OsString::from_wide(&wide[..length]).to_string_lossy().into_owned();
        Some(PathBuf::from(match text.strip_prefix(r"\\?\UNC\") {
            Some(rest) => format!(r"\\{rest}"),
            None => text.strip_prefix(r"\\?\").map(str::to_owned).unwrap_or(text),
        }))
    }

    pub fn of(pid: i64) -> Vec<PathBuf> {
        let Ok(pid) = u32::try_from(pid) else { return Vec::new() };
        let process = unsafe { OpenProcess(PROCESS_DUP_HANDLE, 0, pid) };
        if process.is_null() {
            return Vec::new();
        }
        let process = Owned(process);
        let Some(buffer) = table() else { return Vec::new() };
        let count = buffer[0] as usize;
        let room = (buffer.len() * 8 - 16) / std::mem::size_of::<Entry>();
        let entries = unsafe { std::slice::from_raw_parts(buffer.as_ptr().add(2).cast::<Entry>(), count.min(room)) };
        let mut out = Vec::new();
        for entry in entries.iter().filter(|e| e.pid == pid as usize) {
            let mut copy: HANDLE = null_mut();
            let ok = unsafe { DuplicateHandle(process.0, entry.handle as HANDLE, GetCurrentProcess(), &mut copy, 0, 0, DUPLICATE_SAME_ACCESS) };
            if ok == 0 {
                continue;
            }
            let copy = Owned(copy);
            if let Some(path) = path_of(copy.0)
                && !out.contains(&path)
            {
                out.push(path);
            }
        }
        out
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct BashBody {
    command: String,
}

fn invalid_body(msg: &str) -> Response {
    json_response(StatusCode::UNPROCESSABLE_ENTITY, json!({"detail": [{"type": "value_error", "loc": ["body"], "msg": msg, "input": null}]}))
}

pub async fn bash_output_route(
    State(st): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    RoutePath(name): RoutePath<String>,
    req: Request,
) -> Response {
    let (fwd, caller) = match who(&st, peer, &req).await { Ok(value) => value, Err(e) => return crate::auth::unavailable(e) };
    if !caller.is_some_and(|c| c.can(&name, req.method())) {
        return pass(&st, req, &fwd).await;
    }
    serve_bash_output(&st, &name, req).await
}

async fn serve_bash_output(st: &AppState, name: &str, req: Request) -> Response {
    // Só confere a troca de conversa e solta, antes do corpo, como o `_transfer_check`.
    if let Some(runtime) = st.state.runtime.get()
        && runtime.ingress().enter(name, st.write_gate_wait).await.is_err()
    {
        return crate::diag::coded(json_response(StatusCode::CONFLICT, crate::session_write::busy_body()), "session_transfer_busy");
    }
    let headers = req.headers().clone();
    let Ok(bytes) = axum::body::to_bytes(req.into_body(), 2 * 1024 * 1024).await else {
        return invalid_body("corpo grande demais");
    };
    let command = match serde_json::from_slice::<BashBody>(&bytes) {
        Ok(body) if body.command.chars().count() <= COMMAND_CHARS => body.command,
        Ok(_) => return invalid_body("command passa de 200000 caracteres"),
        Err(e) => return invalid_body(&e.to_string()),
    };
    let info = match st.info(name).await {
        Ok(info) => info,
        Err(e) => return list_unavailable(st, "rust.bash_output_failed", name, e.0, INFO_REASON),
    };
    let Some(sid) = info.map(|i| i.session_key).filter(|s| !s.is_empty()) else {
        return json_response(StatusCode::OK, json!({"text": null}));
    };
    let list = st.list.clone();
    let found = tokio::task::spawn_blocking(move || {
        let (dirs, _) = list.state_dirs_blocking().map_err(|e| e.code)?;
        bash_output(&dirs, &sid, &command, &*list.env().procs)
    })
    .await;
    match found {
        Ok(Ok(text)) => json_response(StatusCode::OK, json!({"text": text})),
        Ok(Err(code)) if code.starts_with("bash_output_") => {
            st.diag.report("rust.bash_output_failed", name, code, "a saída do comando não pôde ser lida");
            json_response(StatusCode::SERVICE_UNAVAILABLE, detail_body(code, "a saída do comando não pôde ser lida", json!({})))
        }
        Ok(Err(code)) => list_unavailable(st, "rust.bash_output_failed", name, code, "pastas das contas indisponíveis"),
        Err(_) => route_failed(st, &headers, "rust.bash_output_failed", name, "bash_output_panic", "a leitura da saída caiu no servidor"),
    }
}

/// Anexos, `/transcribe`, saída de ferramenta e imagem do transcript (fora a da conversa transferida).
pub fn is_media(method: &axum::http::Method, path: &str) -> bool {
    use axum::http::Method;
    let Some((_, tail)) = path.strip_prefix("/api/sessions/").and_then(|p| p.split_once('/')) else { return false };
    crate::uploads::http::matches(method, path)
        || (*method == Method::POST && matches!(tail, "bash-output" | "transcribe"))
        || (*method == Method::GET && (tail.starts_with("tool-progress/") || tail.starts_with("transcript-image/")))
}

/// Rotas de mídia da sessão que chegam pelas portas do Python (Connect, convidado com login), já
/// autenticadas lá. `guest` diz se quem pediu é convidado.
pub async fn serve_forwarded(st: &Arc<AppState>, path: &str, req: Request, guest: bool) -> Option<Response> {
    let decode = |s: &str| percent_encoding::percent_decode_str(s).decode_utf8().ok().map(|s| s.into_owned());
    let (name, tail) = path.strip_prefix("/api/sessions/")?.split_once('/')?;
    let name = decode(name)?;
    let method = req.method().clone();
    let get = method == axum::http::Method::GET;
    let parts: Vec<&str> = tail.split('/').collect();
    Some(match (parts.as_slice(), method == axum::http::Method::POST) {
        (["tool-progress", id], false) if get => serve_tool_progress(st, &name, &decode(id)?, req.headers()).await,
        (["transcript-image", uuid, idx], false) if get => {
            let uuid = decode(uuid)?;
            // A conversa transferida o Python atende sem repassar.
            if uuid.starts_with("transfer:") {
                return None;
            }
            serve_transcript_image(st, &name, uuid, &decode(idx)?, req.headers()).await
        }
        (["bash-output"], true) => serve_bash_output(st, &name, req).await,
        (["transcribe"], true) => crate::uploads::transcribe::serve(st, &name, req, guest).await,
        _ if crate::uploads::http::matches(&method, path) => crate::uploads::http::public(st.clone(), req).await,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn splitlines_matches_python() {
        assert_eq!(py_splitlines("a\r\nb\rc\u{2028}d\x1ce"), ["a", "b", "c", "d", "e"]);
        assert_eq!(py_splitlines("a\r"), ["a"]);
        assert_eq!(py_splitlines("\r"), [""]);
    }

    #[test]
    fn b64_matches_python_cases() {
        for (input, expected) in [("QUJD", Some(&b"ABC"[..])), ("QUJ", None), ("QUJD=", Some(b"ABC")), ("QQ=", None),
            ("Q===", None), ("QQ==\n", Some(b"A")), ("=QUJD", Some(b"ABC")), ("QU=JD", Some(b"ABC")),
            ("QUJDRA==extra", None), ("QQ==QQ==", Some(b"A\x04\x10"))] {
            assert_eq!(py_b64decode(input).as_deref(), expected, "{input}");
        }
    }

    proptest! {
        #[test]
        fn steps_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
            let _ = parse_steps(&bytes);
        }

        #[test]
        fn image_scan_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..512), idx in -2i64..4) {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("t.jsonl");
            std::fs::write(&path, &bytes).unwrap();
            let _ = find_image(&path, "u", idx);
            let _ = py_b64decode(&String::from_utf8_lossy(&bytes));
        }
    }
}
