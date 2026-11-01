//! Refinar o objetivo do loop por um `claude -p` efêmero (`crate::claude_p`).
use crate::claude_p::{Runner, claude_p};

pub const GOAL_MAX: usize = 2000;
const REFUSALS: [&str; 7] = ["não consigo", "nao consigo", "não posso", "nao posso", "desculpe", "não é possível", "nao e possivel"];

pub(crate) const SYSTEM: &str = "Você reescreve OBJETIVOS pra um loop autônomo de código. Boas práticas:\n\
- Objetivo é UMA coisa só e VERIFICÁVEL: descreve o RESULTADO final, não os passos.\n\
- Pequeno e concreto (ex: \"migre utils/date.ts pra date-fns mantendo o check verde\"), nunca amplo (\"refatore o projeto\").\n\
- Peça EVIDÊNCIA, não promessa: inclua \"rode o comando de verificação e mostre a saída\".\n\
- Deixe explícito: não editar arquivos de teste nem o comando de verificação; corrigir o código.\n\
A saída é SEMPRE um objetivo reescrito — NUNCA uma pergunta, recusa ou comentário. Se faltar contexto, PRESERVE a intenção e marque a lacuna inline entre <colchetes> (ex: \"Melhore o haiku em <arquivo> até <critério de qualidade>, rode o check e mostre a saída\").\n\
Responda SÓ com o objetivo reescrito em pt-BR, texto puro, sem preâmbulo, sem aspas, sem markdown.";

pub fn prompt(goal: &str, check_cmd: Option<&str>) -> String {
    let mut parts = vec![SYSTEM.to_owned(), String::new(), "Objetivo do usuário:".into(), goal.to_owned()];
    if let Some(cmd) = check_cmd.filter(|c| !c.is_empty()) {
        parts.push(String::new());
        parts.push(format!("Comando de verificação (check): {cmd}"));
    }
    parts.join("\n")
}

/// Pergunta ou recusa no lugar do objetivo é falha: o objetivo estava vago.
pub async fn refine(goal: &str, check_cmd: Option<&str>, runner: &dyn Runner) -> Result<String, String> {
    let out = claude_p(&prompt(goal, check_cmd), runner).await?;
    let lower = out.to_lowercase();
    if out.ends_with('?') || REFUSALS.iter().any(|p| lower.starts_with(p)) {
        return Err("refine falhou, tente detalhar o objetivo".into());
    }
    Ok(out.chars().take(GOAL_MAX).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claude_p::{Output, limited, order};
    use futures_util::future::BoxFuture;
    use serde_json::Value;
    use std::sync::Mutex;

    struct Fake { first: Vec<Value>, fresh: Option<Vec<Value>>, answers: Mutex<Vec<Output>>, calls: Mutex<Vec<Option<String>>> }

    impl Runner for Fake {
        fn run<'a>(&'a self, _prompt: &'a str, account: Option<&'a str>) -> BoxFuture<'a, Result<Output, String>> {
            Box::pin(async move {
                self.calls.lock().unwrap().push(account.map(str::to_owned));
                Ok(self.answers.lock().unwrap().remove(0))
            })
        }
        fn quotas(&self, fresh: bool) -> BoxFuture<'_, Result<Vec<Value>, String>> {
            Box::pin(async move { if fresh { self.fresh.clone().ok_or_else(|| "sem".into()) } else { Ok(self.first.clone()) } })
        }
    }

    fn rows(v: &Value) -> Vec<Value> { v.as_array().cloned().unwrap_or_default() }

    #[test]
    fn prompt_order_and_limit_match_the_python() {
        let g = super::super::tests::golden();
        assert_eq!(prompt("g", Some("make test")), g["prompt"].as_str().unwrap());
        for c in g["orders"].as_array().unwrap() {
            let got: Vec<Value> = order(&rows(&c["rows"])).into_iter().map(|a| a.map_or(Value::Null, Value::String)).collect();
            assert_eq!(Value::Array(got), c["order"], "{c}");
        }
        for c in g["limits"].as_array().unwrap() {
            let o = Output { code: c["exit"].as_i64().unwrap(), stdout: c["stdout"].as_str().unwrap().into(), stderr: c["stderr"].as_str().unwrap().into() };
            assert_eq!(limited(&o), c["limited"].as_bool().unwrap(), "{c}");
        }
    }

    #[tokio::test]
    async fn refine_flows_match_the_python() {
        for c in super::super::tests::golden()["flows"].as_array().unwrap() {
            let answers = c["answers"].as_array().unwrap().iter().map(|a| Output {
                code: a[0].as_i64().unwrap(), stdout: a[1].as_str().unwrap().into(), stderr: a[2].as_str().unwrap().into() }).collect();
            let fake = Fake { first: rows(&c["lidas"]), fresh: c["relidas"].as_array().cloned(), answers: Mutex::new(answers), calls: Mutex::new(Vec::new()) };
            let got = refine("g", Some("make test"), &fake).await;
            match c.get("goal") {
                Some(goal) => assert_eq!(got, Ok(goal.as_str().unwrap().to_owned()), "{}", c["label"]),
                None => assert_eq!(got, Err(c["error"].as_str().unwrap().to_owned()), "{}", c["label"]),
            }
            let calls: Vec<Value> = fake.calls.lock().unwrap().iter().map(|a| a.clone().map_or(Value::Null, Value::String)).collect();
            assert_eq!(Value::Array(calls), c["calls"], "{}", c["label"]);
        }
    }
}
