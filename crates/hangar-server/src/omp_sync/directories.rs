use super::*;
/// A leitura tolera perfil inválido; quem grava precisa da mesma recusa do resolvedor nativo.
pub fn agent_directory(
    home: &Path,
    env: &BTreeMap<String, String>,
    strict: bool,
) -> Result<(PathBuf, Option<String>), String> {
    match crate::accounts::secondary_auth::omp_directories(home, env) {
        Ok(d) => Ok((d.agent_dir, None)),
        Err(code) => {
            let message = directory_error(code);
            if strict {
                return Err(format!("perfil do omp inválido: {message}"));
            }
            let fallback = env
                .get("PI_CODING_AGENT_DIR")
                .filter(|s| !s.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".omp/agent"));
            Ok((fallback, Some(format!("perfil do omp ignorado: {message}"))))
        }
    }
}
pub(crate) fn directory_error(code: &str) -> String {
    if code == "omp_profile_invalid" {
        err("Nome de perfil OMP inválido")
    } else {
        err("Diretório OMP inválido")
    }
}
