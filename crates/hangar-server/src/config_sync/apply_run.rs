//! Ordem dos itens e execução dos CLIs; o pós-aplicar recebe o relatório parcial inteiro.
use super::{apply::Apply,bundle::{Error,ITEMS},command};
use serde_json::json;
use std::time::Duration;

pub async fn run_items(mut context:Apply,requested:&[String])->Result<Apply,Error> {
    let chosen:Vec<_>=ITEMS.iter().copied().filter(|item|requested.iter().any(|requested|requested==item)).collect();
    for (index,item) in chosen.iter().enumerate() {
        context.context.emit(json!({"phase":"apply","item":item,"entry":null,"index":index,"total":chosen.len()}));
        context.result(item);
        if !context.bundle.items.contains_key(*item) {
            context.result(item)["status"]=json!("failed");
            context.warn(item,"config_sync_item_missing",json!({"item":item}));
            continue;
        }
        let name=(*item).to_owned();
        let (mut next,mut result)=tokio::task::spawn_blocking(move || {
            let result=context.apply_item(&name);(context,result)
        }).await.map_err(|_|Error::new("config_sync_failed","falha ao aplicar a configuração",json!({"item":item})))?;
        if result.is_ok() {
            if *item=="claude_plugins" {
                let env=next.accounts.env.clone();
                result=next.install_plugins_with(move |args|command::run(args,env.clone(),Duration::from_secs(180))).await;
            }else if *item=="codex" {
                result=next.apply_codex_config().await;
            }
        }
        next.finish_item(item,result);
        context=next;
    }
    Ok(context)
}
