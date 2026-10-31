//! O que a pessoa já viu do tour e das dicas, em `tour.json` na pasta de `appearance.json`. O formato é o do
//! `/api/tour-progress` (`crates/hangar-server/src/tour_progress.rs`) e a junção é a mesma: nunca desfaz o que algum
//! aparelho já viu.
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, sync::Mutex};

/// A ordem das variantes é a da junção: `feito` vence `pulado`, e `None` perde para os dois.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome { Pulado, Feito }

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Switch { pub valor: bool, pub em: String }

impl Switch {
    /// Agora, com o fuso local: a comparação é pelo instante, não pelo texto.
    pub fn now(valor: bool) -> Self { Self { valor, em: chrono::Local::now().to_rfc3339() } }

    fn instant(&self) -> Option<chrono::DateTime<chrono::FixedOffset>> { chrono::DateTime::parse_from_rfc3339(&self.em).ok() }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct Seen {
    #[serde(default)] pub tour: Option<Outcome>,
    #[serde(default)] pub dicas: BTreeSet<String>,
    #[serde(default)] pub dicas_desligadas: Option<Switch>,
    /// Só do app: os celulares que já dispararam a dica deles. O servidor recusa campo a mais: `for_server` o tira.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")] pub aparelhos: BTreeSet<String>,
}

impl Seen {
    pub fn merge(&mut self, other: Seen) {
        self.tour = self.tour.max(other.tour);
        self.dicas.extend(other.dicas);
        self.aparelhos.extend(other.aparelhos);
        if let Some(theirs) = other.dicas_desligadas {
            let newer = match &self.dicas_desligadas {
                Some(ours) => theirs.instant() > ours.instant(),
                None => true,
            };
            if newer { self.dicas_desligadas = Some(theirs); }
        }
    }

    /// O corpo do `PUT /api/tour-progress`.
    pub fn for_server(&self) -> Seen { Seen { aparelhos: BTreeSet::new(), ..self.clone() } }

    pub fn tips_off(&self) -> bool { self.dicas_desligadas.as_ref().is_some_and(|s| s.valor) }
}

fn path() -> Option<std::path::PathBuf> { crate::appearance::tour_path() }

/// Bloqueante. Sem arquivo, nada visto; ilegível também, com o motivo de volta para o diário.
pub fn load() -> Result<Seen, String> {
    let Some(path) = path() else { return Ok(Seen::default()) };
    match std::fs::read(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Seen::default()),
        Err(e) => Err(format!("{}: {e}", path.display())),
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display())),
    }
}

/// Bloqueante. Só a geração mais nova grava: gravações que terminam fora de ordem deixam no arquivo a última.
pub fn save(seen: &Seen, generation: u64) -> std::io::Result<()> {
    static WRITTEN: Mutex<u64> = Mutex::new(0);
    let mut written = WRITTEN.lock().unwrap_or_else(|e| e.into_inner());
    if generation <= *written { return Ok(()); }
    let path = path().ok_or_else(|| std::io::Error::other("sem pasta de configuração"))?;
    std::fs::create_dir_all(path.parent().ok_or_else(|| std::io::Error::other("caminho sem pasta"))?)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(seen).map_err(std::io::Error::other)?)?;
    std::fs::rename(&tmp, &path)?;
    *written = generation;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seen(tour: Option<Outcome>) -> Seen { Seen { tour, ..Default::default() } }

    fn switch(valor: bool, em: &str) -> Option<Switch> { Some(Switch { valor, em: em.to_owned() }) }

    #[test]
    fn done_beats_skipped_and_skipped_beats_nothing() {
        let mut a = seen(Some(Outcome::Feito));
        a.merge(seen(Some(Outcome::Pulado)));
        assert_eq!(a.tour, Some(Outcome::Feito));
        let mut b = seen(Some(Outcome::Pulado));
        b.merge(seen(Some(Outcome::Feito)));
        assert_eq!(b.tour, Some(Outcome::Feito));
        let mut c = seen(None);
        c.merge(seen(Some(Outcome::Pulado)));
        assert_eq!(c.tour, Some(Outcome::Pulado));
        let mut d = seen(Some(Outcome::Pulado));
        d.merge(seen(None));
        assert_eq!(d.tour, Some(Outcome::Pulado));
    }

    #[test]
    fn tips_are_the_union() {
        let mut a = Seen { dicas: ["fila".to_owned(), "citar".to_owned()].into(), ..Default::default() };
        a.merge(Seen { dicas: ["citar".to_owned(), "abas".to_owned()].into(), ..Default::default() });
        assert_eq!(a.dicas, ["abas".to_owned(), "citar".to_owned(), "fila".to_owned()].into());
    }

    #[test]
    fn newest_tips_switch_wins_comparing_the_instant_across_zones() {
        // 12:00 em São Paulo é 15:00 UTC: depois das 14:30 UTC, apesar de o texto ser "menor".
        let mut a = Seen { dicas_desligadas: switch(false, "2026-10-30T14:30:00+00:00"), ..Default::default() };
        a.merge(Seen { dicas_desligadas: switch(true, "2026-10-30T12:00:00-03:00"), ..Default::default() });
        assert_eq!(a.dicas_desligadas, switch(true, "2026-10-30T12:00:00-03:00"));
        let mut b = Seen { dicas_desligadas: switch(true, "2026-10-30T12:00:00-03:00"), ..Default::default() };
        b.merge(Seen { dicas_desligadas: switch(false, "2026-10-30T14:30:00+00:00"), ..Default::default() });
        assert_eq!(b.dicas_desligadas, switch(true, "2026-10-30T12:00:00-03:00"));
        let mut c = Seen::default();
        c.merge(Seen { dicas_desligadas: switch(true, "2026-10-30T12:00:00-03:00"), ..Default::default() });
        assert!(c.tips_off());
    }

    #[test]
    fn server_copy_drops_the_local_devices_and_parses_back() {
        let local = Seen { tour: Some(Outcome::Feito), aparelhos: ["ios:abc".to_owned()].into(), ..Default::default() };
        let body = serde_json::to_value(local.for_server()).unwrap();
        assert!(body.get("aparelhos").is_none());
        assert_eq!(body["tour"], "feito");
        let back: Seen = serde_json::from_value(serde_json::json!({"tour": null, "dicas": [], "dicas_desligadas": null})).unwrap();
        assert_eq!(back, Seen::default());
    }
}
