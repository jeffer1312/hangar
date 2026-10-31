use super::*;
use crate::config_sync::{bundle::Bundle,export::Context,paths::Roots};
use std::collections::BTreeMap;

struct NativeReport;
impl Actions for NativeReport {
    fn bridge(&self)->Step<'_,BridgeReport>{Box::pin(async {
        Ok(BridgeReport {
            stats:json!({"kimi":{"erro":1}}),
            errors:BTreeMap::from([("kimi".into(),"skills-bridge: PermissionDenied".into())]),
        })
    })}
    fn hook(&self,_name:&'static str)->Step<'_>{Box::pin(async{panic!("Hook não selecionado")})}
    fn codex(&self)->Step<'_>{Box::pin(async{panic!("Codex não selecionado")})}
}

#[tokio::test]
async fn after_apply_preserves_the_native_bridge_failure_cause() {
    let home=tempfile::tempdir().unwrap();let root=home.path().display().to_string();
    let context=Context::new(Roots{home:root.clone(),claude:format!("{root}/.claude"),codex:format!("{root}/.codex"),hangar:format!("{root}/repo")});
    let mut context=Apply::new(context,Bundle::default(),"fixture",Default::default());
    run(&mut context,&["claude_skills".into()],false,&NativeReport).await;
    let warning=&context.report["claude_skills"]["warnings"][0];
    assert_eq!(warning["code"],"config_sync_after_failed");
    assert_eq!(warning["params"]["step"],"skill_bridge:kimi");
    assert_eq!(warning["params"]["error"],"skills-bridge: PermissionDenied");
}
