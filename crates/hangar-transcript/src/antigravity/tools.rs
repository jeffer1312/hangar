//! Ferramentas do agy no vocabulário do Claude (`Bash`, `Read`, `Edit`, `TodoWrite`, `Agent`…), o único que
//! os três clientes desenham como comando, leitura, diff e painel de atividade. Mesma tradução que o leitor do Codex
//! faz no `exec`: aqui, e não em cada cliente. Ferramenta sem par, ou com argumento fora do esquema, segue como veio.
use serde_json::{json, Map, Value};

type Args = Map<String, Value>;

/// Nome e entrada que o cliente entende; sem tradução, os mesmos da chamada.
pub(super) fn canonical(name: &str, args: Args) -> (String, Args) {
    let mapped = match name {
        "run_command" => run_command(&args).map(|input| ("Bash", input)),
        "view_file" | "read_file" => read(&args).map(|input| ("Read", input)),
        "write_to_file" | "write_file" => write(&args).map(|input| ("Write", input)),
        "replace_file_content" | "multi_replace_file_content" | "edit_file" => edit(&args),
        "update_steps" => steps(&args).map(|input| ("TodoWrite", input)),
        "invoke_subagent" => subagents(&args).or_else(|| subagent(&args)),
        "exit_plan_mode" => text(&args, "plan").map(|plan| ("ExitPlanMode", object([("plan", plan.into())]))),
        "enter_plan_mode" => text(&args, "reason").map(|reason| ("EnterPlanMode", object([("reason", reason.into())]))),
        "manage_subagents" => text(&args, "Action").map(|action| ("manage_subagents", object([("action", action.into())]))),
        "manage_task" => task(&args).map(|input| ("manage_task", input)),
        "schedule" => number(&args, "DurationSeconds").map(|seconds| {
            let mut input = object([("delaySeconds", seconds.into())]);
            if let Some(prompt) = text(&args, "Prompt") { input.insert("prompt".into(), prompt.into()); }
            ("ScheduleWakeup", input)
        }),
        "send_message" => message(&args).map(|input| ("SendMessage", input)),
        "define_subagent" => text(&args, "Name").map(|name| {
            let mut input = object([("name", name.into())]);
            if let Some(description) = text(&args, "Description") { input.insert("description".into(), description.into()); }
            ("define_subagent", input)
        }),
        "browser_subagent" => browser(&args).map(|input| ("Agent", input)),
        "grep_search" => search(&args, "Query", "SearchPath").map(|input| ("Grep", input)),
        "find_by_name" => search(&args, "Pattern", "SearchDirectory").map(|input| ("Glob", input)),
        "list_dir" => text(&args, "DirectoryPath").map(|path| ("LS", object([("path", path.into())]))),
        "read_url_content" => text(&args, "Url").map(|url| ("WebFetch", object([("url", url.into())]))),
        "search_web" => text(&args, "query").map(|query| ("WebSearch", object([("query", query.into())]))),
        _ => None,
    };
    match mapped {
        Some((name, input)) => (name.to_owned(), input),
        None => (name.to_owned(), args),
    }
}

fn object<const N: usize>(pairs: [(&str, Value); N]) -> Args {
    pairs.into_iter().map(|(key, value)| (key.to_owned(), value)).collect()
}

fn text(args: &Args, key: &str) -> Option<String> { args.get(key)?.as_str().map(str::to_owned) }

fn first_text(args: &Args, keys: &[&str]) -> Option<String> { keys.iter().find_map(|key| text(args, key)) }

/// Conteúdo opcional: ausente ou `null` vale vazio; presente com outro tipo é `Err`, e a chamada segue como veio em vez de
/// virar escrita ou troca vazia que ela não pediu.
fn optional_text(args: &Args, keys: &[&str]) -> Result<String, ()> {
    match keys.iter().find_map(|key| args.get(*key)) {
        None | Some(Value::Null) => Ok(String::new()),
        Some(Value::String(s)) => Ok(s.clone()),
        Some(_) => Err(()),
    }
}

/// Número que às vezes chega como texto (`"30"`).
fn number(args: &Args, key: &str) -> Option<u64> {
    match args.get(key)? {
        Value::Number(n) => n.as_u64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn flag(args: &Args, key: &str) -> bool {
    match args.get(key) {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => s.trim().eq_ignore_ascii_case("true"),
        _ => false,
    }
}

/// Lista que o modelo às vezes manda como texto JSON, apesar de o esquema pedir array.
fn list(args: &Args, key: &str) -> Option<Vec<Value>> {
    match args.get(key)? {
        Value::Array(items) => Some(items.clone()),
        Value::String(s) => serde_json::from_str::<Value>(s).ok()?.as_array().cloned(),
        _ => None,
    }
}

fn run_command(args: &Args) -> Option<Args> {
    let mut input = object([("command", text(args, "CommandLine")?.into())]);
    if let Some(summary) = text(args, "toolSummary").filter(|s| !s.trim().is_empty()) { input.insert("description".into(), summary.into()); }
    Some(input)
}

/// `StartLine`/`EndLine` inclusivos viram o recorte do `Read`: linha inicial e quantidade.
fn read(args: &Args) -> Option<Args> {
    let mut input = object([("file_path", first_text(args, &["AbsolutePath", "Path"])?.into())]);
    let (start, end) = (number(args, "StartLine"), number(args, "EndLine"));
    if let Some(start) = start { input.insert("offset".into(), start.into()); }
    let limit = match (start, end) {
        (Some(start), Some(end)) if end >= start => Some(end - start + 1),
        (None, Some(end)) => Some(end),
        _ => None,
    };
    if let Some(limit) = limit { input.insert("limit".into(), limit.into()); }
    Some(input)
}

fn target_file(args: &Args) -> Option<String> { first_text(args, &["TargetFile", "target_file", "Path"]) }

fn write(args: &Args) -> Option<Args> {
    // Acréscimo não é o arquivo inteiro: como `Write`, os apps mostrariam um arquivo criado só com o trecho.
    if flag(args, "Append") { return None; }
    let content = optional_text(args, &["CodeContent", "Content"]).ok()?;
    Some(object([("file_path", target_file(args)?.into()), ("content", content.into())]))
}

fn replacement(item: &Args) -> Option<Args> {
    let old = first_text(item, &["TargetContent", "Target"])?;
    // Substituto vazio é apagar o trecho.
    let new = optional_text(item, &["ReplacementContent", "Replacement"]).ok()?;
    let mut edit = object([("old_string", old.into()), ("new_string", new.into())]);
    if flag(item, "AllowMultiple") { edit.insert("replace_all".into(), true.into()); }
    Some(edit)
}

/// Uma troca vira `Edit`; a lista (`ReplacementChunks` do IDE, `Edits` do CLI), `MultiEdit`.
fn edit(args: &Args) -> Option<(&'static str, Args)> {
    let path = target_file(args)?;
    let Some(chunks) = list(args, "ReplacementChunks").or_else(|| list(args, "Edits")) else {
        let mut input = replacement(args)?;
        input.insert("file_path".into(), path.into());
        return Some(("Edit", input));
    };
    let edits = chunks.iter().map(|chunk| chunk.as_object().and_then(replacement).map(Value::Object)).collect::<Option<Vec<_>>>()?;
    Some(("MultiEdit", object([("file_path", path.into()), ("edits", edits.into())])))
}

/// `done` é o `completed` do Claude; `skipped` e `failed` não têm par e seguem como vieram.
fn steps(args: &Args) -> Option<Args> {
    let todos = list(args, "steps")?.iter().filter_map(|step| {
        let title = step.get("title")?.as_str()?;
        let status = match step.get("status").and_then(Value::as_str) {
            Some("done") => "completed",
            Some(other) => other,
            None => "pending",
        };
        Some(json!({"content": title, "status": status}))
    }).collect::<Vec<_>>();
    Some(object([("todos", todos.into())]))
}

/// Uma chamada pode lançar vários subagentes: um vira `Agent`, vários, `AgentSwarm` (um resultado para todos).
fn subagents(args: &Args) -> Option<(&'static str, Args)> {
    let items = list(args, "Subagents")?;
    let entries = items.iter().filter_map(Value::as_object).collect::<Vec<_>>();
    fn label(entry: &Args) -> Option<String> { first_text(entry, &["Role", "TypeName"]) }
    match entries.as_slice() {
        [] => None,
        [entry] => {
            let entry: &Args = entry;
            let mut input = Args::new();
            if let Some(role) = label(entry) { input.insert("description".into(), role.into()); }
            if let Some(kind) = text(entry, "TypeName") { input.insert("subagent_type".into(), kind.into()); }
            if let Some(prompt) = text(entry, "Prompt") { input.insert("prompt".into(), prompt.into()); }
            Some(("Agent", input))
        }
        many => {
            let roles = many.iter().filter_map(|entry| label(entry)).collect::<Vec<_>>().join(", ");
            Some(("AgentSwarm", object([("description", roles.into()), ("items", items.clone().into())])))
        }
    }
}

/// Um subagente por chamada, que roda em segundo plano e manda o relatório depois.
fn subagent(args: &Args) -> Option<(&'static str, Args)> {
    let prompt = text(args, "Prompt")?;
    let mut input = Args::new();
    if let Some(role) = first_text(args, &["Role", "TypeName"]) { input.insert("description".into(), role.into()); }
    if let Some(kind) = text(args, "TypeName") { input.insert("subagent_type".into(), kind.into()); }
    input.insert("prompt".into(), prompt.into());
    input.insert("run_in_background".into(), true.into());
    Some(("Agent", input))
}

/// O id da tarefa sem a conversa (`task-N`), como o comando de fundo aparece no chat.
fn task(args: &Args) -> Option<Args> {
    let id = text(args, "TaskId")?;
    let mut input = object([("task_id", id.rsplit('/').next().unwrap_or(&id).into())]);
    if let Some(action) = text(args, "Action") { input.insert("action".into(), action.into()); }
    Some(input)
}

fn message(args: &Args) -> Option<Args> {
    Some(object([("to", text(args, "Recipient")?.into()), ("message", text(args, "Message")?.into())]))
}

fn browser(args: &Args) -> Option<Args> {
    let mut input = object([("subagent_type", "browser_subagent".into())]);
    if let Some(name) = text(args, "TaskName") { input.insert("description".into(), name.into()); }
    if let Some(task) = text(args, "Task") { input.insert("prompt".into(), task.into()); }
    Some(input)
}

fn search(args: &Args, pattern: &str, path: &str) -> Option<Args> {
    let mut input = object([("path", text(args, path)?.into())]);
    if let Some(pattern) = text(args, pattern) { input.insert("pattern".into(), pattern.into()); }
    Some(input)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(name: &str, args: Value) -> (String, Value) {
        let (name, input) = canonical(name, args.as_object().cloned().unwrap_or_default());
        (name, Value::Object(input))
    }

    #[test]
    fn a_command_is_bash_with_the_summary_as_description() {
        let (name, input) = call("run_command", json!({"CommandLine":"ls -la","Cwd":"/w","WaitMsBeforeAsync":5000,"toolSummary":"Listar"}));
        assert_eq!(name, "Bash");
        assert_eq!(input, json!({"command":"ls -la","description":"Listar"}));
    }

    #[test]
    fn both_readers_become_read_with_the_inclusive_range_as_offset_and_limit() {
        assert_eq!(call("view_file", json!({"AbsolutePath":"/w/a.rs","StartLine":10,"EndLine":19})),
            ("Read".to_owned(), json!({"file_path":"/w/a.rs","offset":10,"limit":10})));
        assert_eq!(call("read_file", json!({"Path":"/w/b.md","StartLine":"1","EndLine":"30"})).1, json!({"file_path":"/w/b.md","offset":1,"limit":30}));
        assert_eq!(call("read_file", json!({"Path":"/w/b.md"})).1, json!({"file_path":"/w/b.md"}));
    }

    #[test]
    fn edits_of_the_ide_and_of_the_cli_carry_old_and_new_text() {
        let (name, input) = call("replace_file_content", json!({"TargetFile":"/w/a.rs","TargetContent":"a","ReplacementContent":"b","AllowMultiple":false,"Instruction":"x"}));
        assert_eq!(name, "Edit");
        assert_eq!(input, json!({"file_path":"/w/a.rs","old_string":"a","new_string":"b"}));
        let (name, input) = call("multi_replace_file_content", json!({"TargetFile":"/w/a.rs",
            "ReplacementChunks":"[{\"TargetContent\":\"a\",\"ReplacementContent\":\"b\",\"AllowMultiple\":true},{\"TargetContent\":\"c\",\"ReplacementContent\":\"\"}]"}));
        assert_eq!(name, "MultiEdit");
        assert_eq!(input, json!({"file_path":"/w/a.rs","edits":[{"old_string":"a","new_string":"b","replace_all":true},{"old_string":"c","new_string":""}]}));
        assert_eq!(call("edit_file", json!({"Path":"/w/a.rs","Target":"a","Replacement":"b"})).1, json!({"file_path":"/w/a.rs","old_string":"a","new_string":"b"}));
        assert_eq!(call("edit_file", json!({"Path":"/w/a.rs","Edits":[{"Target":"a","Replacement":"b"}]})).0, "MultiEdit");
    }

    #[test]
    fn a_chunk_out_of_the_schema_keeps_the_original_call() {
        let args = json!({"TargetFile":"/w/a.rs","ReplacementChunks":[{"TargetContent":"a"},{"nope":1}]});
        assert_eq!(call("multi_replace_file_content", args.clone()), ("multi_replace_file_content".to_owned(), args));
    }

    #[test]
    fn writes_become_write_with_the_content() {
        assert_eq!(call("write_to_file", json!({"TargetFile":"/w/n.md","CodeContent":"# N\n","Overwrite":false})),
            ("Write".to_owned(), json!({"file_path":"/w/n.md","content":"# N\n"})));
        assert_eq!(call("write_file", json!({"Path":"/w/n.md","Content":"x"})).1, json!({"file_path":"/w/n.md","content":"x"}));
        assert_eq!(call("write_file", json!({"Path":"/w/n.md","Content":""})).1, json!({"file_path":"/w/n.md","content":""}));
        assert_eq!(call("write_file", json!({"Path":"/w/n.md"})).1, json!({"file_path":"/w/n.md","content":""}));
    }

    #[test]
    fn an_append_or_a_content_of_another_type_keeps_the_original_call() {
        for (name, args) in [
            ("write_file", json!({"Path":"/w/LOG.txt","Content":"ok\n","Append":true})),
            ("write_to_file", json!({"TargetFile":"/w/n.md","CodeContent":42})),
            ("replace_file_content", json!({"TargetFile":"/w/a.rs","TargetContent":"a","ReplacementContent":42})),
        ] {
            assert_eq!(call(name, args.clone()), (name.to_owned(), args));
        }
        assert_eq!(call("replace_file_content", json!({"TargetFile":"/w/a.rs","TargetContent":"a","ReplacementContent":""})).1,
            json!({"file_path":"/w/a.rs","old_string":"a","new_string":""}));
    }

    #[test]
    fn steps_feed_the_task_list_with_done_as_completed() {
        let (name, input) = call("update_steps", json!({"steps":[{"title":"Ler","status":"done"},{"title":"Editar","status":"in_progress"},{"title":"Testar","status":"skipped"}]}));
        assert_eq!(name, "TodoWrite");
        assert_eq!(input, json!({"todos":[{"content":"Ler","status":"completed"},{"content":"Editar","status":"in_progress"},{"content":"Testar","status":"skipped"}]}));
    }

    #[test]
    fn one_subagent_is_an_agent_and_several_are_a_swarm() {
        let (name, input) = call("invoke_subagent", json!({"Subagents":[{"TypeName":"research","Role":"Pesquisador","Prompt":"Ache X"}]}));
        assert_eq!(name, "Agent");
        assert_eq!(input, json!({"description":"Pesquisador","subagent_type":"research","prompt":"Ache X"}));
        let (name, input) = call("invoke_subagent", json!({"Subagents":[{"TypeName":"a","Prompt":"1"},{"TypeName":"b","Role":"Revisor","Prompt":"2"}]}));
        assert_eq!(name, "AgentSwarm");
        assert_eq!(input["description"], "a, Revisor");
        assert_eq!(input["items"].as_array().map(Vec::len), Some(2));
        assert_eq!(call("browser_subagent", json!({"TaskName":"Abrir","Task":"Abra a página"})).1,
            json!({"subagent_type":"browser_subagent","description":"Abrir","prompt":"Abra a página"}));
    }

    /// Um subagente por chamada, sempre em segundo plano (o relatório chega depois).
    #[test]
    fn a_flat_subagent_call_is_a_background_agent() {
        let (name, input) = call("invoke_subagent", json!({"Prompt":"Liste os arquivos","Role":"Directory Lister","TypeName":"self"}));
        assert_eq!(name, "Agent");
        assert_eq!(input, json!({"description":"Directory Lister","subagent_type":"self","prompt":"Liste os arquivos","run_in_background":true}));
    }

    #[test]
    fn plan_message_and_subagent_definition_use_the_claude_names() {
        assert_eq!(call("exit_plan_mode", json!({"plan":"1. Ler\n2. Editar"})), ("ExitPlanMode".to_owned(), json!({"plan":"1. Ler\n2. Editar"})));
        assert_eq!(call("send_message", json!({"Recipient":"eb8ac2c0-bd0c-4771-b60f-1d146a580e7a","Message":"Corrija o Cargo.toml"})),
            ("SendMessage".to_owned(), json!({"to":"eb8ac2c0-bd0c-4771-b60f-1d146a580e7a","message":"Corrija o Cargo.toml"})));
        assert_eq!(call("enter_plan_mode", json!({"reason":"T1 e T2 prontas; falta decidir T3"})), ("EnterPlanMode".to_owned(), json!({"reason":"T1 e T2 prontas; falta decidir T3"})));
        assert_eq!(call("manage_subagents", json!({"Action":"list"})), ("manage_subagents".to_owned(), json!({"action":"list"})));
        assert_eq!(call("manage_task", json!({"Action":"status","TaskId":"5837ea9c-4418-4a9b-84da-33407ae57b6f/task-157"})),
            ("manage_task".to_owned(), json!({"action":"status","task_id":"task-157"})));
        assert_eq!(call("schedule", json!({"DurationSeconds":90,"Prompt":"Verificar task-157","TimerCondition":"5837ea9c-4418-4a9b-84da-33407ae57b6f/task-157"})),
            ("ScheduleWakeup".to_owned(), json!({"delaySeconds":90,"prompt":"Verificar task-157"})));
        assert_eq!(call("define_subagent", json!({"Name":"pm-revisor","Description":"Revisor somente leitura","SystemPrompt":"Você é...","EnableWriteTools":false})),
            ("define_subagent".to_owned(), json!({"name":"pm-revisor","description":"Revisor somente leitura"})));
    }

    #[test]
    fn searches_and_the_web_use_the_claude_fields() {
        assert_eq!(call("grep_search", json!({"Query":"fn main","SearchPath":"/w","Includes":["*.rs"]})), ("Grep".to_owned(), json!({"path":"/w","pattern":"fn main"})));
        assert_eq!(call("find_by_name", json!({"SearchDirectory":"/w","Pattern":"*.md"})).0, "Glob");
        assert_eq!(call("list_dir", json!({"DirectoryPath":"/w"})), ("LS".to_owned(), json!({"path":"/w"})));
        assert_eq!(call("read_url_content", json!({"Url":"https://e.x/a"})), ("WebFetch".to_owned(), json!({"url":"https://e.x/a"})));
        assert_eq!(call("search_web", json!({"query":"agy","domain":"e.x"})), ("WebSearch".to_owned(), json!({"query":"agy"})));
    }

    #[test]
    fn a_tool_without_pair_or_without_its_required_field_stays_as_it_came() {
        for (name, args) in [("update_focus", json!({"focus":"Lendo"})), ("run_command", json!({"Cwd":"/w"})), ("view_file", json!({}))] {
            assert_eq!(call(name, args.clone()), (name.to_owned(), args));
        }
    }
}
