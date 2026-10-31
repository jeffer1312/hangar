//! O que a pessoa já viu do tour e das dicas, em `tour.json` na pasta de `appearance.json`. O formato é o do
//! `/api/tour-progress` (`crates/hangar-server/src/tour_progress.rs`) e a junção é a mesma: nunca desfaz o que algum
//! aparelho já viu.
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, sync::Mutex};

/// A ordem das variantes é a da junção: `done` vence `skipped`, e `None` perde para os dois.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome { Skipped, Done }

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Switch { pub on: bool, pub at: String }

impl Switch {
    /// Agora, com o fuso local: a comparação é pelo instante, não pelo texto.
    pub fn now(on: bool) -> Self { Self { on, at: chrono::Local::now().to_rfc3339() } }

    fn instant(&self) -> Option<chrono::DateTime<chrono::FixedOffset>> { chrono::DateTime::parse_from_rfc3339(&self.at).ok() }
}

/// Os nomes no JSON são os do arquivo e do `/api/tour-progress`: mudam só com os dois lados.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct Seen {
    #[serde(default)] pub tour: Option<Outcome>,
    #[serde(default)] pub tips: BTreeSet<String>,
    #[serde(default, rename = "tips_off")] pub tips_switch: Option<Switch>,
    /// Só do app: os celulares cuja dica já foi respondida. Não é do servidor: `for_server` o tira.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")] pub devices: BTreeSet<String>,
}

impl Seen {
    pub fn merge(&mut self, other: Seen) {
        self.tour = self.tour.max(other.tour);
        self.tips.extend(other.tips);
        self.devices.extend(other.devices);
        if let Some(theirs) = other.tips_switch {
            let newer = match &self.tips_switch {
                Some(ours) => theirs.instant() > ours.instant(),
                None => true,
            };
            if newer { self.tips_switch = Some(theirs); }
        }
    }

    /// O corpo do `PUT /api/tour-progress`.
    pub fn for_server(&self) -> Seen { Seen { devices: BTreeSet::new(), ..self.clone() } }

    pub fn tips_off(&self) -> bool { self.tips_switch.as_ref().is_some_and(|s| s.on) }
}

fn path() -> Option<std::path::PathBuf> { crate::appearance::tour_path() }

/// Bloqueante. Sem arquivo, nada visto; ilegível também, com o motivo de volta para o diário.
pub fn load() -> Result<Seen, String> {
    match path() { Some(path) => load_at(&path), None => Ok(Seen::default()) }
}

fn load_at(path: &std::path::Path) -> Result<Seen, String> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Seen::default()),
        Err(e) => return Err(format!("arquivo ilegível em {}: {e}", path.display())),
    };
    // Corrompido não se conserta sozinho e travaria toda gravação: fica ao lado e o tour recomeça limpo.
    serde_json::from_slice(&bytes).or_else(|e| {
        let bad = path.with_extension("json.bad");
        let moved = std::fs::rename(path, &bad).map(|_| bad.display().to_string()).unwrap_or_else(|e| format!("não movido: {e}"));
        crate::log_line(&format!("tour: tour.json inválido ({e}); {moved}"));
        Ok(Seen::default())
    })
}

/// Bloqueante. Só a geração mais nova grava: gravações que terminam fora de ordem deixam no arquivo a última.
pub fn save(seen: &Seen, generation: u64) -> std::io::Result<()> {
    static WRITTEN: Mutex<u64> = Mutex::new(0);
    crate::appearance::save_json(path(), generation, &WRITTEN, seen)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seen(tour: Option<Outcome>) -> Seen { Seen { tour, ..Default::default() } }

    #[test]
    fn a_corrupted_file_is_set_aside_and_the_tour_starts_clean() {
        let dir = std::env::temp_dir().join(format!("hangar-tour-state-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("tour.json");
        std::fs::write(&file, b"{nao e json").unwrap();
        assert_eq!(load_at(&file).unwrap(), Seen::default());
        assert!(!file.exists() && dir.join("tour.json.bad").exists(), "o arquivo ruim fica ao lado, fora do caminho");
        assert_eq!(load_at(&file).unwrap(), Seen::default(), "sem arquivo, nada visto");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn switch(on: bool, at: &str) -> Option<Switch> { Some(Switch { on, at: at.to_owned() }) }

    #[test]
    fn done_beats_skipped_and_skipped_beats_nothing() {
        let mut a = seen(Some(Outcome::Done));
        a.merge(seen(Some(Outcome::Skipped)));
        assert_eq!(a.tour, Some(Outcome::Done));
        let mut b = seen(Some(Outcome::Skipped));
        b.merge(seen(Some(Outcome::Done)));
        assert_eq!(b.tour, Some(Outcome::Done));
        let mut c = seen(None);
        c.merge(seen(Some(Outcome::Skipped)));
        assert_eq!(c.tour, Some(Outcome::Skipped));
        let mut d = seen(Some(Outcome::Skipped));
        d.merge(seen(None));
        assert_eq!(d.tour, Some(Outcome::Skipped));
    }

    #[test]
    fn tips_are_the_union() {
        let mut a = Seen { tips: ["fila".to_owned(), "citar".to_owned()].into(), ..Default::default() };
        a.merge(Seen { tips: ["citar".to_owned(), "abas".to_owned()].into(), ..Default::default() });
        assert_eq!(a.tips, ["abas".to_owned(), "citar".to_owned(), "fila".to_owned()].into());
    }

    #[test]
    fn newest_tips_switch_wins_comparing_the_instant_across_zones() {
        // 12:00 em São Paulo é 15:00 UTC: depois das 14:30 UTC, apesar de o texto ser "menor".
        let mut a = Seen { tips_switch: switch(false, "2026-10-30T14:30:00+00:00"), ..Default::default() };
        a.merge(Seen { tips_switch: switch(true, "2026-10-30T12:00:00-03:00"), ..Default::default() });
        assert_eq!(a.tips_switch, switch(true, "2026-10-30T12:00:00-03:00"));
        let mut b = Seen { tips_switch: switch(true, "2026-10-30T12:00:00-03:00"), ..Default::default() };
        b.merge(Seen { tips_switch: switch(false, "2026-10-30T14:30:00+00:00"), ..Default::default() });
        assert_eq!(b.tips_switch, switch(true, "2026-10-30T12:00:00-03:00"));
        let mut c = Seen::default();
        c.merge(Seen { tips_switch: switch(true, "2026-10-30T12:00:00-03:00"), ..Default::default() });
        assert!(c.tips_off());
    }

    #[test]
    fn json_keeps_the_file_and_server_names() {
        let seen = Seen { tour: Some(Outcome::Skipped), tips: ["fila".to_owned()].into(), tips_switch: switch(true, "2026-10-30T12:00:00Z"),
            devices: ["ios:abc".to_owned()].into() };
        let json = serde_json::json!({"tour": "skipped", "tips": ["fila"], "tips_off": {"on": true, "at": "2026-10-30T12:00:00Z"},
            "devices": ["ios:abc"]});
        assert_eq!(serde_json::to_value(&seen).unwrap(), json);
        assert_eq!(serde_json::from_value::<Seen>(json).unwrap(), seen);
        assert_eq!(serde_json::to_value(Outcome::Done).unwrap(), "done");
    }

    #[test]
    fn server_copy_drops_the_local_devices_and_parses_back() {
        let local = Seen { tour: Some(Outcome::Done), devices: ["ios:abc".to_owned()].into(), ..Default::default() };
        let body = serde_json::to_value(local.for_server()).unwrap();
        assert!(body.get("devices").is_none());
        assert_eq!(body["tour"], "done");
        let back: Seen = serde_json::from_value(serde_json::json!({"tour": null, "tips": [], "tips_off": null})).unwrap();
        assert_eq!(back, Seen::default());
    }
}
