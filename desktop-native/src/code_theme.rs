//! Cores do código no estilo do T3: a paleta Pierre (Apache-2.0), a mesma que o T3 usa no Shiki, levada aos nomes
//! de destaque do tree-sitter. O fundo e a margem continuam os do tema do app.
use gpui_kit::component::highlighter::HighlightTheme;
use serde_json::{Value, json};
use std::sync::{Arc, OnceLock};

struct Palette {
    comment: &'static str, string: &'static str, number: &'static str, constant: &'static str, keyword: &'static str,
    variable: &'static str, special: &'static str, function: &'static str, kind: &'static str, operator: &'static str,
    punctuation: &'static str, decorator: &'static str, tag: &'static str, attribute: &'static str,
    regex: &'static str, escape: &'static str,
}

// Comentário e pontuação um tom acima do Pierre no escuro: o fundo do Hangar é mais claro que o #0a0a0a dele.
const DARK: Palette = Palette {
    comment: "#8f8f8f", string: "#5ecc71", number: "#68cdf2", constant: "#ffd452", keyword: "#ff678d",
    variable: "#ffa359", special: "#ffab16", function: "#9d6afb", kind: "#d568ea", operator: "#08c0ef",
    punctuation: "#7d7d82", decorator: "#69b1ff", tag: "#ff855e", attribute: "#60d199",
    regex: "#64d1db", escape: "#61d5c0",
};

const LIGHT: Palette = Palette {
    comment: "#737373", string: "#199f43", number: "#1ca1c7", constant: "#d5a910", keyword: "#d32a61",
    variable: "#d47628", special: "#d5901c", function: "#693acf", kind: "#a631be", operator: "#08a5cf",
    punctuation: "#636363", decorator: "#1a85d4", tag: "#d5512f", attribute: "#18a46c",
    regex: "#17a5af", escape: "#16a994",
};

fn syntax(p: &Palette) -> Value {
    let c = |color: &str| json!({ "color": color });
    json!({
        "attribute": c(p.attribute), "boolean": c(p.number), "comment": c(p.comment), "comment.doc": c(p.comment),
        "constant": c(p.constant), "constructor": c(p.kind), "enum": c(p.kind), "function": c(p.function),
        "keyword": c(p.keyword), "label": c(p.special), "number": c(p.number), "operator": c(p.operator),
        "preproc": c(p.decorator), "property": c(p.variable), "punctuation": c(p.punctuation),
        "punctuation.bracket": c(p.punctuation), "punctuation.delimiter": c(p.punctuation),
        "punctuation.list_marker": c(p.keyword), "punctuation.special": c(p.keyword), "string": c(p.string),
        "string.escape": c(p.escape), "string.regex": c(p.regex), "string.special": c(p.constant),
        "string.special.symbol": c(p.string), "tag": c(p.tag), "tag.doctype": c(p.tag), "text.literal": c(p.string),
        "text.code.span": c(p.string), "title": c(p.tag), "type": c(p.kind), "variable": c(p.variable),
        "variable.special": c(p.special), "variant": c(p.constant),
        "emphasis": { "font_style": "italic" }, "emphasis.strong": { "font_weight": 700 },
        "link_text": c(p.function), "link_uri": c(p.comment),
    })
}

fn build(dark: bool) -> Arc<HighlightTheme> {
    let base = if dark { HighlightTheme::default_dark() } else { HighlightTheme::default_light() };
    let mut value = serde_json::to_value(&*base).expect("tema de destaque serializável");
    value["name"] = json!(if dark { "Hangar Dark" } else { "Hangar Light" });
    value["style"]["syntax"] = syntax(if dark { &DARK } else { &LIGHT });
    serde_json::from_value(value).map(Arc::new).unwrap_or(base)
}

pub fn highlight_theme(dark: bool) -> Arc<HighlightTheme> {
    static THEMES: [OnceLock<Arc<HighlightTheme>>; 2] = [OnceLock::new(), OnceLock::new()];
    THEMES[dark as usize].get_or_init(|| build(dark)).clone()
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_token_family_gets_its_own_color() {
        for dark in [true, false] {
            let theme = super::highlight_theme(dark);
            let color = |name: &str| theme.style(name).and_then(|style| style.color);
            for name in ["keyword", "string", "function", "type", "variable", "number", "comment", "punctuation", "operator"] {
                assert!(color(name).is_some(), "{name} sem cor");
            }
            assert_ne!(color("keyword"), color("string"));
            assert_eq!(color("variable.parameter"), color("variable"));
            assert_eq!(theme.style.editor_background, if dark { super::HighlightTheme::default_dark() } else { super::HighlightTheme::default_light() }.style.editor_background);
        }
    }
}
