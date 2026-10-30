//! Confirmação de hooks no app-server já vivo, com um prazo total para a troca.
use crate::{config_sync::runtime_values::ValueGuard,orchestration::{body,codec,truthy}};
use futures_util::{SinkExt,StreamExt};
use serde_json::{Value,json};
use std::{path::Path,time::Duration};
use tokio::{net::TcpStream,time::{Instant,timeout,timeout_at}};
use tokio_tungstenite::{MaybeTlsStream,WebSocketStream,tungstenite::{Message,protocol::{WebSocketConfig,CloseFrame,frame::coding::CloseCode}}};
type Socket=WebSocketStream<MaybeTlsStream<TcpStream>>;
async fn request(ws:&mut Socket,id:u64,method:&str,params:Value,deadline:Instant)->Result<Value,()> {
    let text=codec::dumps(&json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}),false);
    timeout_at(deadline,ws.send(Message::Text(text.into()))).await.map_err(|_|())?.map_err(|_|())?;
    loop {
        let message=timeout_at(deadline,ws.next()).await.map_err(|_|())?.ok_or(())?.map_err(|_|())?;
        if message.is_close(){return Err(());}
        if !message.is_text()&&!message.is_binary(){continue;}
        let value=ValueGuard(body::parse(&message.into_data()).map_err(|(_,value)|{codec::discard(value);})?);
        if value.is_object()&&value.get("id").is_some_and(|value|crate::config_sync::value_equality::equal(value,&json!(id)))&&value.get("method").is_none(){
            if value.get("error").is_some_and(truthy){return Err(());}
            return Ok(value.get("result").filter(|v|truthy(v)).map(codec::clone_value).unwrap_or_else(||json!({})));
        }
    }
}
pub async fn reload(endpoint:&str,cwd:&str,home:&Path,account:&Path)->bool {
    let deadline=Instant::now()+Duration::from_secs(15);
    let config=WebSocketConfig::default().max_message_size(None).max_frame_size(None);
    let Ok(Ok((mut ws,_)))=timeout(Duration::from_secs(5),tokio_tungstenite::connect_async_with_config(endpoint,Some(config),false)).await else{return false;};
    let result=async {
        request(&mut ws,1,"initialize",json!({"clientInfo":{"name":"hangar","title":null,"version":"0.1.0"},"capabilities":null}),deadline).await?;
        let data=ValueGuard(request(&mut ws,2,"hooks/list",json!({"cwds":[cwd]}),deadline).await?);
        let (edits,_manual)=super::trust::edits(&data,home,account,true).map_err(|_|())?;
        if !edits.is_empty(){request(&mut ws,3,"config/batchWrite",json!({"edits":edits,"reloadUserConfig":true}),deadline).await?;}
        Ok::<_,()>(())
    }.await;
    // O reparo não conserva conexão administrativa após sucesso, erro ou recusa.
    let _=timeout(Duration::from_secs(1),async {
        let _=ws.close(Some(CloseFrame{code:if result.is_ok(){CloseCode::Normal}else{CloseCode::Error},reason:"".into()})).await;
        while let Some(Ok(message))=ws.next().await {if message.is_close(){break;}}
    }).await;
    result.is_ok()
}
