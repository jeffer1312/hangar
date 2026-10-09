//! Texto do protocolo de grupo: a fonte continua `pair_texto.py`, perguntada por
//! `/internal/pair/text`. A entrega em si é `session_write::input::deliver_text`, fora do lock.
use std::collections::BTreeMap;

use serde_json::{Value, json};

use super::orq::PythonOrq;
use crate::routes::AppState;

/// Argumentos do `pair_texto` por tipo: `group` usa me/others/task/contract/harness, `orq` só a
/// tarefa, `external` me/peer/owner.
#[derive(Clone, Debug, Default)]
pub struct ProtocolArgs {
    pub me: String,
    pub others: Vec<String>,
    pub task: String,
    pub contract: Option<String>,
    pub contract_remote: bool,
    pub harness: BTreeMap<String, String>,
    pub peer: String,
    pub owner: String,
}

/// `kind`: `group`, `orq` ou `external`. `Err` = envelope `{code, params, msg}` de aviso que não sai.
pub async fn protocol_text(st: &AppState, kind: &str, args: &ProtocolArgs) -> Result<String, Value> {
    let body = json!({"kind": kind, "me": args.me, "others": args.others, "task": args.task, "contract": args.contract,
        "contract_remote": args.contract_remote, "harness": args.harness, "peer": args.peer, "owner": args.owner});
    let failed = |code: String| json!({"code": "erro_envio_falhou", "params": {"erro": code}, "msg": format!("não deu pra enviar: {code}")});
    match PythonOrq::from_state(st).post("pair/text", body).await {
        Ok((200, reply)) => reply["text"].as_str().map(str::to_owned).ok_or_else(|| failed("groups_pair_text_invalid".into())),
        Ok((status, _)) => Err(failed(format!("groups_pair_text_status:{status}"))),
        Err(code) => Err(failed(code)),
    }
}
