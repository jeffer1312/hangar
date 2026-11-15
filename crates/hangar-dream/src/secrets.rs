//! Tira credenciais do texto que vai ao modelo e do que volta dele. Só credencial: caminho, e-mail,
//! IP e hash de commit são fatos que a memória precisa (o limpador do relatório de problema os tira).
//! Limpar de novo não muda nada.

use std::borrow::Cow;
use std::sync::LazyLock;

use regex::Regex;

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).unwrap()
}

static URL_USERINFO: LazyLock<Regex> = LazyLock::new(|| re(r#"(?i)\b([a-z][a-z0-9+.-]*://)[^\s/?#"'@:]+:[^\s/?#"']*@"#));
static AUTH_HEADER: LazyLock<Regex> = LazyLock::new(|| re(r#"(?i)(authorization):\s*(basic|bearer)\s+[^\s"'\\,;*]+"#));
static BEARER: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)\bBearer[ \t]+[A-Za-z0-9._~+/=-]+"));
static JSON_SECRET: LazyLock<Regex> =
    LazyLock::new(|| re(r#"(?i)"(password|token|secret|api_key|apikey|access_token|refresh_token|client_secret|authorization)"\s*:\s*"[^"*][^"]*""#));
// Só nomes que são credencial de fato; `key=` sozinho pega `hotkey=ctrl` e afins.
static KEY_VALUE: LazyLock<Regex> = LazyLock::new(|| {
    re(r#"(?i)\b(access_token|refresh_token|id_token|client_secret|api_key|apikey|cp_token|password|passwd|secret|token)=[^&\s'"\\,;*][^&\s'"\\,;]*"#)
});
static PREFIXED: LazyLock<Regex> = LazyLock::new(|| re(r"\b(sk-|glpat-|github_pat_|gh[pousr]_|xox[abpr]-|tskey-|hf_)[A-Za-z0-9_-]{8,}"));
static AWS: LazyLock<Regex> = LazyLock::new(|| re(r"\bAKIA[0-9A-Z]{16}\b"));
static JWT: LazyLock<Regex> = LazyLock::new(|| re(r"\beyJ[\w-]{8,}\.[\w-]+\.[\w-]*"));

pub fn scrub(text: &str) -> Cow<'_, str> {
    let mut out = Cow::Borrowed(text);
    out = apply(out, &URL_USERINFO, "${1}***@");
    out = apply(out, &AUTH_HEADER, "$1: $2 ***");
    out = apply(out, &BEARER, "Bearer ***");
    out = apply(out, &JSON_SECRET, r#""$1":"***""#);
    out = apply(out, &KEY_VALUE, "$1=***");
    out = apply(out, &PREFIXED, "$1***");
    out = apply(out, &AWS, "AKIA***");
    apply(out, &JWT, "eyJ***")
}

fn apply<'a>(text: Cow<'a, str>, re: &Regex, rep: &str) -> Cow<'a, str> {
    match re.replace_all(&text, rep) {
        Cow::Owned(s) => Cow::Owned(s),
        Cow::Borrowed(_) => text,
    }
}

#[cfg(test)]
mod tests {
    use super::scrub;

    /// Montada em tempo de execução: a trava do repositório público recusa literal com cara de chave.
    fn fake(prefix: &str) -> String {
        format!("{prefix}{}", "abcdefghij".repeat(2))
    }

    #[test]
    fn credentials_go_facts_stay() {
        let text = format!(
            "chave {} e {}; Authorization: Bearer abc.def; password=hunter2 https://user:pw@host.com/x {{\"token\": \"t0k3n\"}}",
            fake("sk-ant-"),
            fake("ghp_")
        );
        let out = scrub(&text);
        for leaked in ["abcdefghij", "abc.def", "hunter2", "user:pw", "t0k3n"] {
            assert!(!out.contains(leaked), "{leaked} vazou: {out}");
        }
        let facts = "/home/ana/proj · 10.0.0.5 · a@b.com · 63f659545abcdef0123456789abcdef01234567 · hotkey=ctrl · FOO=1";
        assert_eq!(scrub(facts), facts);
    }

    #[test]
    fn scrubbing_twice_changes_nothing() {
        let once = scrub(&format!("{} password=x token=y Bearer zzz {{\"secret\": \"s\"}}", fake("sk-ant-"))).into_owned();
        assert_eq!(scrub(&once), once);
    }
}
