//! Desligamentos que vencem o consentimento: a variável da pessoa e as de CI.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Disabled {
    /// `HANGAR_TELEMETRY=0` ou `DO_NOT_TRACK=1`, no ambiente do processo ou no `.env` do Hangar.
    Env,
    /// `CI`, `GITHUB_ACTIONS`, `GITLAB_CI` ou `JENKINS_URL` no ambiente.
    Ci,
}

impl Disabled {
    pub const fn as_str(self) -> &'static str {
        match self {
            Disabled::Env => "env",
            Disabled::Ci => "ci",
        }
    }
}

const CI_VARS: &[&str] = &["CI", "GITHUB_ACTIONS", "GITLAB_CI", "JENKINS_URL"];

/// `key` em minúsculas.
fn says_off(key: &str, value: &str) -> bool {
    match key {
        "hangar_telemetry" => value.trim() == "0",
        "do_not_track" => value.trim() == "1",
        _ => false,
    }
}

/// `dotenv`: o `.env` do Hangar cru (`None` sem pasta de dados). No arquivo vale a última linha
/// da chave, sem diferenciar caixa, como na subida do servidor.
pub(crate) fn disabled_by(env: &dyn Fn(&str) -> Option<String>, dotenv: Option<&str>) -> Option<Disabled> {
    let pairs = dotenv.map(hangar_core::user_data::parse_env_file).unwrap_or_default();
    let off = |key: &str| {
        env(key.to_ascii_uppercase().as_str()).is_some_and(|value| says_off(key, &value))
            || pairs.iter().rev().find(|(name, _)| name == key).is_some_and(|(_, value)| says_off(key, value))
    };
    if off("hangar_telemetry") || off("do_not_track") {
        return Some(Disabled::Env);
    }
    let set = |key: &&str| env(*key).is_some_and(|value| !matches!(value.trim().to_ascii_lowercase().as_str(), "" | "0" | "false"));
    CI_VARS.iter().any(set).then_some(Disabled::Ci)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(list: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |key: &str| list.iter().find(|(name, _)| *name == key).map(|(_, value)| (*value).to_owned())
    }

    #[test]
    fn process_env_dotenv_and_ci() {
        assert_eq!(disabled_by(&vars(&[]), None), None);
        assert_eq!(disabled_by(&vars(&[("HANGAR_TELEMETRY", "0")]), None), Some(Disabled::Env));
        assert_eq!(disabled_by(&vars(&[("HANGAR_TELEMETRY", "1")]), None), None);
        assert_eq!(disabled_by(&vars(&[("DO_NOT_TRACK", "1")]), None), Some(Disabled::Env));
        assert_eq!(disabled_by(&vars(&[]), Some("Do_Not_Track=1\n")), Some(Disabled::Env));
        assert_eq!(disabled_by(&vars(&[]), Some("hangar_telemetry=0")), Some(Disabled::Env));
        assert_eq!(disabled_by(&vars(&[]), Some("DO_NOT_TRACK=1\nDO_NOT_TRACK=0\n")), None);
        assert_eq!(disabled_by(&vars(&[("CI", "true")]), None), Some(Disabled::Ci));
        assert_eq!(disabled_by(&vars(&[("JENKINS_URL", "http://ci")]), None), Some(Disabled::Ci));
        assert_eq!(disabled_by(&vars(&[("CI", "false")]), None), None);
        assert_eq!(disabled_by(&vars(&[("CI", "true")]), Some("DO_NOT_TRACK=1")), Some(Disabled::Env));
    }
}
