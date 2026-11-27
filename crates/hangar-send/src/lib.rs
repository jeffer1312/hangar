//! `hangar-server send`: o `hangar-send` inteiro (recados, grupo, contrato, `--new`, `--close`,
//! `--list`, identidade), sem bash, curl nem Python. O `scripts/hangar-send` é só o atalho que acha o
//! binário e passa a raiz da cópia dele (`--root`), de onde sai o `.env` da máquina não migrada.
//!
//! Contrato: frases, códigos e pedidos HTTP do script antigo (o cartão do app e o orq leem a saída),
//! com os defeitos que a migração corrigiu (flag desconhecida recusada, nome codificado na URL,
//! `--list` sem servidor dá `erro:` e 2, `a::b::c` recusado, resposta sem campo vira `erro:`).
use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;

/// Saída de um processo ou de uma operação do servidor que o comando repassa.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Captured {
    pub code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// O que vem do binário do servidor e do sistema, trocado nos testes.
pub trait Host {
    /// `hangar-server peers <args>`: `path`, `config <id>` ou `tsv`, com a saída e o código dele.
    fn peers(&self, args: &[&str]) -> Captured;
    /// `hangar-server conta --prep <nome>`: o caminho do config dir no stdout.
    fn account_prep(&self, name: &str) -> Captured;
    /// `tmux <args>` com a entrada vazia; `None` quando o programa não roda.
    fn tmux(&self, args: &[&str]) -> Option<Captured>;
}

/// Uma chamada do comando, com o ambiente lido uma vez na largada.
#[derive(Clone, Debug, Default)]
pub struct Invocation {
    /// Os argumentos depois de `send`, já sem `--root`.
    pub args: Vec<String>,
    /// A raiz da cópia do atalho (`<raiz>/backend/.env` na máquina não migrada).
    pub root: Option<PathBuf>,
    /// `HOME` (ou `USERPROFILE`) de quem chama.
    pub home: PathBuf,
    /// O ambiente de quem chama.
    pub vars: BTreeMap<String, String>,
    /// A pasta real do processo (vazia = a do processo atual), contra a qual vale caminho relativo.
    pub cwd: PathBuf,
}

/// Teto do corpo de uma resposta do servidor.
pub const RESPONSE_LIMIT: usize = http::BODY_LIMIT;

/// O texto do `--help`.
pub const HELP: &str = include_str!("help.txt");

/// Tira o `--root <dir>` que o atalho põe na frente dos argumentos.
pub fn split_root(args: &[String]) -> (Option<PathBuf>, Vec<String>) {
    match args {
        [flag, root, rest @ ..] if flag == "--root" => (Some(PathBuf::from(root)), rest.to_vec()),
        _ => (None, args.to_vec()),
    }
}

/// Roda o comando e devolve o código de saída: 0 feito, 1 recusa ou erro, 2 uso errado ou servidor
/// sem resposta, como o script antigo; no recado cortado, 3 não chegou e 4 incerto.
pub fn run(invocation: &Invocation, host: &dyn Host, out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let args = &invocation.args;
    // A ajuda sai antes do `.env`: é o que se lê numa máquina ainda sem Hangar.
    if matches!(args.first().map(String::as_str), Some("--help" | "-h")) {
        let _ = out.write_all(HELP.as_bytes());
        return 0;
    }
    let settings = match env::read(invocation, err) {
        Ok(settings) => settings,
        Err(code) => return code,
    };
    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(error) => {
            let _ = writeln!(err, "erro: não consegui preparar o cliente HTTP: {error}");
            return 1;
        }
    };
    let cx = Ctx { inv: invocation, host, runtime, settings };
    match args.first().map(String::as_str) {
        Some("--whoami") => {
            let name = identity::me(&cx, false, err);
            let _ = writeln!(out, "{name}");
            0
        }
        None | Some("--list") => list::run(&cx, out, err),
        Some("--pair") => group::pair(&cx, &args[1..], out, err),
        Some("--aceitar-par") => group::accept(&cx, &args[1..], out, err),
        Some("--new") => new::run(&cx, &args[1..], out, err),
        Some("--unpair") => group::unpair(&cx, out, err),
        Some("--close") => group::close(&cx, &args[1..], out, err),
        Some("--contrato") => group::contract(&cx, &args[1..], out, err),
        Some("--group") => group::group(&cx, &args[1..], out, err),
        _ => message::run(&cx, args, out, err),
    }
}

mod env;
mod group;
mod http;
mod identity;
mod list;
mod message;
mod new;

use http::Endpoint;
use serde_json::Value;

/// O estado de uma chamada: ambiente, `.env` lido e o runtime dos pedidos.
pub(crate) struct Ctx<'a> {
    pub inv: &'a Invocation,
    pub host: &'a dyn Host,
    pub runtime: tokio::runtime::Runtime,
    pub settings: env::Settings,
}

impl Ctx<'_> {
    /// Variável do ambiente de quem chama; vazia conta como ausente (`${X:-}` do script).
    pub fn var(&self, key: &str) -> Option<&str> {
        self.inv.vars.get(key).map(String::as_str).filter(|value| !value.is_empty())
    }

    pub fn connect(&self) -> f64 {
        http::seconds(self.inv.vars.get("API_CONNECT_TIMEOUT").map(String::as_str), 3.0)
    }

    pub fn total(&self) -> f64 {
        http::seconds(self.inv.vars.get("API_TIMEOUT").map(String::as_str), 30.0)
    }

    /// Quanto o recado cortado espera aparecer na conversa do destino.
    pub fn check_window(&self) -> f64 {
        http::seconds(self.inv.vars.get("HANGAR_SEND_CHECK_SECONDS").map(String::as_str), 6.0)
    }

    pub fn base(&self) -> String {
        format!("http://127.0.0.1:{}", self.settings.port)
    }

    /// O servidor desta máquina, com os prazos do ambiente.
    pub fn local(&self) -> Endpoint {
        Endpoint { base: self.base(), token: self.settings.token.clone(), connect: self.connect(), total: self.total(), peer: None }
    }

    pub fn fetch(&self, endpoint: &Endpoint, method: &str, path: &str, body: Option<&str>) -> http::Fetched {
        self.runtime.block_on(http::fetch(endpoint, method, path, body))
    }

    pub fn api(&self, endpoint: &Endpoint, method: &str, path: &str, body: Option<&str>, err: &mut dyn Write) -> Result<Vec<u8>, u8> {
        http::report(endpoint, self.fetch(endpoint, method, path, body), err)
    }

    /// `peers config <id>`: base e token da outra máquina; falha já explicada pelo livro.
    pub fn peer_config(&self, id: &str, err: &mut dyn Write) -> Result<(String, String), u8> {
        let found = self.host.peers(&["config", id]);
        let _ = err.write_all(&found.stderr);
        if found.code != 0 {
            return Err(1);
        }
        let text = String::from_utf8_lossy(&found.stdout);
        let line = text.lines().next().unwrap_or("").trim_matches('\t');
        let (base, token) = line.split_once('\t').unwrap_or((line, ""));
        Ok((base.to_owned(), token.trim_start_matches('\t').to_owned()))
    }
}

/// O corpo como o `json.loads` do Python o leria.
pub(crate) fn parse(body: &[u8]) -> Option<Value> {
    hangar_core::pycompat::pyjson::loads_lossless(&String::from_utf8_lossy(body))
}

/// `%s` do Python sobre um valor do JSON.
pub(crate) fn text(value: &Value) -> String {
    hangar_core::pycompat::py::py_str(value)
}

pub(crate) fn truthy(value: Option<&Value>) -> bool {
    value.is_some_and(hangar_core::pycompat::truthy)
}

pub(crate) fn unreadable(err: &mut dyn Write, route: &str) -> u8 {
    let _ = writeln!(err, "erro: resposta ilegível de {route}");
    1
}

pub(crate) fn missing(err: &mut dyn Write, route: &str, field: &str) -> u8 {
    let _ = writeln!(err, "erro: resposta de {route} sem o campo '{field}'");
    1
}

/// Os campos pedidos, na ordem do script; o primeiro que falta é o que a frase cita.
pub(crate) fn fields<'v>(object: &'v serde_json::Map<String, Value>, names: &[&'static str]) -> Result<Vec<&'v Value>, &'static str> {
    names.iter().map(|name| object.get(*name).ok_or(*name)).collect()
}

/// O nome da sessão como um segmento do caminho: só `A-Za-z0-9_.-~` fica cru, o resto vira `%XX`
/// (o `urllib.parse.quote(nome, safe="")`). Cru, `#`, `?` e `/` mandavam o pedido a outra rota.
pub fn path_segment(name: &str) -> String {
    const KEEP: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
        .remove(b'_').remove(b'.').remove(b'-').remove(b'~');
    percent_encoding::utf8_percent_encode(name, KEEP).to_string()
}

/// `/c/x` como o Git Bash convertia ao chamar o Python nativo: `C:/x`, com barras para a frente.
/// Só no Windows; nos outros sistemas o caminho segue como veio.
pub fn msys_drive_path(path: &str, windows: bool) -> String {
    let bytes = path.as_bytes();
    let drive = windows && bytes.len() >= 2 && bytes[0] == b'/' && bytes[1].is_ascii_alphabetic()
        && (bytes.len() == 2 || bytes[2] == b'/');
    if !drive {
        return path.to_owned();
    }
    let rest = if bytes.len() == 2 { "/" } else { &path[2..] };
    format!("{}:{rest}", char::from(bytes[1].to_ascii_uppercase()))
}

/// O arquivo como o `open(f, encoding="utf-8").read()` do Python: o BOM fica no texto e `\r\n` e `\r`
/// viram `\n`. `None` quando não é UTF-8.
pub fn read_text_universal(bytes: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(bytes).ok()?;
    Some(text.replace("\r\n", "\n").replace('\r', "\n"))
}
