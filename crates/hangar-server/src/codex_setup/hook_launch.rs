//! Seleção do executável sem passar os argumentos do hook por outro interpretador.

pub fn expression(path: &str) -> Option<String> {
    let normalized = path.replace('\\', "/");
    let suffix = "/.hangar/current/bin/hangar-server.exe";
    if !normalized.to_ascii_lowercase().ends_with(suffix) { return None; }
    let home = &normalized[..normalized.len() - suffix.len()];
    let fallback = format!("{home}/.hangar/bin/hangar-server.exe").replace('\'', "''");
    let candidate = path.replace('\'', "''");
    Some(format!("& $(if (Test-Path -LiteralPath '{candidate}') {{ '{candidate}' }} else {{ '{fallback}' }})"))
}

pub fn parse(text: &str) -> Option<(String, &str)> {
    let rest = text.strip_prefix("& $(if (Test-Path -LiteralPath '")?;
    let mut chars = rest.chars().peekable();
    let mut path = String::new();
    loop {
        match chars.next()? {
            '\'' if chars.peek() == Some(&'\'') => { chars.next(); path.push('\''); }
            '\'' => break,
            character => path.push(character),
        }
    }
    // Só a expressão emitida por nós é reconhecida; o resto continua sendo o comando original.
    let prefix = expression(&path)?;
    let args = text.strip_prefix(&prefix)?.strip_prefix(' ')?;
    Some((path, args))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_and_developer_share_one_invocation_without_reparsing_arguments() {
        for home in ["C:/Users/test", "C:/Users/Nome com espaços", "C:/Users/D'Ávila"] {
            let path = format!("{home}/.hangar/current/bin/hangar-server.exe");
            let invocation = expression(&path).expect("caminho de instalação");
            assert!(invocation.contains(&format!("{}/.hangar/bin/hangar-server.exe", home.replace('\'', "''"))));
            let tail = "\"codex-hook-json\" \"--\" \"echo a | echo b & echo %VALUE%\"";
            assert_eq!(parse(&format!("{invocation} {tail}")), Some((path.clone(), tail)));
            assert!(parse(&invocation.replace("Test-Path", "Write-Output")).is_none());
        }
        assert!(expression("C:/other/hangar-server.exe").is_none());
    }

    #[test]
    fn backslashes_and_case_keep_the_original_candidate() {
        let path = r"C:\Users\Teste\.hangar\current\bin\hangar-server.exe";
        let command = format!("{} codex-hook-allow", expression(path).unwrap());
        assert_eq!(parse(&command), Some((path.into(), "codex-hook-allow")));
    }
}
