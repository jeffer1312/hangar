//! As respostas cruas do `hangar-send --responder` viram o corpo do `/answer`, pelo formulário da pergunta da filha.

use serde::Deserialize;
use serde_json::{Value, json};

/// Prefixo que manda o resto como texto livre, mesmo que pareça número de opção.
pub const TEXT_PREFIX: &str = "texto:";

#[derive(Deserialize)]
struct Form { #[serde(default)] questions: Vec<Item> }

#[derive(Deserialize)]
struct Item {
    #[serde(default)]
    options: Vec<Choice>,
    #[serde(rename = "multiSelect", default)]
    multi_select: bool,
}

#[derive(Deserialize)]
struct Choice { label: String }

/// Uma resposta por item do formulário (`form`, o `ask_form` da linha), na ordem; sem formulário, uma pergunta só com as
/// `line_options`. `N` ou `N,M` escolhe opções contadas de 1; `texto:…` é texto livre, no campo que vem depois das
/// opções, como no repasse ao pai. Sem o prefixo, só texto sem algarismo vira texto num item com opções: `1 3` ou
/// `opção 2` é escolha mal escrita, nunca texto calado.
pub fn answers_from_replies(form: Option<&Value>, line_options: &[String], replies: &[String]) -> Result<Vec<Value>, String> {
    let form = form.and_then(|form| Form::deserialize(form).ok()).filter(|form| !form.questions.is_empty());
    let items: Vec<(Vec<String>, bool)> = match form {
        Some(form) => form.questions.into_iter().map(|q| (q.options.into_iter().map(|o| o.label).collect(), q.multi_select)).collect(),
        None => vec![(line_options.to_vec(), false)],
    };
    if replies.len() != items.len() {
        return Err(format!("a pergunta tem {} item(ns) e vieram {} resposta(s): mande uma por item, na ordem", items.len(), replies.len()));
    }
    items.iter().zip(replies).enumerate().map(|(n, ((labels, multi), reply))| answer_item(n + 1, labels, *multi, reply)).collect()
}

fn text(item: usize, value: &str, labels: &[String]) -> Result<Value, String> {
    if value.is_empty() { return Err(format!("resposta {item} vazia")); }
    Ok(json!({"kind": "text", "value": value, "type_index": labels.len()}))
}

fn answer_item(item: usize, labels: &[String], multi: bool, reply: &str) -> Result<Value, String> {
    let reply = reply.trim();
    if let Some(free) = reply.strip_prefix(TEXT_PREFIX) { return text(item, free.trim(), labels); }
    // Item sem opções só aceita texto: um número ali é a resposta escrita.
    if labels.is_empty() { return text(item, reply, labels); }
    let Some(mut picked) = reply.split(',').map(|n| n.trim().parse::<usize>().ok()).collect::<Option<Vec<usize>>>() else {
        if reply.chars().any(|c| c.is_ascii_digit()) {
            return Err(format!("item {item}: '{reply}' não é N nem N,M; para mandar como texto, use {TEXT_PREFIX}{reply}"));
        }
        return text(item, reply, labels);
    };
    if let Some(bad) = picked.iter().find(|n| **n == 0 || **n > labels.len()) {
        return Err(format!("item {item}: a opção {bad} não existe (1 a {})", labels.len()));
    }
    picked.sort_unstable();
    picked.dedup();
    if picked.len() > 1 && !multi { return Err(format!("item {item} aceita uma opção só")); }
    let chosen: Vec<&String> = picked.iter().map(|n| &labels[n - 1]).collect();
    let indices: Vec<usize> = picked.iter().map(|n| n - 1).collect();
    Ok(json!({"kind": "option", "indices": indices, "multi": multi, "labels": chosen}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn form(items: &[(&[&str], bool)]) -> Value {
        let questions: Vec<Value> = items.iter().map(|(options, multi)| json!({"header": "h", "question": "q", "multiSelect": multi,
            "options": options.iter().map(|label| json!({"label": label})).collect::<Vec<_>>()})).collect();
        json!({"interaction_id": "q1", "questions": questions})
    }

    fn replies(items: &[&str]) -> Vec<String> { items.iter().map(|s| (*s).to_owned()).collect() }

    #[test]
    fn a_number_picks_the_option_counted_from_one_and_text_goes_after_the_options() {
        let asked = form(&[(&["src", "docs"], false), (&["a", "b", "c"], true)]);
        assert_eq!(answers_from_replies(Some(&asked), &[], &replies(&["2", "3,1"])).unwrap(), vec![
            json!({"kind": "option", "indices": [1], "multi": false, "labels": ["docs"]}),
            json!({"kind": "option", "indices": [0, 2], "multi": true, "labels": ["a", "c"]}),
        ]);
        assert_eq!(answers_from_replies(Some(&asked), &[], &replies(&["outra pasta", "b"])).unwrap()[0],
            json!({"kind": "text", "value": "outra pasta", "type_index": 2}));
    }

    #[test]
    fn the_text_prefix_sends_a_number_as_free_text() {
        let asked = form(&[(&["src", "docs"], false)]);
        assert_eq!(answers_from_replies(Some(&asked), &[], &replies(&["texto: 2"])).unwrap(), vec![json!({"kind": "text", "value": "2", "type_index": 2})]);
        assert!(answers_from_replies(Some(&asked), &[], &replies(&["texto:"])).is_err(), "texto vazio");
    }

    /// Escolha mal escrita (`1 3`, `1;3`, `2.`, `opção 2`) é recusada: nunca vai calada como texto.
    #[test]
    fn a_reply_that_does_not_fit_the_form_is_refused_before_any_answer() {
        let asked = form(&[(&["src", "docs"], true)]);
        for bad in [replies(&["3"]), replies(&["0"]), replies(&[" "]), replies(&["1", "2"]), replies(&[]),
            replies(&["1 2"]), replies(&["1;2"]), replies(&["2."]), replies(&["opção 2"])] {
            assert!(answers_from_replies(Some(&asked), &[], &bad).is_err(), "{bad:?}");
        }
        let single = form(&[(&["src", "docs"], false)]);
        assert!(answers_from_replies(Some(&single), &[], &replies(&["1,2"])).is_err(), "item de uma opção só");
    }

    #[test]
    fn without_a_form_the_line_options_answer_and_a_question_without_options_takes_text() {
        let line = ["sim".to_owned(), "não".to_owned()];
        assert_eq!(answers_from_replies(None, &line, &replies(&["1"])).unwrap(), vec![json!({"kind": "option", "indices": [0], "multi": false, "labels": ["sim"]})]);
        assert_eq!(answers_from_replies(Some(&Value::Null), &[], &replies(&["42"])).unwrap(), vec![json!({"kind": "text", "value": "42", "type_index": 0})]);
    }
}
