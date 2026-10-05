//! Importa o login do Chrome real do usuário para o navegador embutido: cookies por CDP (já
//! decifrados, sem ler o arquivo `Cookies` em disco) e, no Linux, as senhas salvas para preencher.
//!
//! O Chrome 136+ não aceita mais `--remote-debugging-port` no perfil padrão. O que liga é o toggle
//! em `chrome://inspect/#remote-debugging`, que grava `DevToolsActivePort` na raiz do perfil (linha
//! 1 = porta, linha 2 = caminho do WebSocket do browser). A porta fixa fica como plano B.
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::time::Duration;

use base64::{Engine, engine::general_purpose::STANDARD};
use ring::rand::{SecureRandom, SystemRandom};
use serde_json::{Value, json};

pub const ACTIVATE_PAGE: &str = "chrome://inspect/#remote-debugging";
const IO_TIMEOUT: Duration = Duration::from_secs(15);
/// Dump de cookies de um perfil antigo passa de 1 MB; teto generoso, só pra barrar resposta absurda.
const MAX_FRAME: usize = 64 * 1024 * 1024;

pub enum ImportError {
    /// Chrome sem depuração remota ligada (ou nenhum candidato conectou).
    ChromeClosed,
    /// A porta fixa tem um Chrome de automação, não o do usuário.
    Headless,
    /// Falha ao falar CDP.
    Failed(String),
}

impl ImportError {
    /// Chave de i18n (`native_<key>`) do status mostrado ao usuário.
    pub fn status_key(&self) -> &'static str {
        match self {
            Self::ChromeClosed => "browser_cookies_chrome_off",
            Self::Headless => "browser_cookies_headless",
            Self::Failed(_) => "browser_cookies_error",
        }
    }

    pub fn detail(&self) -> &str {
        match self {
            Self::Failed(e) => e,
            _ => "",
        }
    }
}

/// Raízes de perfil onde o Chrome grava `DevToolsActivePort`, na ordem em que vale procurar.
/// Este módulo não compila no macOS, então o não-Windows aqui é o Linux.
fn profile_roots() -> Vec<PathBuf> {
    let home = dirs_home();
    #[cfg(target_os = "windows")]
    {
        let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
            .unwrap_or_else(|| home.join("AppData").join("Local"));
        vec![
            local.join("Google").join("Chrome").join("User Data"),
            local.join("Microsoft").join("Edge").join("User Data"),
        ]
    }
    #[cfg(not(target_os = "windows"))]
    {
        let cfg = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".config"));
        vec![
            cfg.join("google-chrome"),
            cfg.join("chromium"),
            cfg.join("BraveSoftware").join("Brave-Browser"),
        ]
    }
}

fn dirs_home() -> PathBuf {
    std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from).unwrap_or_default()
}

/// `DevToolsActivePort` de um perfil -> URL do WebSocket do browser, ou `None`.
fn endpoint_from_profile(root: &PathBuf) -> Option<String> {
    let text = std::fs::read_to_string(root.join("DevToolsActivePort")).ok()?;
    let mut lines = text.lines();
    let port = lines.next()?.trim();
    let path = lines.next()?.trim();
    if port.parse::<u16>().is_err() || !path.starts_with('/') {
        return None;
    }
    Some(format!("ws://127.0.0.1:{port}{path}"))
}

/// `/json/version` da porta fixa -> URL do WebSocket. `Err(Headless)` quando a porta tem um Chrome de
/// automação (o agent-browser ocupa a 9222): sem cookies de ninguém, e trazer 0 parecia "sem login".
fn url_from_port(port: u16) -> Result<Option<String>, ImportError> {
    let Ok(mut stream) = TcpStream::connect(("127.0.0.1", port)) else { return Ok(None) };
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok();
    // HTTP/1.1 com `Connection: close`: o servidor de DevTools do Chrome ignora pedidos HTTP/1.0 (responde nada).
    if stream.write_all(b"GET /json/version HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n").is_err() {
        return Ok(None);
    }
    // Teto na resposta de um processo local qualquer que esteja na porta.
    let mut buf = Vec::new();
    stream.take(64 * 1024).read_to_end(&mut buf).ok();
    let text = String::from_utf8_lossy(&buf);
    let Some(body) = text.split("\r\n\r\n").nth(1) else { return Ok(None) };
    let Ok(v) = serde_json::from_str::<Value>(body.trim()) else {
        eprintln!("[cookies] /json/version da porta {port} nao deu JSON (truncado ou nao e um Chrome)");
        return Ok(None);
    };
    if v["User-Agent"].as_str().unwrap_or("").to_lowercase().contains("headless") {
        return Err(ImportError::Headless);
    }
    // O Chrome monta o host:porta do `webSocketDebuggerUrl` a partir do header Host; reconstrói com a porta conhecida
    // (só o caminho interessa) para não depender desse eco.
    Ok(v["webSocketDebuggerUrl"].as_str().and_then(|ws| url::Url::parse(ws).ok())
        .map(|u| format!("ws://127.0.0.1:{port}{}", u.path())))
}

/// Cookie do `Storage.getCookies` -> `CookieParam` do `Storage.setCookies`: só os campos que viajam.
fn to_param(c: &Value) -> Value {
    let mut o = json!({
        "name": c["name"],
        "value": c["value"],
        "domain": c["domain"],
        "path": c.get("path").filter(|v| v.is_string()).cloned().unwrap_or_else(|| json!("/")),
        "secure": c["secure"].as_bool().unwrap_or(false),
        "httpOnly": c["httpOnly"].as_bool().unwrap_or(false),
    });
    if let Some(s) = c["sameSite"].as_str() {
        o["sameSite"] = json!(s);
    }
    // -1 (ou ausente) = cookie de sessão: sem `expires`.
    if let Some(e) = c["expires"].as_f64().filter(|e| *e > 0.0) {
        o["expires"] = json!(e);
    }
    o
}

/// Todos os cookies do Chrome real como `CookieParam`. Tenta os perfis (jeito que o Chrome atual
/// aceita) e depois a porta fixa; cada candidato duas vezes, porque a 1ª conexão depois de um tempo
/// parado costuma cair e as seguintes respondem.
pub fn fetch_cookies(port: Option<u16>) -> Result<Vec<Value>, ImportError> {
    let mut candidates: Vec<String> = profile_roots().iter().filter_map(endpoint_from_profile).collect();
    // Headless na porta fixa não pode mascarar um perfil que funcionaria: fica como erro de reserva, não aborta.
    let mut last = ImportError::ChromeClosed;
    if let Some(p) = port {
        match url_from_port(p) {
            Ok(Some(ws)) => candidates.push(ws),
            Ok(None) => {}
            Err(e) => last = keep_specific(last, e),
        }
    }
    for ws in candidates {
        for attempt in 0..2 {
            match cdp_once(&ws, "Storage.getCookies") {
                Ok(result) => {
                    let cookies = result["cookies"].as_array().into_iter().flatten().map(to_param).collect();
                    return Ok(cookies);
                }
                Err(e) => {
                    // O erro mais específico vence: um candidato velho (porta morta) não apaga a recusa do Chrome do usuário.
                    last = keep_specific(last, e);
                    if attempt == 0 {
                        std::thread::sleep(Duration::from_millis(400));
                    }
                }
            }
        }
    }
    Err(last)
}

/// Mantém o erro mais informativo: `Failed`/`Headless` vencem o genérico `ChromeClosed`.
fn keep_specific(current: ImportError, new: ImportError) -> ImportError {
    fn rank(e: &ImportError) -> u8 {
        match e {
            ImportError::ChromeClosed => 0,
            ImportError::Headless => 1,
            ImportError::Failed(_) => 2,
        }
    }
    if rank(&new) >= rank(&current) { new } else { current }
}

/// Uma troca CDP por WebSocket contra o Chrome do usuário: manda `method` (sem params) e devolve o
/// `result`. Cliente mínimo em cima de `TcpStream` (bloqueante, roda em thread de fundo), não a pilha
/// WS do terminal: o teto de quadro é outro, e a conexão é de uma chamada só.
fn cdp_once(ws: &str, method: &str) -> Result<Value, ImportError> {
    // Endereço inválido ou fora de loopback é um `webSocketDebuggerUrl` torto da porta fixa, não "Chrome desligado":
    // devolve `Failed` para não disparar o fluxo de "ligue a depuração".
    let url = url::Url::parse(ws).map_err(|_| ImportError::Failed("cdp: endereco invalido".into()))?;
    // A porta fixa devolve o `webSocketDebuggerUrl` por conta própria: um processo local na porta poderia apontar para
    // outro host e injetar cookies forjados. Só loopback (IPv4, IPv6 ou localhost).
    let loopback = match url.host() {
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        Some(url::Host::Domain(d)) => d == "localhost",
        None => false,
    };
    if !loopback {
        return Err(ImportError::Failed("cdp: endereco nao e loopback".into()));
    }
    let host = url.host_str().unwrap_or("127.0.0.1").to_owned();
    let port = url.port().unwrap_or(80);
    let path = match url.query() {
        Some(q) => format!("{}?{q}", url.path()),
        None => url.path().to_owned(),
    };
    // Só a recusa de conexão é "Chrome desligado": uma vez conectado (a depuração estava ligada), qualquer falha é
    // um erro real (porta velha, demora ou recusa no diálogo), não motivo para mandar ligar a depuração de novo.
    let mut stream = TcpStream::connect((host.as_str(), port)).map_err(|_| ImportError::ChromeClosed)?;
    stream.set_read_timeout(Some(IO_TIMEOUT)).ok();
    stream.set_write_timeout(Some(IO_TIMEOUT)).ok();

    let mut nonce = [0u8; 16];
    SystemRandom::new().fill(&mut nonce).map_err(|_| ImportError::Failed("sem aleatoriedade".into()))?;
    let key = STANDARD.encode(nonce);
    let req = format!(
        "GET {path} HTTP/1.1\r\nHost: {host}:{port}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
         Sec-WebSocket-Version: 13\r\nSec-WebSocket-Key: {key}\r\n\r\n"
    );
    stream.write_all(req.as_bytes()).map_err(|_| ImportError::Failed("cdp: conexao caiu no handshake".into()))?;

    // Só os cabeçalhos: o servidor não manda quadro nenhum antes de a gente mandar o comando.
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        // Timeout aqui é a pessoa demorando a aprovar "Permitir depuração remota?" (ou a conexão caindo), não Chrome off.
        stream.read_exact(&mut byte).map_err(|_| ImportError::Failed("cdp: sem resposta ao handshake".into()))?;
        head.push(byte[0]);
        if head.len() > 8192 {
            return Err(ImportError::Failed("cdp: handshake sem fim".into()));
        }
    }
    if !head.starts_with(b"HTTP/1.1 101") {
        return Err(ImportError::Failed("cdp: depuracao recusada ou indisponivel".into()));
    }

    let payload = json!({"id": 1, "method": method}).to_string();
    let mut mask = [0u8; 4];
    SystemRandom::new().fill(&mut mask).ok();
    stream.write_all(&masked_text(payload.as_bytes(), mask)).map_err(|_| ImportError::Failed("cdp: envio falhou".into()))?;

    // Mensagem grande chega em vários quadros (junta até o FIN). Evento vindo antes da resposta: ignora.
    let mut text = Vec::new();
    loop {
        let (fin, op, data) = read_frame(&mut stream)?;
        match op {
            0x8 => return Err(ImportError::Failed("cdp: conexao fechada".into())),
            0x9 | 0xa => continue,
            0x1 | 0x0 => text.extend(data),
            _ => continue,
        }
        // `MAX_FRAME` é por quadro; este é o teto cumulativo entre quadros fragmentados.
        if text.len() > MAX_FRAME {
            return Err(ImportError::Failed("cdp: resposta grande demais".into()));
        }
        if fin {
            let v: Value = serde_json::from_slice(&text).map_err(|e| ImportError::Failed(e.to_string()))?;
            if v["id"] == json!(1) {
                if let Some(err) = v.get("error") {
                    return Err(ImportError::Failed(err["message"].as_str().unwrap_or("cdp").to_owned()));
                }
                return v.get("result").cloned().ok_or_else(|| ImportError::Failed("cdp: sem resultado".into()));
            }
            text.clear();
        }
    }
}

/// Um quadro de texto mascarado (cliente -> servidor): FIN + opcode texto.
fn masked_text(payload: &[u8], mask: [u8; 4]) -> Vec<u8> {
    let mut f = vec![0x81u8];
    let n = payload.len();
    if n < 126 {
        f.push(0x80 | n as u8);
    } else if n < 65536 {
        f.push(0x80 | 126);
        f.extend_from_slice(&(n as u16).to_be_bytes());
    } else {
        f.push(0x80 | 127);
        f.extend_from_slice(&(n as u64).to_be_bytes());
    }
    f.extend_from_slice(&mask);
    f.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
    f
}

/// Um quadro vindo do servidor (nunca mascarado) -> (fin, opcode, dados).
fn read_frame(stream: &mut impl Read) -> Result<(bool, u8, Vec<u8>), ImportError> {
    let mut h = [0u8; 2];
    stream.read_exact(&mut h).map_err(|_| ImportError::Failed("cdp: sem resposta".into()))?;
    let fin = h[0] & 0x80 != 0;
    let op = h[0] & 0x0f;
    let mut len = (h[1] & 0x7f) as usize;
    if len == 126 {
        let mut e = [0u8; 2];
        stream.read_exact(&mut e).map_err(|_| ImportError::Failed("cdp: sem resposta".into()))?;
        len = u16::from_be_bytes(e) as usize;
    } else if len == 127 {
        let mut e = [0u8; 8];
        stream.read_exact(&mut e).map_err(|_| ImportError::Failed("cdp: sem resposta".into()))?;
        len = u64::from_be_bytes(e) as usize;
    }
    if len > MAX_FRAME {
        return Err(ImportError::Failed("cdp: resposta grande demais".into()));
    }
    let mut data = vec![0u8; len];
    stream.read_exact(&mut data).map_err(|_| ImportError::Failed("cdp: sem resposta".into()))?;
    Ok((fin, op, data))
}

/// Abre `chrome://inspect/#remote-debugging` no Chrome do usuário. Não se lança o Chrome direto deste
/// processo sem fechar os descritores: o filho herdaria o pipe do CDP do Chromium embutido (fd 3 e 4)
/// e o seguraria depois de o app fechar — por isso o `sh` que fecha todo descritor acima de 2.
pub fn open_remote_debug_page() -> bool {
    use std::process::{Command, Stdio};
    #[cfg(target_os = "windows")]
    {
        Command::new("cmd").args(["/c", "start", "", "chrome", ACTIVATE_PAGE])
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().is_ok()
    }
    #[cfg(not(target_os = "windows"))]
    {
        use std::os::unix::process::CommandExt;
        let fd_close = "for fd in $(seq 3 1023); do eval \"exec $fd>&-\"; done 2>/dev/null; exec \"$0\" \"$@\"";
        for bin in ["google-chrome-stable", "google-chrome", "chromium", "chromium-browser", "brave"] {
            if !on_path(bin) {
                continue;
            }
            let spawned = Command::new("sh").args(["-c", fd_close, bin, ACTIVATE_PAGE])
                .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
                .process_group(0).spawn();
            if let Ok(mut child) = spawned {
                std::thread::spawn(move || { let _ = child.wait(); });
                return true;
            }
        }
        false
    }
}

#[cfg(not(target_os = "windows"))]
fn on_path(bin: &str) -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|dir| dir.join(bin).is_file()))
        .unwrap_or(false)
}

// Senhas salvas do Chrome do usuário -> preenchimento do login (Linux, como no Electron; o WebView2 do Windows não lê o
// perfil do Chrome). A senha em claro só existe na memória deste processo e no campo da página; nunca vai a disco.
#[cfg(target_os = "linux")]
mod passwords {
    use std::num::NonZeroU32;
    use std::path::PathBuf;
    use std::process::Command;

    use cbc::cipher::{BlockDecryptMut, KeyIvInit, block_padding::Pkcs7};
    use ring::rand::{SecureRandom, SystemRandom};

    type Aes128CbcDec = cbc::Decryptor<aes::Aes128>;

    /// (host sem www) -> [(usuário, senha)], mais usada primeiro. Uma cópia por banco porque o Chrome trava o arquivo.
    pub fn credentials_for(host: &str) -> Vec<(String, String)> {
        let target = host.trim_start_matches("www.").to_lowercase();
        if target.is_empty() {
            return vec![];
        }
        let keys = key_candidates();
        // Diretório só do usuário (`XDG_RUNTIME_DIR`, 0700) em vez de `/tmp`: fecha o symlink pré-plantado em `/tmp`
        // apontando a cópia do blob cifrado para outro arquivo.
        let tmp_base = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(|| {
            eprintln!("[senha] XDG_RUNTIME_DIR ausente; cópia temporária vai para /tmp");
            std::env::temp_dir()
        });
        let mut out = Vec::new();
        for db in login_data_dbs() {
            let tmp = tmp_base.join(format!("hangar-ld-{}-{}.db", std::process::id(), rand_suffix()));
            if let Err(e) = std::fs::copy(&db, &tmp) {
                eprintln!("[senha] copia de {} falhou: {e}", db.display());
                continue;
            }
            // Mais usada/mais recente primeiro: é a que o Chrome sugere, e a 1ª é a que preenche.
            let dump = Command::new("sqlite3").args([
                "-newline", "\x1e", "-separator", "\x1f", tmp.to_str().unwrap_or_default(),
                "select origin_url, username_value, hex(password_value) from logins \
                 where blacklisted_by_user=0 and length(password_value)>0 order by date_last_used desc, times_used desc",
            ]).output();
            std::fs::remove_file(&tmp).ok();
            // `sqlite3` ausente ou banco travado não é "sem senha salva": deixa rastro em vez de sumir calado.
            let dump = match dump {
                Ok(dump) => dump,
                Err(e) => { eprintln!("[senha] sqlite3 nao rodou (instalado?): {e}"); continue; }
            };
            if !dump.status.success() {
                eprintln!("[senha] sqlite3 saiu com {}", dump.status);
                continue;
            }
            let text = String::from_utf8_lossy(&dump.stdout);
            for row in text.split('\x1e') {
                let mut cols = row.split('\x1f');
                let (Some(url), Some(user), Some(hex)) = (cols.next(), cols.next(), cols.next()) else { continue };
                if url.is_empty() {
                    continue;
                }
                let Ok(parsed) = url::Url::parse(url) else { continue };
                let Some(h) = parsed.host_str() else { continue };
                let h = h.trim_start_matches("www.").to_lowercase();
                // Mesma host ou subdomínio nos dois sentidos (auth.exemplo.com <-> exemplo.com).
                if h != target && !target.ends_with(&format!(".{h}")) && !h.ends_with(&format!(".{target}")) {
                    continue;
                }
                let Some(buf) = decode_hex(hex.trim()) else { continue };
                for key in &keys {
                    if let Some(pass) = decrypt(&buf, key) {
                        out.push((user.to_owned(), pass));
                        break;
                    }
                }
            }
        }
        out
    }

    /// JS que preenche o primeiro campo de senha VISÍVEL e o texto/email antes dele. Valor como literal JSON (nunca
    /// concatenado na string do script): senha com aspas, barra ou template não vira código. Devolve `true` se achou.
    pub fn inject_login_js(user: &str, pass: &str) -> String {
        let arg = serde_json::json!({"usuario": user, "senha": pass}).to_string();
        let mut js = String::from("(() => { const {usuario, senha} = ");
        js.push_str(&arg);
        js.push_str(
            r#";
    const vis = (el) => el && el.offsetParent !== null && !el.disabled && !el.readOnly;
    const inputs = [];
    const walk = (root) => { for (const el of root.querySelectorAll('*')) { if (el.tagName === 'INPUT') inputs.push(el); if (el.shadowRoot) walk(el.shadowRoot); } };
    walk(document);
    const pw = inputs.find((el) => el.type === 'password' && vis(el));
    if (!pw) return false;
    const set = (el, v) => { const proto = el instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype; Object.getOwnPropertyDescriptor(proto, 'value').set.call(el, v); el.dispatchEvent(new Event('input', { bubbles: true })); el.dispatchEvent(new Event('change', { bubbles: true })); };
    set(pw, senha);
    if (usuario) { const before = inputs.slice(0, inputs.indexOf(pw)); const user = before.reverse().find((el) => vis(el) && /^(text|email|tel|)$/i.test(el.type)); if (user) set(user, usuario); }
    return true;
})()"#,
        );
        js
    }

    /// Perfis a ler: o padrão e o "For Account", que às vezes tem as senhas de trabalho.
    fn login_data_dbs() -> Vec<PathBuf> {
        let base = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from)
            .unwrap_or_else(|| super::dirs_home().join(".config"))
            .join("google-chrome").join("Default");
        ["Login Data", "Login Data For Account"].iter().map(|n| base.join(n)).filter(|p| p.exists()).collect()
    }

    /// Chaves candidatas: o literal "peanuts" (armazenamento básico) e a do chaveiro ("Chrome Safe Storage"); perfis
    /// diferem, então tenta as duas e fica com a que decifra para texto limpo.
    fn key_candidates() -> Vec<Vec<u8>> {
        let mut keys: Vec<Vec<u8>> = vec![b"peanuts".to_vec()];
        for app in ["chrome", "chromium"] {
            if let Ok(out) = Command::new("secret-tool").args(["lookup", "application", app]).output()
                && out.status.success() && !out.stdout.is_empty()
            {
                keys.push(out.stdout);
            }
        }
        keys.sort();
        keys.dedup();
        keys
    }

    /// AES-128-CBC v10/v11 do Linux: IV de 16 espaços, chave = PBKDF2(senha, "saltysalt", 1, SHA-1, 16 bytes). Sem o
    /// prefixo de 32 bytes do mac/Windows. `None` quando a chave não decifra para texto limpo.
    fn decrypt(cipher: &[u8], password: &[u8]) -> Option<String> {
        if cipher.len() < 3 || (&cipher[..3] != b"v10" && &cipher[..3] != b"v11") {
            return None;
        }
        let mut key = [0u8; 16];
        ring::pbkdf2::derive(ring::pbkdf2::PBKDF2_HMAC_SHA1, NonZeroU32::new(1).unwrap(), b"saltysalt", password, &mut key);
        let mut buf = cipher[3..].to_vec();
        let plain = Aes128CbcDec::new((&key).into(), (&[0x20u8; 16]).into())
            .decrypt_padded_mut::<Pkcs7>(&mut buf).ok()?;
        let text = std::str::from_utf8(plain).ok()?;
        // Lixo de controle (exceto espaços) significa chave errada.
        text.chars().all(|c| !c.is_control() || c.is_whitespace()).then(|| text.to_owned())
    }

    fn decode_hex(s: &str) -> Option<Vec<u8>> {
        // Separador de coluna dentro de um username desloca os campos e a "hex" vira texto arbitrário: sem o guarda,
        // o corte `[i..i+2]` cairia no meio de um char multibyte e derrubaria a thread.
        if !s.is_ascii() || !s.len().is_multiple_of(2) {
            return None;
        }
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok()).collect()
    }

    fn rand_suffix() -> String {
        let mut b = [0u8; 4];
        SystemRandom::new().fill(&mut b).ok();
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn hex_needs_even_length_and_valid_nibbles() {
            assert_eq!(decode_hex("0aff"), Some(vec![0x0a, 0xff]));
            assert!(decode_hex("0a1").is_none());
            assert!(decode_hex("zz").is_none());
            // Char multibyte (coluna deslocada por separador no username): não pode entrar em panic no corte.
            assert!(decode_hex("€a").is_none());
        }

        #[test]
        fn decrypt_round_trips_with_peanuts_and_rejects_wrong_key() {
            use cbc::cipher::{BlockEncryptMut, block_padding::Pkcs7};
            type Enc = cbc::Encryptor<aes::Aes128>;
            let mut key = [0u8; 16];
            ring::pbkdf2::derive(ring::pbkdf2::PBKDF2_HMAC_SHA1, NonZeroU32::new(1).unwrap(), b"saltysalt", b"peanuts", &mut key);
            let secret = b"s3nh4-secreta";
            let mut buf = vec![0u8; secret.len() + 16];
            buf[..secret.len()].copy_from_slice(secret);
            let ct = Enc::new((&key).into(), (&[0x20u8; 16]).into()).encrypt_padded_mut::<Pkcs7>(&mut buf, secret.len()).unwrap().to_vec();
            let mut stored = b"v10".to_vec();
            stored.extend_from_slice(&ct);
            assert_eq!(decrypt(&stored, b"peanuts").as_deref(), Some("s3nh4-secreta"));
            assert!(decrypt(&stored, b"wrong-key").is_none());
            assert!(decrypt(b"plaintext-no-prefix", b"peanuts").is_none());
        }

        #[test]
        fn inject_js_embeds_values_as_json_literal() {
            let js = inject_login_js("me@x.com", "a\"b\\c");
            assert!(js.contains(r#""usuario":"me@x.com""#));
            assert!(js.contains(r#""senha":"a\"b\\c""#));
            assert!(js.trim_end().ends_with("})()"));
        }
    }
}

#[cfg(target_os = "linux")]
pub use passwords::{credentials_for, inject_login_js};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_cookie_drops_expires_and_keeps_same_site() {
        let raw = json!({
            "name": "SID", "value": "abc", "domain": ".example.com", "path": "/app",
            "secure": true, "httpOnly": true, "sameSite": "Lax", "expires": -1.0, "size": 999
        });
        let p = to_param(&raw);
        assert_eq!(p["name"], "SID");
        assert_eq!(p["domain"], ".example.com");
        assert_eq!(p["path"], "/app");
        assert_eq!(p["secure"], true);
        assert_eq!(p["sameSite"], "Lax");
        assert!(p.get("expires").is_none(), "cookie de sessao nao leva expires");
        assert!(p.get("size").is_none(), "campos extras do getCookies nao viajam");
    }

    #[test]
    fn persistent_cookie_keeps_expires_and_defaults_missing_flags() {
        let p = to_param(&json!({"name": "a", "value": "b", "domain": "x.com", "expires": 1900000000.0}));
        assert_eq!(p["expires"], 1900000000.0);
        assert_eq!(p["path"], "/");
        assert_eq!(p["secure"], false);
        assert_eq!(p["httpOnly"], false);
        assert!(p.get("sameSite").is_none());
    }

    #[test]
    fn masked_frame_header_and_unmask_round_trips() {
        let frame = masked_text(b"Hello", [0x37, 0xfa, 0x21, 0x3d]);
        assert_eq!(&frame[..6], &[0x81, 0x85, 0x37, 0xfa, 0x21, 0x3d]);
        let back: Vec<u8> = frame[6..].iter().enumerate().map(|(i, b)| b ^ [0x37, 0xfa, 0x21, 0x3d][i % 4]).collect();
        assert_eq!(back, b"Hello");
        // Fronteira do comprimento estendido de 16 bits.
        assert_eq!(masked_text(&vec![0u8; 126], [0; 4])[1], 0x80 | 126);
        assert_eq!(masked_text(&vec![0u8; 125], [0; 4])[1], 0x80 | 125);
    }

    // Integração real: contra um Chrome headful com depuração na porta HANGAR_TEST_CDP_PORT. Rode com
    // XDG_CONFIG_HOME vazio para `profile_roots` não achar o Chrome do usuário e cair na porta fixa.
    #[test]
    #[ignore = "precisa de um Chrome com depuração em HANGAR_TEST_CDP_PORT"]
    fn fetch_cookies_against_a_live_chrome() {
        let port: u16 = std::env::var("HANGAR_TEST_CDP_PORT").expect("HANGAR_TEST_CDP_PORT").parse().unwrap();
        let cookies = match fetch_cookies(Some(port)) {
            Ok(c) => c,
            Err(e) => panic!("fetch_cookies falhou: {} {}", e.status_key(), e.detail()),
        };
        eprintln!("importou {} cookies do Chrome real", cookies.len());
        assert!(
            cookies.iter().any(|c| c["name"] == "hangar_test" && c["value"] == "abc123"),
            "esperava o cookie de teste hangar_test=abc123; veio: {cookies:?}"
        );
    }

    #[test]
    fn devtools_active_port_needs_port_then_ws_path() {
        let dir = std::env::temp_dir().join(format!("hangar-cdp-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("DevToolsActivePort"), "9222\n/devtools/browser/abc\n").unwrap();
        assert_eq!(endpoint_from_profile(&dir).as_deref(), Some("ws://127.0.0.1:9222/devtools/browser/abc"));
        std::fs::write(dir.join("DevToolsActivePort"), "notaport\n/x\n").unwrap();
        assert!(endpoint_from_profile(&dir).is_none());
        std::fs::remove_dir_all(&dir).ok();
    }
}
