//! Leitura de uma linha da projeção da fila durável.
use serde_json::Value;

/// A parte da regra que a linha sozinha responde, a única que a projeção (sem as operações) permite ler:
/// o painel da fila a usa, e a tentativa em voo que ela não vê o ator recusa como `not_waiting`.
pub fn row_waits(row:&Value)->bool {
    row["delivered"]==false && row["confirmed"]!=true && row["desistiu"]!=true && row["papel"]!="assistant" && row["reserved"]!=true
}

/// Remetente e corpo de um recado reservado da fila.
pub fn message_prefix(text: &str) -> Option<(&str, &str)> {
    let rest = text.strip_prefix('[')?;
    let (prefix, body) = rest.split_once(']')?;
    let sender = ["de:", "grupo:", "painel:"].iter().find_map(|kind| prefix.strip_prefix(kind))?.trim();
    if sender.is_empty() { return None; }
    Some((sender, body.trim_start()))
}
