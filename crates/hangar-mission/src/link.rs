//! Ticket ligado a uma sessão, pela mesma regra da tool `mission`, sem gravar nada.
use crate::model::{Item, Snapshot};
use crate::store::{Error, Kind};

/// Membro pela sessão ou pelo grupo; a vida gravada (`session_keys`) precisa conferir com `key`, senão
/// o nome reaproveitado herdaria o ticket. O ativo vence; sem ativo, o último entregue.
pub fn resolve(snapshot: &Snapshot, session: &str, key: Option<&str>, group: Option<&str>) -> Result<Option<Item>, Error> {
    let in_group = |item: &Item| group.is_some_and(|group| item.group_ids.iter().any(|id| id == group));
    let matches = snapshot.items.iter().filter(|item| {
        let member = item.sessions.iter().any(|name| name == session) || in_group(item);
        let confirmed = item.session_keys.get(session).map_or_else(|| key.is_none() || in_group(item), |expected| key == Some(expected.as_str()));
        member && confirmed
    }).collect::<Vec<_>>();
    let active = matches.iter().filter(|item| item.delivered_at.is_none()).collect::<Vec<_>>();
    if active.len() > 1 {
        return Err(Error::new(Kind::Gate, "ticket_group_conflict", "o grupo está associado a tickets diferentes; remova a associação incorreta no board"));
    }
    Ok(active.first().map(|item| (**item).clone()).or_else(|| matches.last().map(|item| (*item).clone())))
}
