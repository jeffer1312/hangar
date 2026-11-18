//! Fim de turno pelas linhas da lista: quando reler a revisão de cada sessão.
use std::collections::HashMap;

/// O que a lista diz de uma sessão a cada rodada.
#[derive(Clone, Debug, PartialEq)]
pub struct TurnRow {
    pub name: String,
    pub lifecycle_id: Option<String>,
    pub state: String,
    pub last_reply_at: Option<f64>,
    pub branch: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ended {
    pub name: String,
    /// Primeira vez que a sessão aparece: só relê quem ainda não tem resumo.
    pub first_sight: bool,
}

#[derive(Default)]
pub struct TurnTracker { seen: HashMap<String, TurnRow> }

impl TurnTracker {
    pub fn new() -> Self { Self::default() }

    /// `None` é a rodada sem linhas (lista em erro): não muda nada, senão todo nome voltaria como novo.
    pub fn ended(&mut self, rows: Option<&[TurnRow]>) -> Vec<Ended> {
        let Some(rows) = rows else { return Vec::new() };
        let mut ended = Vec::new();
        for row in rows {
            let first_sight = self.seen.get(&row.name).is_none_or(|prev| prev.lifecycle_id != row.lifecycle_id);
            let turn = self.seen.get(&row.name).is_some_and(|prev| {
                let stopped = row.state != "working";
                (prev.state == "working" && stopped)
                    || (stopped && row.last_reply_at.is_some() && row.last_reply_at != prev.last_reply_at)
                    || prev.branch != row.branch
            });
            if first_sight || turn { ended.push(Ended { name: row.name.clone(), first_sight }); }
            self.seen.insert(row.name.clone(), row.clone());
        }
        self.seen.retain(|name, _| rows.iter().any(|row| &row.name == name));
        ended
    }
}
