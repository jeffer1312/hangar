//! Leitura de uma linha da projeção da fila durável.
use serde_json::Value;

/// A parte da regra que a linha sozinha responde, a única que a projeção (sem as operações) permite ler:
/// o painel da fila a usa, e a tentativa em voo que ela não vê o ator recusa como `not_waiting`.
pub fn row_waits(row:&Value)->bool {
    row["delivered"]==false && row["confirmed"]!=true && row["desistiu"]!=true && row["papel"]!="assistant" && row["reserved"]!=true
}
