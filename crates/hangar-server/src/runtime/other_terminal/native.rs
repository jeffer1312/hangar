//! Despacho pela extensão dentro da mesma posse que persiste e drena a fila.
use super::*;
use super::super::{queue::Status,receipt::ReceiptIndex};

async fn action(queue:&QueueActor,target:&OtherTarget,label:&str,action:Action)->Result<Value,RuntimeError> {
    let sequence=queue.snapshot().await.map_err(|_|failed("queue_io"))?.next_seq;
    queue.exec(target.generation,&format!("other-native:{sequence}:{label}"),clock(),action).await.map_err(|_|failed("queue_io"))
}
pub(super) async fn drain(target:&OtherTarget,queue:&QueueActor,options:&OtherOptions,io:&Arc<dyn OtherIo>,snapshot:&tokio::sync::watch::Sender<Value>)->Result<Value,RuntimeError> {
    let mut sent=0;
    if !matches!(target.provider,hangar_transcript::Provider::Pi|hangar_transcript::Provider::Omp) || target.binding.conversation.is_empty() {return Ok(json!({"drained":0}));}
    loop {
        let state=queue.snapshot().await.map_err(|_|failed("queue_io"))?;
        if state.terminal_write_blocked(&target.binding.conversation) {break;}
        let Some(row)=state.rows.iter().find(|row|row["delivered"]==false && super::super::queue::row_waits(row)) else {break};
        let id=row["id"].as_str().ok_or_else(||failed("queue_entry_invalid"))?.to_owned();
        let text=row["text"].as_str().ok_or_else(||failed("queue_entry_invalid"))?.to_owned();
        if text.trim_start().starts_with('/') {break;}
        let Some(line)=options.inbox.line_for(Some(&target.name),Some(&target.binding.pane)) else {break};
        if !io.prove(target).await? {return Err(failed("other_terminal_life_changed"));}
        let transcript=target.transcript.clone();let provider=target.provider.as_str();let conversation=target.binding.conversation.clone();
        let cursor=tokio::task::spawn_blocking(move ||ReceiptIndex::new(provider,&conversation).capture(&transcript)).await
            .map_err(|_|failed("receipt_worker"))?.map_err(|_|failed("receipt_scan"))?;
        let claimed=action(queue,target,"claim",Action::Claim {min_ts:target.created,limit:Some(1),entry_id:Some(id.clone())}).await?;
        if claimed.as_array().is_none_or(Vec::is_empty) {break;}
        let operation=format!("other-send:{id}");
        action(queue,target,"prepare",Action::Prepare {id:operation.clone(),entry_id:Some(id.clone()),payload:json!({"kind":"input","payload":{"text":text,"conversation":target.binding.conversation,"_terminal_generation":target.generation}})}).await?;
        action(queue,target,"bind",Action::BindDispatch {id:operation.clone(),cursor:serde_json::to_value(cursor).map_err(|_|failed("receipt_cursor"))?}).await?;
        let wire=format!("terminal:{}:{id}:{}",target.generation,state.next_seq);
        action(queue,target,"dispatch",Action::BeginDispatch {id:operation.clone(),wire_id:wire.clone(),staged:true}).await?;
        action(queue,target,"writing",Action::MarkWriting {id:operation.clone(),wire_id:wire}).await?;
        publish_snapshot(target,queue,snapshot,Some(true)).await?;
        let disposition=match options.inbox.deliver(&line,&text,Some(&id)).await {
            "sent"=>Disposition::Accepted,
            "sem-linha"=>Disposition::Deferred,
            _=>Disposition::Unknown,
        };
        let status=match disposition {Disposition::Accepted=>Status::Accepted,Disposition::Deferred=>Status::Deferred,Disposition::Unknown=>Status::Unknown,Disposition::Rejected=>Status::Rejected};
        let mut payload=json!({"code":if disposition==Disposition::Unknown {"other_terminal_delivery_unconfirmed"}else{"other_terminal_delivery"}});
        if disposition==Disposition::Deferred {payload["cleanup"]=json!("not_needed");}
        let reply=RuntimeReply {operation_id:operation.clone(),disposition,payload};
        action(queue,target,"finish",Action::Finish {id:operation,status,result:serde_json::to_value(reply).map_err(|_|failed("runtime_reply"))?}).await?;
        if disposition==Disposition::Deferred {action(queue,target,"not-written",Action::SetDelivered {entry_id:id,value:false,steered:false}).await?;}
        publish_snapshot(target,queue,snapshot,Some(true)).await?;
        if disposition!=Disposition::Accepted {break;}
        sent+=1;
    }
    Ok(json!({"drained":sent}))
}
pub(super) async fn input_reply(target:&OtherTarget,queue:&QueueActor,id:String)->Result<RuntimeReply,RuntimeError> {
    let state=queue.snapshot().await.map_err(|_|failed("queue_io"))?;
    let row=state.rows.iter().find(|row|row["id"]==id).ok_or_else(||failed("queue_entry_missing"))?;
    let operation=state.operations.get(&format!("other-send:{id}"));
    let disposition=if row["confirmed"]==true {Disposition::Accepted}
        else if operation.is_some_and(|operation|operation.status==Status::Unknown) {Disposition::Unknown}
        else if row["delivered"]==true {Disposition::Accepted}else{Disposition::Deferred};
    Ok(RuntimeReply {operation_id:id.clone(),disposition,payload:json!({"queued":disposition==Disposition::Deferred,"entry_id":id,"native":false})})
}
