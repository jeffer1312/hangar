//! Dados do usuário (`.env`, `peers.json`, `projects.json`) em `~/.hangar/config/`, fora da pasta da
//! versão. Máquina ainda não migrada os tem na `backend/`: vale o lugar novo quando o arquivo está lá;
//! senão, o antigo. A mesma regra do `app/user_data.py`, mais uma: máquina migrada (`~/.hangar/current`)
//! usa sempre o lugar novo, porque a `backend/` do pacote é da versão e some na troca seguinte, e a do
//! checkout do modo desenvolvedor deixa de ser lida quando o modo desliga.
use std::io;
use std::path::{Path, PathBuf};

use crate::account_env::AccountEnvironment;

/// `None` só quando o arquivo não está no lugar novo, a máquina não migrou e não há `backend/`
/// conhecida. Disco que não deixa conferir o lugar novo é erro, nunca "não existe".
pub fn path(home: &Path, backend: Option<&Path>, name: &str) -> io::Result<Option<PathBuf>> {
    let new = home.join(".hangar").join("config").join(name);
    if new.try_exists()? || crate::paths::current(home).try_exists()? {
        return Ok(Some(new));
    }
    Ok(backend.map(|dir| dir.join(name)))
}

/// O `.env` do servidor, com o home do processo.
pub fn env_file(backend: Option<&Path>) -> io::Result<Option<PathBuf>> {
    path(&AccountEnvironment::capture().home, backend, ".env")
}

/// O `.env` no que o Hangar grava: `CHAVE=valor`, a última linha vale, aspas fecham o valor e `#`
/// depois de espaço é comentário. Sem `export` nem `${VAR}`; a chave volta em minúsculas. A mesma
/// leitura para a subida do servidor e para quem consulta uma chave depois.
pub fn parse_env_file(text: &str) -> Vec<(String, String)> {
    text.lines().filter_map(|line| {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') { return None; }
        let (key, value) = line.split_once('=')?;
        Some((key.trim().to_ascii_lowercase(), unquote(value.trim())))
    }).collect()
}

/// As linhas do `.env` que a leitura acima deixa de fora ou lê ao pé da letra, pelo número da linha
/// e nunca pelo valor (pode ser o token).
pub fn env_file_issues(text: &str) -> Vec<String> {
    let mut issues = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let (number, line) = (index + 1, line.trim());
        if line.is_empty() || line.starts_with('#') { continue; }
        let Some((key, value)) = line.split_once('=') else {
            issues.push(format!("linha {number} sem '=' ignorada"));
            continue;
        };
        let key = key.trim();
        if key.starts_with("export ") || key.starts_with("export\t") {
            issues.push(format!("linha {number} com `export` ignorada; escreva só CHAVE=valor"));
        } else if key.is_empty() || !key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
            issues.push(format!("linha {number} com chave fora do formato ignorada"));
        } else if value.contains("${") {
            issues.push(format!("linha {number} ({key}) usa ${{...}}, que não é expandido"));
        }
    }
    issues
}

fn unquote(value: &str) -> String {
    for quote in ['"', '\''] {
        if let Some(rest) = value.strip_prefix(quote) {
            return quoted(rest, quote);
        }
    }
    let cut = value.char_indices().find(|&(at, c)| c == '#' && value[..at].ends_with(char::is_whitespace))
        .map_or(value.len(), |(at, _)| at);
    value[..cut].trim_end().to_owned()
}

/// Até a aspa que fecha, sem escape antes dela. Os escapes são os do python-dotenv: entre aspas duplas
/// `\\ \' \" \a \b \f \n \r \t \v`; entre simples, só `\\` e `\'`. Escape desconhecido fica como está.
fn quoted(rest: &str, quote: char) -> String {
    let mut out = String::new();
    let mut chars = rest.chars();
    while let Some(c) = chars.next() {
        if c == quote { break; }
        if c != '\\' { out.push(c); continue; }
        let Some(next) = chars.next() else { out.push('\\'); break };
        let decoded = match (quote, next) {
            (_, '\\') => Some('\\'),
            (_, '\'') => Some('\''),
            ('"', '"') => Some('"'),
            ('"', 'a') => Some('\u{7}'),
            ('"', 'b') => Some('\u{8}'),
            ('"', 'f') => Some('\u{c}'),
            ('"', 'n') => Some('\n'),
            ('"', 'r') => Some('\r'),
            ('"', 't') => Some('\t'),
            ('"', 'v') => Some('\u{b}'),
            _ => None,
        };
        match decoded {
            Some(decoded) => out.push(decoded),
            None => { out.push('\\'); out.push(next); }
        }
    }
    out
}

/// O valor de `key` no `.env` dos dados do usuário, lido por [`parse_env_file`] (chave sem caixa, a
/// última linha vale). `None` sem o arquivo ou sem a chave; arquivo que não se lê é erro.
pub fn env_value(home: &Path, backend: Option<&Path>, key: &str) -> Result<Option<String>, String> {
    let env = path(home, backend, ".env").map_err(|e| format!("não consegui conferir o .env dos dados do usuário: {e}"))?;
    let Some(env) = env else { return Ok(None) };
    let text = match std::fs::read_to_string(&env) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", env.display())),
    };
    Ok(parse_env_file(&text).into_iter().rev().find(|(name, _)| name.eq_ignore_ascii_case(key)).map(|(_, value)| value))
}

/// O `.env` cru dos dados do usuário; ausente vale vazio e `None` é a falta da pasta de dados.
pub fn read_env_raw(home: &Path, backend: Option<&Path>) -> Result<Option<String>, String> {
    let Some(env) = path(home, backend, ".env").map_err(|e| e.to_string())? else { return Ok(None) };
    read_env_at(&env).map(Some)
}

/// O `.env` cru de um caminho já resolvido, lido como numa troca atômica em andamento.
pub fn read_env_at(env: &Path) -> Result<String, String> {
    match crate::fs::read_replaced_to_string(env) {
        Ok(raw) => Ok(raw),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e.to_string()),
    }
}

/// Uma porta (`CP_PORT`, `CP_FRONT_PORT`) do `.env`, como a subida a lê: valor que não é porta é
/// erro, vazio incluído (o servidor recusa subir com `CP_PORT=`). Só `CP_FRONT_PORT` vazia vale
/// ausente: é o jeito de desligá-la à mão.
pub fn env_port(home: &Path, backend: Option<&Path>, key: &str) -> Result<Option<u16>, String> {
    let Some(value) = env_value(home, backend, key)? else { return Ok(None) };
    if value.trim().is_empty() && key.eq_ignore_ascii_case("CP_FRONT_PORT") { return Ok(None); }
    value.trim().parse::<u16>().map(Some).map_err(|_| format!("{key}={value:?} no .env não é uma porta"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_new_place_wins_and_the_backend_is_the_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let (home, backend) = (dir.path().join("home"), dir.path().join("backend"));
        std::fs::create_dir_all(&backend).unwrap();
        std::fs::write(backend.join(".env"), "").unwrap();
        assert_eq!(path(&home, Some(&backend), ".env").unwrap(), Some(backend.join(".env")));
        assert_eq!(path(&home, None, ".env").unwrap(), None);
        assert!(!home.join(".hangar").exists(), "ler não cria a pasta nova");
        let new = home.join(".hangar/config/.env");
        std::fs::create_dir_all(new.parent().unwrap()).unwrap();
        std::fs::write(&new, "").unwrap();
        assert_eq!(path(&home, Some(&backend), ".env").unwrap(), Some(new.clone()));
        assert_eq!(path(&home, None, ".env").unwrap(), Some(new));
        // Arquivo a arquivo: o que ainda não migrou continua na `backend/`.
        assert_eq!(path(&home, Some(&backend), "peers.json").unwrap(), Some(backend.join("peers.json")));
    }

    /// `~/.hangar/current` numa versão com manifesto de pacote.
    #[cfg(unix)]
    fn package_home(home: &std::path::Path) {
        let version = home.join(".hangar/versions/0.1.0.5");
        std::fs::create_dir_all(&version).unwrap();
        std::fs::write(version.join("manifest.json"), r#"{"version": "0.1.0.5", "commit": "c", "channel": "latest"}"#).unwrap();
        std::os::unix::fs::symlink(&version, home.join(".hangar/current")).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_package_install_uses_the_new_place_even_without_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let (home, backend) = (dir.path().join("home"), dir.path().join("backend"));
        std::fs::create_dir_all(&backend).unwrap();
        std::fs::write(backend.join("peers.json"), "{}").unwrap();
        package_home(&home);
        assert_eq!(path(&home, Some(&backend), "peers.json").unwrap(), Some(home.join(".hangar/config/peers.json")));
        assert_eq!(path(&home, None, "projects.json").unwrap(), Some(home.join(".hangar/config/projects.json")));
    }

    /// Modo desenvolvedor: o arquivo que não existia na migração nasce no lugar novo, não no checkout,
    /// senão some quando o modo desenvolvedor desliga.
    #[cfg(unix)]
    #[test]
    fn a_developer_mode_machine_uses_the_new_place_even_without_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let (home, checkout) = (dir.path().join("home"), dir.path().join("checkout"));
        std::fs::create_dir_all(checkout.join(".git")).unwrap();
        std::fs::create_dir_all(checkout.join("backend")).unwrap();
        std::fs::create_dir_all(home.join(".hangar")).unwrap();
        std::os::unix::fs::symlink(&checkout, home.join(".hangar/current")).unwrap();
        assert_eq!(path(&home, Some(&checkout.join("backend")), "peers.json").unwrap(), Some(home.join(".hangar/config/peers.json")));
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_config_dir_is_an_error_not_the_old_place() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let (home, backend) = (dir.path().join("home"), dir.path().join("backend"));
        let config = home.join(".hangar/config");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o000)).unwrap();
        let (read, root) = (path(&home, Some(&backend), ".env"), std::fs::read_dir(&config).is_ok());
        std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o700)).unwrap();
        if root { return; } // root lê tudo: o caso não se monta
        assert!(read.is_err(), "{read:?}");
    }

    #[test]
    fn env_value_strips_quotes_comments_and_spaces_and_a_bad_port_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let (home, backend) = (dir.path().join("home"), dir.path().join("backend"));
        std::fs::create_dir_all(&backend).unwrap();
        assert_eq!(env_port(&home, Some(&backend), "CP_PORT").unwrap(), None, "sem .env");
        std::fs::write(backend.join(".env"), "CP_PORT=1\n  CP_PORT = 2\nCP_PORT=\"9100\"  # a porta\nCP_FRONT_PORT='abc'\nCP_EMPTY=\nCP_X=a #b\nCP_Y=a#b\nexport CP_Z=1\ncp_low=9\n").unwrap();
        assert_eq!(env_port(&home, Some(&backend), "CP_PORT").unwrap(), Some(9100), "a última vale, sem aspas nem comentário");
        assert_eq!(env_value(&home, Some(&backend), "CP_X").unwrap().as_deref(), Some("a"));
        assert_eq!(env_value(&home, Some(&backend), "CP_Y").unwrap().as_deref(), Some("a#b"), "# sem espaço antes é valor");
        assert_eq!(env_value(&home, Some(&backend), "CP_Z").unwrap(), None, "export não é lido (decisão 10)");
        assert_eq!(env_value(&home, Some(&backend), "CP_LOW").unwrap().as_deref(), Some("9"), "chave sem caixa");
        assert!(env_port(&home, Some(&backend), "CP_EMPTY").is_err(), "porta vazia é recusada, como na subida");
        assert_eq!(env_port(&home, Some(&backend), "CP_NADA").unwrap(), None);
        let error = env_port(&home, Some(&backend), "CP_FRONT_PORT").unwrap_err();
        assert!(error.contains("CP_FRONT_PORT") && error.contains("abc"), "{error}");
        std::fs::write(backend.join(".env"), "CP_FRONT_PORT=\n").unwrap();
        assert_eq!(env_port(&home, Some(&backend), "CP_FRONT_PORT").unwrap(), None, "CP_FRONT_PORT vazia desliga");
    }

    /// O aviso diz a linha e nunca o valor, que pode ser o token.
    #[test]
    fn env_file_issues_name_the_line_never_the_value() {
        let text = "# comentário\nCP_AUTH_TOKEN=segredo\nsolto segredo\nexport CP_PORT=9000\n\u{feff}CP_X=segredo\nCP_Y=${BASE}-segredo\n\n";
        let issues = env_file_issues(text);
        assert_eq!(issues.len(), 4, "{issues:?}");
        for (issue, line) in issues.iter().zip([3, 4, 5, 6]) {
            assert!(issue.starts_with(&format!("linha {line} ")), "{issue}");
            assert!(!issue.contains("segredo"), "{issue}");
        }
        assert!(env_file_issues("CP_PORT=9000\nCP_AUTH_TOKEN=\"a b\" # x\n").is_empty());
    }
}
