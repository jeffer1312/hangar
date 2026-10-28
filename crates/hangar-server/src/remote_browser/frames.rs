//! Screencast com confirmação integral; captura periódica quando falta compositor.
use super::{
    cdp::Cdp,
    input,
    ws::{Failure, Output, send},
};
use serde_json::json;
use std::{sync::Arc, time::Duration};
use tokio::sync::Mutex;
pub(super) struct View {
    pub size: (i64, i64),
    pub width: i64,
    pub quality: i64,
}
pub(super) async fn run(
    cdp: &Cdp,
    output: &Output,
    view: &Arc<Mutex<View>>,
) -> Result<(), Failure> {
    cdp.cmd("Page.enable", json!({}), Duration::from_secs(10))
        .await?;
    let (width, quality) = {
        let view = view.lock().await;
        (
            if view.width == 0 { 1400 } else { view.width },
            view.quality,
        )
    };
    cdp.cmd("Page.startScreencast", json!({
        "format": "jpeg", "quality": quality,
        "maxWidth": width, "maxHeight": width, "everyNthFrame": 5,
    }), Duration::from_secs(10)).await?;
    let mut last = None;
    let mut wait = Duration::from_millis(1500);
    loop {
        let event = match tokio::time::timeout(wait, cdp.event()).await {
            Ok(value) => value,
            Err(_) => {
                screenshot(cdp, output, view).await?;
                wait = Duration::from_millis(10);
                continue;
            }
        };
        let p = &event["params"];
        match event["method"].as_str() {
            Some("Page.screencastFrame") => {
                wait = Duration::from_millis(1500);
                cdp.send(
                    "Page.screencastFrameAck",
                    json!({"sessionId":p["sessionId"]}),
                );
                let mut view = view.lock().await;
                if input::truth(&p["metadata"]["deviceWidth"]) {
                    view.size.0 = input::integer(&p["metadata"]["deviceWidth"])?;
                }
                if input::truth(&p["metadata"]["deviceHeight"]) {
                    view.size.1 = input::integer(&p["metadata"]["deviceHeight"])?;
                }
                let now = tokio::time::Instant::now();
                if last.is_some_and(|last| {
                    now.duration_since(last) < Duration::from_secs_f64(1.0 / 12.0)
                }) {
                    continue;
                }
                last = Some(now);
                let size = view.size;
                drop(view);
                send(output, json!({"t":"q","d":p["data"],"w":size.0,"h":size.1})).await?;
            }
            Some("Page.frameNavigated") => {
                if !input::truth(&p["frame"]["parentId"]) {
                    send(output, json!({"t":"url","url":p["frame"]["url"]})).await?;
                }
            }
            Some("Inspector.detached") => {
                return Err(Failure::connection(
                    if p["reason"] == "replaced_by_another_viewer" {
                        "a tela remota foi aberta em outro aparelho"
                    } else {
                        "o navegador desta sessão fechou"
                    },
                ));
            }
            _ => {}
        }
    }
}
async fn screenshot(cdp: &Cdp, output: &Output, view: &Arc<Mutex<View>>) -> Result<(), Failure> {
    let quality = view.lock().await.quality.max(55);
    match cdp
        .cmd(
            "Page.captureScreenshot",
            json!({"format":"jpeg","quality":quality}),
            Duration::from_secs(20),
        )
        .await
    {
        Ok(result) => {
            if input::truth(&result["result"]["data"]) {
                let size = view.lock().await.size;
                send(output,json!({"t":"q","d":result["result"]["data"],"w":size.0,"h":size.1,"lento":true})).await?;
            }
        }
        Err(error) if error.transport_closed => {
            return Err(Failure::connection("o navegador desta sessão fechou"));
        }
        Err(_) => {}
    }
    tokio::time::sleep(Duration::from_millis(400)).await;
    Ok(())
}
