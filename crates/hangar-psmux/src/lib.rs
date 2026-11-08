//! Leitura direta dos servidores do psmux: o mesmo pedido que o `tmux.exe list-panes -a` faz, sem
//! abrir um processo por consulta. O formato da conversa não é API pública do psmux; só as versões
//! conferidas contra o código delas usam este caminho.
use std::hash::{Hash, Hasher};
use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Saída de `tmux -V` das versões cujo `list-panes -a` e cuja autenticação foram lidos no código.
/// É a versão do programa chamado; a dos servidores vivos não é verificada aqui.
pub const VERSIONS: &[&str] = &[
    "tmux 3.3.8 psmux 3.3.8 (66cf613 2026-08-18)",
    "tmux 3.4.0 psmux 3.4.0 (20aa1d7 2026-10-13)",
];

/// O `-V` do psmux sai em duas linhas (`tmux X` e `psmux X (commit data)`); conta só a sequência de palavras.
pub fn known_version(output: &str) -> bool {
    let words = output.split_whitespace().collect::<Vec<_>>().join(" ");
    VERSIONS.contains(&words.as_str())
}

/// Resposta de um servidor de sessão; acima disso não é `list-panes`.
const MAX_REPLY: usize = 1 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Failure { pub code: &'static str }

fn fail(code: &'static str) -> Failure { Failure { code } }

/// Um servidor lido: nome no registro, porta e um resumo da chave (a chave em si não sai daqui).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Server { pub base: String, pub port: u16, key: u64 }

/// O texto que o CLI imprimiria e os servidores que o produziram.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listing { pub text: String, pub servers: Vec<Server> }

/// Pasta de dados que o CLI usaria com este ambiente: `PSMUX_DATA_DIR`, senão `USERPROFILE\.psmux`.
pub fn data_dir(var: impl Fn(&str) -> Option<std::ffi::OsString>) -> Option<PathBuf> {
    if let Some(raw) = var("PSMUX_DATA_DIR") {
        let text = raw.to_string_lossy();
        let path = PathBuf::from(text.trim_end_matches(['/', '\\']));
        return path.is_absolute().then_some(path);
    }
    var("USERPROFILE").filter(|home| !home.is_empty()).map(|home| PathBuf::from(home).join(".psmux"))
}

/// O `quote_arg` do psmux: o servidor relê o argumento entre aspas.
fn quote(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n").replace('\r', "\\r"))
}

fn io_code(error: &std::io::Error) -> &'static str {
    match error.kind() {
        ErrorKind::TimedOut | ErrorKind::WouldBlock => "psmux_direct_timeout",
        _ => "psmux_direct_io",
    }
}

fn left(deadline: Instant) -> Result<Duration, Failure> {
    let wait = deadline.saturating_duration_since(Instant::now());
    if wait.is_zero() { Err(fail("psmux_direct_timeout")) } else { Ok(wait) }
}

fn session(dir: &Path, base: &str, query: &str, deadline: Instant) -> Result<(String, Server), Failure> {
    let port = std::fs::read_to_string(dir.join(format!("{base}.port"))).map_err(|_| fail("psmux_direct_registry"))?;
    let port: u16 = port.trim().parse().map_err(|_| fail("psmux_direct_registry"))?;
    let key = std::fs::read_to_string(dir.join(format!("{base}.key"))).map_err(|_| fail("psmux_direct_registry"))?;
    let key = key.trim();
    let mut digest = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut digest);
    let server = Server { base: base.to_owned(), port, key: digest.finish() };
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream = TcpStream::connect_timeout(&addr, left(deadline)?).map_err(|e| match io_code(&e) {
        "psmux_direct_timeout" => fail("psmux_direct_timeout"),
        _ => fail("psmux_direct_connect"),
    })?;
    stream.set_write_timeout(Some(left(deadline)?)).map_err(|e| fail(io_code(&e)))?;
    stream.write_all(format!("AUTH {key}\n{query}").as_bytes()).map_err(|e| fail(io_code(&e)))?;
    // Cada leitura espera só o que falta do prazo: resposta que chega aos poucos não o estica.
    let mut reply = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        stream.set_read_timeout(Some(left(deadline)?)).map_err(|e| fail(io_code(&e)))?;
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => reply.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) => return Err(fail(io_code(&e))),
        }
        if reply.len() > MAX_REPLY { return Err(fail("psmux_direct_oversized")); }
        left(deadline)?;
    }
    let reply = String::from_utf8(reply).map_err(|_| fail("psmux_direct_encoding"))?;
    let mut lines = reply.split_inclusive('\n');
    if lines.next().map(str::trim) != Some("OK") { return Err(fail("psmux_direct_auth")); }
    // Como o CLI: as linhas saem como vieram, menos as em branco.
    let text: String = lines.filter(|line| !line.trim().is_empty()).collect();
    // Sessão viva tem ao menos um pane: nada depois do OK é resposta cortada.
    if text.is_empty() { return Err(fail("psmux_direct_empty")); }
    Ok((text, server))
}

/// `list-panes -a -F <format>` no espaço padrão (sem `-L`), na ordem da pasta, como o CLI. Um
/// servidor que falha derruba a lista inteira: o CLI o pularia calado. Tudo termina até `deadline`.
pub fn list_panes(dir: &Path, format: &str, deadline: Instant) -> Result<Listing, Failure> {
    let query = format!("list-panes -s -F {}\n", quote(format));
    let entries = std::fs::read_dir(dir).map_err(|_| fail("psmux_direct_registry"))?;
    let mut listing = Listing { text: String::new(), servers: Vec::new() };
    for entry in entries {
        let name = entry.map_err(|_| fail("psmux_direct_registry"))?.file_name();
        let Some(base) = name.to_str().and_then(|n| n.strip_suffix(".port")) else { continue };
        // A sessão pré-aberta e as de outro `-L` levam `__` no nome.
        if base.contains("__") { continue; }
        let (text, server) = session(dir, base, &query, deadline)?;
        listing.text.push_str(&text);
        listing.servers.push(server);
    }
    Ok(listing)
}

#[cfg(test)]
mod tests;
