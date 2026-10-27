//! Injeção de falha nas fontes da projeção, sem trocar a montagem usada pela rota.
use hangar_server::orchestration::{
    self as orq,
    panel::{self, NativeProjection, Projection},
};
use serde_json::Value;
use std::{path::Path, sync::Arc};
struct Fault<'a>(&'a str);
impl Projection for Fault<'_> {
    fn entry(&self, obj: &Value, dir: &Path) -> Result<Option<Value>, String> {
        NativeProjection.entry(obj, dir)
    }
    fn team(&self, cfg: &Value, st: &Value, events: &[Value]) -> Result<Vec<Value>, String> {
        if self.0 == "team" {
            Err("ZeroDivisionError: division by zero".into())
        } else {
            NativeProjection.team(cfg, st, events)
        }
    }
    fn decisions(
        &self,
        entries: &[(Value, Value)],
        events: &[Value],
    ) -> Result<Vec<Value>, String> {
        if self.0 == "decisions" {
            Err("ZeroDivisionError: division by zero".into())
        } else {
            NativeProjection.decisions(entries, events)
        }
    }
}
pub fn evaluate(action: &str, input: &Value) -> Option<Result<Value, String>> {
    Some(match action {
        "panel.failed" => {
            let dir = Path::new(orq::text(&input["directory"]));
            let home = Path::new(orq::text(&input["home"]));
            let (out, aux) = panel::build_with(dir, home, &Fault(orq::text(&input["block"])));
            let temp = tempfile::tempdir().unwrap();
            Ok(panel::attach_consumption(
                out,
                aux,
                dir,
                home,
                Vec::new(),
                Arc::new(super::consumption::collector(temp.path())),
            ))
        }
        "timeline.parse-failed" => Ok(serde_json::json!(orq::timeline::parse_obj_with(
            &input["obj"],
            None,
            |_, _| Err("division by zero".into())
        ))),
        _ => return None,
    })
}
