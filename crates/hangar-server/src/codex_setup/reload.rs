//! Confirmação de hooks no app-server já vivo, com um prazo total para a troca.
use crate::{config_sync::runtime_values::ValueGuard,orchestration::{body,codec,truthy}};
use futures_util::{SinkExt,StreamExt};
use serde_json::{Value,json};
use std::{path::Path,time::Duration};
use tokio::{net::TcpStream,time::{Instant,timeout,timeout_at}};
use tokio_tungstenite::{MaybeTlsStream,WebSocketStream,tungstenite::{Message,protocol::{WebSocketConfig,CloseFrame,frame::coding::CloseCode}}};
pub(crate) type Socket=WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Por que um pedido ao app-server não voltou com resultado; o lançador mostra a etapa e isto.
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub(crate) enum RpcFailure {
    /// O prazo total acabou.
    Timeout,
    /// O app-server fechou a conexão.
    Closed,
    /// Conexão que não abriu ou caiu no meio.
    Transport,
    /// Mensagem que não é JSON, ou resultado que a decisão não aceita.
    Invalid,
    /// O app-server respondeu com erro; o código só quando é inteiro.
    Refused(Option<i64>),
}

/// Abre o WebSocket do app-server em até 5 s, sem teto de mensagem.
pub(crate) async fn connect(endpoint:&str,token:Option<&str>)->Result<Socket,RpcFailure> {
    let config=WebSocketConfig::default().max_message_size(None).max_frame_size(None);
    let request=hangar_codex::client::ws_request(endpoint,token).map_err(|_|RpcFailure::Transport)?;
    match timeout(Duration::from_secs(5),tokio_tungstenite::connect_async_with_config(request,Some(config),false)).await {
        Ok(Ok((ws,_)))=>Ok(ws),
        Ok(Err(_))=>Err(RpcFailure::Transport),
        Err(_)=>Err(RpcFailure::Timeout),
    }
}

/// Um pedido JSON-RPC e a resposta com o mesmo id; notificações no meio não esticam o prazo.
pub(crate) async fn request(ws:&mut Socket,id:u64,method:&str,params:Value,deadline:Instant)->Result<Value,RpcFailure> {
    let text=codec::dumps(&json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}),false);
    timeout_at(deadline,ws.send(Message::Text(text.into()))).await.map_err(|_|RpcFailure::Timeout)?.map_err(|_|RpcFailure::Transport)?;
    loop {
        let message=timeout_at(deadline,ws.next()).await.map_err(|_|RpcFailure::Timeout)?.ok_or(RpcFailure::Closed)?.map_err(|_|RpcFailure::Transport)?;
        if message.is_close(){return Err(RpcFailure::Closed);}
        if !message.is_text()&&!message.is_binary(){continue;}
        let value=ValueGuard(body::parse(&message.into_data()).map_err(|(_,value)|{codec::discard(value);RpcFailure::Invalid})?);
        if value.is_object()&&value.get("id").is_some_and(|value|crate::config_sync::value_equality::equal(value,&json!(id)))&&value.get("method").is_none(){
            if let Some(error)=value.get("error").filter(|error|truthy(error)){
                // Só inteiro de verdade vira código; booleano e fração não dizem nada.
                let code=error.get("code").filter(|code|code.is_i64()||code.is_u64()).and_then(Value::as_i64);
                return Err(RpcFailure::Refused(code));
            }
            return Ok(value.get("result").filter(|v|truthy(v)).map(codec::clone_value).unwrap_or_else(||json!({})));
        }
    }
}

/// Fecha a conexão administrativa em até 1 s, com o código do resultado.
pub(crate) async fn close(mut ws:Socket,ok:bool) {
    let _=timeout(Duration::from_secs(1),async {
        let _=ws.close(Some(CloseFrame{code:if ok{CloseCode::Normal}else{CloseCode::Error},reason:"".into()})).await;
        while let Some(Ok(message))=ws.next().await {if message.is_close(){break;}}
    }).await;
}

/// O `clientInfo` do `initialize`, o mesmo do lançador.
pub(crate) fn client_info()->Value { json!({"name":"hangar","title":null,"version":"0.1.0"}) }

pub async fn reload(endpoint:&str,token:Option<&str>,cwd:&str,home:&Path,account:&Path)->bool {
    let deadline=Instant::now()+Duration::from_secs(15);
    let Ok(mut ws)=connect(endpoint,token).await else{return false;};
    let result=async {
        request(&mut ws,1,"initialize",json!({"clientInfo":client_info(),"capabilities":null}),deadline).await.map_err(|_|())?;
        let data=ValueGuard(request(&mut ws,2,"hooks/list",json!({"cwds":[cwd]}),deadline).await.map_err(|_|())?);
        let (edits,_manual)=super::trust::edits(&data,home,account,true).map_err(|_|())?;
        if !edits.is_empty(){request(&mut ws,3,"config/batchWrite",json!({"edits":edits,"reloadUserConfig":true}),deadline).await.map_err(|_|())?;}
        Ok::<_,()>(())
    }.await;
    // O reparo não conserva conexão administrativa após sucesso, erro ou recusa.
    close(ws,result.is_ok()).await;
    result.is_ok()
}
