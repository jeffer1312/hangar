//! Os passos de uma chamada, tirados do `stream-json` do CLI, para a tela "Como sonhou". Só o que o
//! modelo disse e as ferramentas que pediu: o que elas devolvem (o texto das conversas lidas) não fica.

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// O que se mostra de um texto escrito numa nota; o arquivo inteiro está no "O que mudou".
pub const DETAIL_MAX: usize = 4000;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Step {
    Text { text: String },
    Tool { tool: String, target: Option<String>, detail: Option<String> },
}

pub fn parse(line: &str, workdir: &Path) -> Vec<Step> {
    let Ok(value) = serde_json::from_str::<Value>(line) else { return Vec::new() };
    if value["type"] != "assistant" {
        return Vec::new();
    }
    let Some(content) = value["message"]["content"].as_array() else { return Vec::new() };
    content
        .iter()
        .filter_map(|item| match item["type"].as_str()? {
            "text" => {
                let text = item["text"].as_str()?.trim();
                (!text.is_empty()).then(|| Step::Text { text: text.to_string() })
            }
            "tool_use" => {
                let input = &item["input"];
                let target = ["file_path", "pattern", "path"].iter().find_map(|k| input[*k].as_str()).map(|t| relative(t, workdir));
                let detail = ["content", "new_string"].iter().find_map(|k| input[*k].as_str()).map(cut);
                Some(Step::Tool { tool: item["name"].as_str()?.to_string(), target, detail })
            }
            _ => None,
        })
        .collect()
}

fn relative(path: &str, workdir: &Path) -> String {
    Path::new(path).strip_prefix(workdir).map_or_else(|_| path.to_string(), |p| p.to_string_lossy().into_owned())
}

fn cut(text: &str) -> String {
    match text.char_indices().nth(DETAIL_MAX) {
        Some((at, _)) => format!("{}…", &text[..at]),
        None => text.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_what_the_model_did_and_drops_what_it_read() {
        let work = Path::new("/w/run-1");
        let read = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Read","input":{"file_path":"/w/run-1/inputs/s0-p0.txt"}}]}}"#;
        let result = r#"{"type":"user","message":{"content":[{"type":"tool_result","content":"USER: senha do banco"}]}}"#;
        let write = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Há um fato novo."},{"type":"tool_use","name":"Write","input":{"file_path":"/w/run-1/memory/deploy.md","content":"porta 8765"}}]}}"#;
        let glob = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Glob","input":{"pattern":"memory/*.md"}}]}}"#;
        assert_eq!(parse(read, work), [Step::Tool { tool: "Read".into(), target: Some("inputs/s0-p0.txt".into()), detail: None }]);
        assert!(parse(result, work).is_empty());
        assert!(parse(r#"{"type":"result","total_cost_usd":0.1}"#, work).is_empty());
        assert!(parse("lixo", work).is_empty());
        assert_eq!(
            parse(write, work),
            [
                Step::Text { text: "Há um fato novo.".into() },
                Step::Tool { tool: "Write".into(), target: Some("memory/deploy.md".into()), detail: Some("porta 8765".into()) },
            ]
        );
        assert_eq!(parse(glob, work), [Step::Tool { tool: "Glob".into(), target: Some("memory/*.md".into()), detail: None }]);
    }

    #[test]
    fn long_writes_are_cut() {
        let content = "é".repeat(DETAIL_MAX + 10);
        let line = serde_json::json!({"type":"assistant","message":{"content":[{"type":"tool_use","name":"Write","input":{"file_path":"/w/memory/a.md","content":content}}]}}).to_string();
        let Step::Tool { detail: Some(detail), .. } = &parse(&line, Path::new("/w"))[0] else { panic!() };
        assert_eq!(detail.chars().count(), DETAIL_MAX + 1);
    }
}
