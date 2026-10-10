use crate::mods_support;

use hangar_server::mods::model::*;
use hangar_server::mods::tree;
use mods_support::{first_render, nth_render};
use serde_json::json;

#[test]
fn finds_button_by_key_with_its_press() {
    let tree = first_render("vitrine", "vitrine-botoes");
    let comum = tree::find(&tree, Some("vitrine"), "V15-comum", &["Button"]).unwrap();
    assert_eq!((comum.kind.as_str(), comum.plugin.as_str()), ("Button", "vitrine"));
    assert!(comum.handle > 0);
    let a = tree::find(&tree, Some("vitrine"), "V16-a", &["Button"]).unwrap();
    let b = tree::find(&tree, Some("vitrine"), "V16-b", &["Button"]).unwrap();
    assert_ne!(a.handle, b.handle, "mesmo rótulo, handle próprio (P04)");
}

#[test]
fn input_is_found_only_as_input() {
    let tree = first_render("vitrine", "vitrine-campos");
    assert!(tree::find(&tree, Some("vitrine"), "V18-campo", &["Button"]).is_none());
    assert_eq!(tree::find(&tree, Some("vitrine"), "V18-campo", &["Input"]).unwrap().kind, "Input");
}

#[test]
fn key_without_press_or_unknown_is_none() {
    let tree = json!({"type": "Box", "children": [
        "texto solto",
        {"type": "Text", "props": {"key": "k"}, "children": ["k"]},
        {"type": "Button", "props": {"key": "sem-press", "label": "x"}},
    ]});
    assert!(tree::find(&tree, Some("m"), "k", &["Button"]).is_none());
    assert!(tree::find(&tree, Some("m"), "sem-press", &["Button"]).is_none());
    assert!(tree::find(&tree, Some("m"), "nao-existe", &["Button"]).is_none());
}

#[test]
fn two_mods_with_the_same_key_are_told_apart_by_the_mod() {
    // Dois mods com a mesma `key` no mesmo lugar: o pedido diz de qual é, e cada um acha o seu.
    let tree = json!({"type": "Box", "children": [
        {"type": "Box", "children": [{"type": "Button", "props": {"key": "k", "label": "do m"}, "press": {"plugin": "m", "handle": 1}}]},
        {"type": "Button", "props": {"key": "k", "label": "do outro"}, "press": {"plugin": "outro", "handle": 2}},
    ]});
    assert_eq!(tree::find(&tree, Some("m"), "k", &["Button"]).map(|c| c.handle), Some(1));
    assert_eq!(tree::find(&tree, Some("outro"), "k", &["Button"]).map(|c| c.handle), Some(2));
    assert_eq!(tree::find(&tree, Some("terceiro"), "k", &["Button"]), None);
    assert!(!tree::ambiguous(&tree, Some("m"), "k", &["Button"]));
    assert_eq!(tree::label(&tree, "outro", "k").as_deref(), Some("do outro"));
    // Sem o mod (app de antes de o pedido levá-lo), a `key` nos dois não diz de qual é.
    assert!(tree::ambiguous(&tree, None, "k", &["Button"]));
    assert_eq!(tree::find(&tree, None, "k", &["Button"]), None);
    assert_eq!(tree::find(&tree, None, "nao-existe", &["Button"]), None);
}

#[test]
fn the_same_mod_with_the_same_key_twice_finds_none() {
    // O mesmo mod com a mesma `key` duas vezes no lugar: nenhum é o do pedido.
    let tree = json!({"type": "Box", "children": [
        {"type": "Box", "children": [{"type": "Button", "props": {"key": "k"}, "press": {"plugin": "m", "handle": 1}}]},
        {"type": "Button", "props": {"key": "k"}, "press": {"plugin": "m", "handle": 2}},
    ]});
    assert_eq!(tree::find(&tree, Some("m"), "k", &["Button"]), None);
    assert!(tree::ambiguous(&tree, Some("m"), "k", &["Button"]));
    assert!(!tree::ambiguous(&tree, Some("m"), "k", &["Input"]));
}

#[test]
fn engine_only_band_is_empty() {
    assert!(tree::is_engine_only(&first_render("sem-plugins", "above-prompt")));
    assert!(tree::is_engine_only(&first_render("vitrine", "above-prompt")), "ref 1 antes de o mod desenhar");
    // A segunda resposta é a faixa já desenhada pelo mod: um Box que contém o `engine`.
    let band = nth_render("vitrine", "above-prompt", 1);
    assert!(!tree::is_engine_only(&band));
    assert_eq!(band["type"], "Box");
    assert_eq!(band["children"][0]["type"], "engine");
}

#[test]
fn label_buttons_and_anchor() {
    let tree = json!({"type": "Box", "children": [
        {"type": "Text", "children": ["ok"]},
        {"type": "Button", "props": {"key": "a", "label": "  [ copiar ]  "}, "press": {"plugin": "m", "handle": 1}},
        {"type": "Button", "props": {"key": "b"}, "children": ["fe", "char"], "press": {"plugin": "m", "handle": 2}},
    ]});
    assert_eq!(tree::label(&tree, "m", "a").as_deref(), Some("[ copiar ]"));
    assert_eq!(tree::label(&tree, "m", "b").as_deref(), Some("fechar"));
    assert_eq!(tree::label(&tree, "m", "c"), None);
    assert_eq!(tree::count_buttons(&tree), 2);
    // Como o `band_anchor` do Python: o primeiro texto com três ou mais letras ou dígitos, em 16 caracteres;
    // o botão desenha o `label` antes dos filhos.
    assert_eq!(tree::anchor(&tree).as_deref(), Some("[ copiar ]"));
    assert_eq!(tree::anchor(&json!({"type": "Text", "children": ["V01 Vitrine uma linha na faixa"]})).as_deref(), Some("V01 Vitrine uma "));
    assert_eq!(tree::anchor(&json!(null)), None);
}

#[test]
fn terminal_refusals_use_the_phase_one_texts() {
    // O app traduz pelo código; a `msg` só aparece em cliente antigo e é a de `messages/pt.json`.
    assert_eq!(dialog_open().msg, "Há uma pergunta aberta no terminal da sessão; responda a ela antes.");
    assert_eq!(draft_in_prompt().msg, "Há texto digitado no prompt do terminal; envie ou apague antes de usar este botão pelo app.");
    assert_eq!(unreachable_pane().msg, "O terminal da sessão está estreito ou baixo demais para alcançar esse painel; aumente a janela ou use o terminal.");
    assert_eq!(not_found("x").params, json!({"rotulo": "x"}));
}

#[test]
fn label_count_sees_homonyms_and_texts_that_contain_the_label() {
    let one = json!({"type": "Box", "children": [
        {"type": "Text", "children": ["Nada aqui"]},
        {"type": "Button", "props": {"key": "a", "label": "Excluir"}, "press": {"plugin": "m", "handle": 1}},
    ]});
    assert_eq!(tree::label_count(&one, "Excluir"), 1);
    // Um homônimo em qualquer ponto da árvore, visível ou não, e o texto que só contém o rótulo.
    let twin = json!({"type": "Box", "children": [
        {"type": "Button", "props": {"key": "a", "label": "Excluir"}, "press": {"plugin": "m", "handle": 1}},
        {"type": "Box", "children": [{"type": "Button", "props": {"key": "b"}, "children": ["Exc", "luir"], "press": {"plugin": "m", "handle": 2}}]},
    ]});
    assert_eq!(tree::label_count(&twin, "Excluir"), 2);
    let text = json!({"type": "Box", "children": [
        {"type": "Text", "children": ["Excluir tudo"]},
        {"type": "Button", "props": {"key": "a", "label": "Excluir"}, "press": {"plugin": "m", "handle": 1}},
    ]});
    assert_eq!(tree::label_count(&text, "Excluir"), 2);
    let field = json!({"type": "Box", "children": [
        {"type": "Input", "props": {"key": "c", "placeholder": "Excluir o quê?"}},
        {"type": "Select", "props": {"key": "s", "options": [{"value": "x", "label": "Excluir"}]}},
        {"type": "Button", "props": {"key": "a", "label": "Excluir"}, "press": {"plugin": "m", "handle": 1}},
    ]});
    assert_eq!(tree::label_count(&field, "Excluir"), 3);
    assert_eq!(tree::label_count(&one, "  "), 0);
}
