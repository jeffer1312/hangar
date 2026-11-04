//! Tira do texto do relatório o que identifica a pessoa ou abre acesso: casa, tokens, chaves,
//! e-mail, nomes do Tailscale e IPs. Cada regra olha uma linha só, então limpar por partes e limpar
//! o texto montado dão o mesmo resultado, e limpar de novo não muda nada.
use regex::{Captures, Regex};
use std::borrow::Cow;
use std::sync::LazyLock;

/// O que muda por máquina: a casa e a `LOCALAPPDATA` literais, além das formas padrão que as
/// regras já cobrem. Cada uma guarda também a forma escapada do JSON, que vem antes: senão sobra
/// meio caminho.
pub struct Scrubber {
    home: Vec<String>,
    local_app_data: Vec<String>,
}

fn re(pattern: &str) -> Regex { Regex::new(pattern).unwrap() }

// `[\\/]+` cobre `/`, `\` e o `\\` do JSON; o nome da pessoa pode ter espaço.
static LOCAL_APP_DATA: LazyLock<Regex> = LazyLock::new(|| re(r#"(?i)[a-z]:[\\/]+users[\\/]+[^\\/\r\n"]+[\\/]+appdata[\\/]+local"#));
static WINDOWS_HOME: LazyLock<Regex> = LazyLock::new(|| re(r#"(?i)[a-z]:[\\/]+users[\\/]+[^\\/\r\n"]+"#));
static UNIX_HOME: LazyLock<Regex> = LazyLock::new(|| re(r#"/(?:home|Users)/[^/\s'"\\]+"#));
static TMP_CLAUDE: LazyLock<Regex> = LazyLock::new(|| re(r"/tmp/claude-\d+"));
// Credencial na URL (`esquema://usuário:senha@host`); sem `/`, aspas nem espaço, para não engolir o
// resto de uma linha JSON.
static URL_USERINFO: LazyLock<Regex> = LazyLock::new(|| re(r#"(?i)\b([a-z][a-z0-9+.-]*://)[^\s/?#"'@:]+:[^\s/?#"']*@"#));
static AUTH_HEADER: LazyLock<Regex> = LazyLock::new(|| re(r#"(?i)(authorization):\s*(basic|bearer)\s+[^\s"'\\,;]+"#));
static BEARER: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)\bBearer[ \t]+[A-Za-z0-9._~+/=-]+"));
static JSON_SECRET: LazyLock<Regex> =
    LazyLock::new(|| re(r#"(?i)"(password|token|secret|api_key|access_token|refresh_token|authorization)"\s*:\s*"[^"]*""#));
// Sem `\b` na frente: `access_token` e `my_token` têm `_` antes do `token`.
static KEY_VALUE: LazyLock<Regex> = LazyLock::new(|| {
    re(r#"(?i)(access_token|refresh_token|id_token|client_secret|api_key|apikey|cp_token|password|passwd|pwd|secret|token|key)=[^&\s'"\\,;]+"#)
});
// `code=` só em URL: no texto do relatório `code=` é o código de erro do log.
static URL_CODE: LazyLock<Regex> = LazyLock::new(|| re(r#"(?i)([?&]code)=[^&\s'"\\,;]+"#));
static PREFIXED: LazyLock<Regex> =
    LazyLock::new(|| re(r"\b(sk-|glpat-|github_pat_|gh[pousr]_|xox[abpr]-|tskey-|hf_)[A-Za-z0-9_-]+"));
static AWS: LazyLock<Regex> = LazyLock::new(|| re(r"\bAKIA[0-9A-Z]{16}\b"));
static JWT: LazyLock<Regex> = LazyLock::new(|| re(r"\beyJ[\w-]+\.[\w-]+\.[\w-]*"));
static EMAIL: LazyLock<Regex> =
    LazyLock::new(|| re(r"([A-Za-z0-9._%+-])[A-Za-z0-9._%+-]*@([A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)*\.[A-Za-z]{2,})"));
static TS_NET: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)\b[a-z0-9-]+(?:\.[a-z0-9-]+)*\.ts\.net\b"));
static IPV4: LazyLock<Regex> = LazyLock::new(|| re(r"\b(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})\b"));
static IPV6: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)\b[0-9a-f]{1,4}(?::[0-9a-f]{0,4}){2,7}"));
static HEX: LazyLock<Regex> = LazyLock::new(|| re(r"\b[0-9a-fA-F]{32,}\b"));
static BASE64: LazyLock<Regex> = LazyLock::new(|| re(r"[A-Za-z0-9+/_-]{32,}={0,2}"));
static ABSOLUTE_PATH: LazyLock<Regex> = LazyLock::new(|| {
    re(r#"(?:~|<tmp-claude>|%LOCALAPPDATA%|/(?:home|mnt|Users|tmp|var|opt|media)\b|\b[A-Za-z]:)(?:[\\/]+[^\\/\s"'<>]+)+"#)
});

impl Scrubber {
    pub fn new(home: Option<String>, local_app_data: Option<String>) -> Self {
        let forms = |path: Option<String>| -> Vec<String> {
            let Some(path) = path.filter(|p| p.len() > 3) else { return Vec::new() };
            let escaped = path.replace('\\', "\\\\");
            if escaped == path { vec![path] } else { vec![escaped, path] }
        };
        Self { home: forms(home), local_app_data: forms(local_app_data) }
    }

    /// A casa e a `LOCALAPPDATA` do ambiente do servidor (nos testes, a casa de teste).
    pub fn for_env(env: &crate::accounts::environment::AccountEnvironment) -> Self {
        Self::new(Some(env.home.to_string_lossy().into_owned()), env.base.get("LOCALAPPDATA").cloned())
    }

    /// Só aloca quando alguma regra casa.
    pub fn scrub<'a>(&self, text: &'a str) -> Cow<'a, str> {
        let mut out = Cow::Borrowed(text);
        for dir in &self.local_app_data { out = literal(out, dir, "%LOCALAPPDATA%"); }
        out = apply(out, &LOCAL_APP_DATA, "%LOCALAPPDATA%");
        for home in &self.home { out = literal(out, home, "~"); }
        out = apply(out, &WINDOWS_HOME, "~");
        out = apply(out, &UNIX_HOME, "~");
        out = apply(out, &TMP_CLAUDE, "<tmp-claude>");
        out = apply(out, &URL_USERINFO, "${1}***@");
        out = apply(out, &AUTH_HEADER, "$1: $2 ***");
        out = apply(out, &BEARER, "Bearer ***");
        out = apply(out, &JSON_SECRET, r#""$1":"***""#);
        out = apply(out, &KEY_VALUE, "$1=***");
        out = apply(out, &URL_CODE, "$1=***");
        out = apply(out, &PREFIXED, "$1***");
        out = apply(out, &AWS, "AKIA***");
        out = apply(out, &JWT, "eyJ***");
        out = apply(out, &EMAIL, "$1***@$2");
        out = apply(out, &TS_NET, "***.ts.net");
        out = apply(out, &IPV4, |c: &Captures| {
            let octets = (1..=4).all(|i| c[i].parse::<u16>().is_ok_and(|n| n <= 255));
            if octets { "*.*.*.*".to_owned() } else { c[0].to_owned() }
        });
        // Hora `10:00:00` também casa: só é IPv6 com `::`, letra hex ou cinco grupos ou mais.
        out = apply(out, &IPV6, |c: &Captures| {
            let run = &c[0];
            let ipv6 = run.contains("::") || run.bytes().any(|b| b.is_ascii_alphabetic()) || run.matches(':').count() >= 4;
            if ipv6 { "*:*".to_owned() } else { run.to_owned() }
        });
        out = apply(out, &HEX, "***");
        apply(out, &BASE64, |c: &Captures| {
            let run = &c[0];
            if base64_secret(run) { "***".to_owned() } else { run.to_owned() }
        })
    }
}

fn literal<'a>(text: Cow<'a, str>, from: &str, to: &str) -> Cow<'a, str> {
    if text.contains(from) { Cow::Owned(text.replace(from, to)) } else { text }
}

fn apply<'a>(text: Cow<'a, str>, re: &Regex, rep: impl regex::Replacer) -> Cow<'a, str> {
    let replaced = match re.replace_all(&text, rep) { Cow::Owned(s) => Some(s), Cow::Borrowed(_) => None };
    replaced.map_or(text, Cow::Owned)
}

/// Segredo: um trecho de 32+ sem `/` com maiúscula, minúscula e dígito (mesmo com `-`/`_`), ou
/// dígito e um trecho de 16+ sem separador. Nome de sessão em minúsculas com hífen não é.
fn base64_secret(run: &str) -> bool {
    let has = |s: &str, f: fn(&u8) -> bool| s.bytes().any(|b| f(&b));
    let mixed = run.split('/').any(|p| p.len() >= 32 && has(p, u8::is_ascii_uppercase) && has(p, u8::is_ascii_lowercase) && has(p, u8::is_ascii_digit));
    let dense = has(run, u8::is_ascii_digit) && run.split(['-', '_', '/']).map(str::len).max().unwrap_or(0) >= 16;
    mixed || dense
}

/// `a***@dominio` para um rótulo que é e-mail; o resto passa igual.
pub fn mask_email(text: &str) -> String {
    EMAIL.replace_all(text, "$1***@$2").into_owned()
}

/// Caminho absoluto vira `…/<último segmento>`: a mensagem do log não é fixa e pode trazer pastas.
pub fn shorten_paths(text: &str) -> String {
    ABSOLUTE_PATH.replace_all(text, |c: &Captures| {
        let last = c[0].rsplit(['/', '\\']).find(|s| !s.is_empty()).unwrap_or("");
        format!("…/{last}")
    }).into_owned()
}
