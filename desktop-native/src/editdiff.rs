//! Diff das chamadas que editam arquivo (Edit, MultiEdit, Write, `apply_patch` do Codex), o `editdiff.ts` do web.
//! Fonte = a entrada da chamada, não o arquivo: os números de linha são do trecho editado, a partir de 1.
use std::{cell::RefCell, collections::HashMap, rc::Rc};
use serde_json::Value;
use crate::api::dto::ChatEvent;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op { Same, Del, Add }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line { pub op: Op, pub old: Option<u32>, pub new: Option<u32>, pub text: String }

/// Uma edição: o arquivo, os dois lados (tab já virou espaço, para o realce e a linha medirem igual) e o diff.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit { pub path: String, pub old: String, pub new: String, pub lines: Vec<Line>, pub added: usize, pub removed: usize }

/// Acima disto o Myers para e o meio vira remoção + adição em bloco: diff válido, só não mínimo, e nunca trava.
const MAX_D: usize = 1_000;

fn split(text: &str) -> Vec<&str> {
    if text.is_empty() { return Vec::new(); }
    text.strip_suffix('\n').unwrap_or(text).split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)).collect()
}

/// Myers O(ND) no trecho que sobra depois de tirar começo e fim iguais. `None` = passou do `MAX_D`.
fn myers(a: &[&str], b: &[&str]) -> Option<Vec<Op>> {
    let (n, m) = (a.len() as isize, b.len() as isize);
    let max = (a.len() + b.len()).min(MAX_D) as isize;
    let off = max + 1;
    let mut v = vec![0isize; 2 * off as usize + 1];
    // Só a faixa -d..=d de cada passo: a memória cresce com D², não com D·(N+M).
    let mut trace: Vec<Vec<isize>> = Vec::new();
    let mut found = None;
    'outer: for d in 0..=max {
        for k in (-d..=d).step_by(2) {
            let mut x = if k == -d || (k != d && v[(k - 1 + off) as usize] < v[(k + 1 + off) as usize]) { v[(k + 1 + off) as usize] }
                else { v[(k - 1 + off) as usize] + 1 };
            let mut y = x - k;
            while x < n && y < m && a[x as usize] == b[y as usize] { x += 1; y += 1; }
            v[(k + off) as usize] = x;
            if x >= n && y >= m { found = Some(d); break 'outer; }
        }
        trace.push(v[(off - d) as usize..=(off + d) as usize].to_vec());
    }
    let found = found?;
    let mut ops = Vec::new();
    let (mut x, mut y) = (n, m);
    for d in (1..=found).rev() {
        let prev = &trace[d as usize - 1];
        let at = |k: isize| prev[(k + d - 1) as usize];
        let k = x - y;
        let prev_k = if k == -d || (k != d && at(k - 1) < at(k + 1)) { k + 1 } else { k - 1 };
        let prev_x = at(prev_k);
        let prev_y = prev_x - prev_k;
        while x > prev_x && y > prev_y { ops.push(Op::Same); x -= 1; y -= 1; }
        if x == prev_x { ops.push(Op::Add); y -= 1; } else { ops.push(Op::Del); x -= 1; }
    }
    while x > 0 && y > 0 { ops.push(Op::Same); x -= 1; y -= 1; }
    ops.extend(std::iter::repeat_n(Op::Del, x as usize));
    ops.extend(std::iter::repeat_n(Op::Add, y as usize));
    ops.reverse();
    Some(ops)
}

/// Diff linha a linha de dois textos. Num bloco alterado as remoções vêm antes das adições, como no `git diff`.
pub fn diff(old: &str, new: &str) -> Vec<Line> {
    let (a, b) = (split(old), split(new));
    let pre = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let suf = a[pre..].iter().rev().zip(b[pre..].iter().rev()).take_while(|(x, y)| x == y).count();
    let (mid_a, mid_b) = (&a[pre..a.len() - suf], &b[pre..b.len() - suf]);
    let middle = myers(mid_a, mid_b).unwrap_or_else(|| {
        std::iter::repeat_n(Op::Del, mid_a.len()).chain(std::iter::repeat_n(Op::Add, mid_b.len())).collect()
    });
    let ops = std::iter::repeat_n(Op::Same, pre).chain(middle).chain(std::iter::repeat_n(Op::Same, suf));
    let (mut lines, mut o, mut n) = (Vec::new(), 0usize, 0usize);
    let mut adds: Vec<Line> = Vec::new();
    for op in ops {
        match op {
            Op::Same => {
                lines.append(&mut adds);
                lines.push(Line { op, old: Some(o as u32 + 1), new: Some(n as u32 + 1), text: a[o].to_owned() });
                o += 1; n += 1;
            }
            Op::Del => { lines.push(Line { op, old: Some(o as u32 + 1), new: None, text: a[o].to_owned() }); o += 1; }
            Op::Add => { adds.push(Line { op, old: None, new: Some(n as u32 + 1), text: b[n].to_owned() }); n += 1; }
        }
    }
    lines.append(&mut adds);
    lines
}

fn edit(path: &str, old: &str, new: &str) -> Edit {
    let (old, new) = (old.replace('\t', "    "), new.replace('\t', "    "));
    let lines = diff(&old, &new);
    let added = lines.iter().filter(|l| l.op == Op::Add).count();
    let removed = lines.iter().filter(|l| l.op == Op::Del).count();
    Edit { path: path.to_owned(), old, new, lines, added, removed }
}

/// Um par antigo/novo nos dois dialetos: Claude (`old_string`/`new_string`) e Pi (`oldText`/`newText`).
fn pair(value: &Value) -> Option<(&str, &str)> {
    let text = |a: &str, b: &str| value.get(a).or_else(|| value.get(b)).and_then(Value::as_str);
    Some((text("old_string", "oldText")?, text("new_string", "newText")?))
}

/// O patch do Codex: um bloco por `*** <verbo> File: <caminho>`; contexto entra nos dois lados, `-` no velho, `+` no novo.
fn patch_edits(patch: &str) -> Vec<Edit> {
    let mut out = Vec::new();
    let mut block: Option<(String, Vec<&str>, Vec<&str>)> = None;
    let close = |block: &mut Option<(String, Vec<&str>, Vec<&str>)>, out: &mut Vec<Edit>| {
        // Bloco sem linha nenhuma (`Delete File` sozinho) não vira diff: "sem mudança" seria mentira.
        if let Some((path, old, new)) = block.take().filter(|(_, o, n)| !o.is_empty() || !n.is_empty()) {
            out.push(edit(&path, &old.join("\n"), &new.join("\n")));
        }
    };
    for line in patch.split('\n') {
        let file = ["Add", "Update", "Delete"].iter().find_map(|verb| line.strip_prefix(&format!("*** {verb} File: ")));
        if let Some(path) = file {
            close(&mut block, &mut out);
            block = Some((path.trim().to_owned(), Vec::new(), Vec::new()));
            continue;
        }
        // Begin/End Patch, Move to, End of File: marcas do formato; conteúdo sempre começa com espaço, `+` ou `-`.
        if line.starts_with("*** ") { if line.starts_with("*** End Patch") { close(&mut block, &mut out); } continue; }
        let Some((_, old, new)) = block.as_mut() else { continue };
        // `@@` é cabeçalho de trecho e `\ No newline` é nota do diff: nenhum dos dois é linha do arquivo.
        if line.starts_with("@@") || line.starts_with('\\') { continue; }
        if let Some(rest) = line.strip_prefix('-') { old.push(rest); }
        else if let Some(rest) = line.strip_prefix('+') { new.push(rest); }
        else { let same = line.strip_prefix(' ').unwrap_or(line); old.push(same); new.push(same); }
    }
    close(&mut block, &mut out);
    out
}

/// As edições da chamada, ou `None` quando não é edição de arquivo ou o formato é outro (aí fica a entrada crua).
pub fn edits(name: Option<&str>, input: Option<&Value>) -> Option<Vec<Edit>> {
    let input = input?;
    let path = match input.get("file_path").or_else(|| input.get("path")) {
        Some(Value::String(path)) => path.as_str(),
        Some(Value::Array(list)) => list.first().and_then(Value::as_str).unwrap_or(""),
        _ => "",
    };
    let out = match name?.to_ascii_lowercase().as_str() {
        // Write não traz o conteúdo anterior: tudo entra como adição, como o próprio Claude Code desenha.
        "write" => vec![edit(path, "", input.get("content").and_then(Value::as_str).filter(|c| !c.is_empty())?)],
        "apply_patch" => patch_edits(input.get("patch").or_else(|| input.get("code")).and_then(Value::as_str)?),
        "edit" | "multiedit" => match pair(input) {
            Some((old, new)) => vec![edit(path, old, new)],
            None => input.get("edits")?.as_array()?.iter().map(|e| pair(e).map(|(o, n)| edit(path, o, n))).collect::<Option<_>>()?,
        },
        _ => return None,
    };
    (!out.is_empty()).then_some(out)
}

thread_local! {
    static CACHE: RefCell<HashMap<String, Option<Rc<[Edit]>>>> = RefCell::new(HashMap::new());
}

/// As edições da chamada, calculadas uma vez: o desenho pede a cada quadro, e o Myers de uma edição grande pesa.
// ponytail: o cache inteiro some ao passar de 512 chamadas; um LRU entra se a troca de sessão ficar lenta.
pub fn of(call: &ChatEvent) -> Option<Rc<[Edit]>> {
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some(found) = cache.get(&call.id) { return found.clone(); }
        if cache.len() >= 512 { cache.clear(); }
        let found: Option<Rc<[Edit]>> = edits(call.tool_name.as_deref(), call.tool_input.clone().map(Value::Object).as_ref()).map(Into::into);
        cache.insert(call.id.clone(), found.clone());
        found
    })
}

/// Linhas postas e tiradas na chamada inteira.
pub fn totals(edits: &[Edit]) -> (usize, usize) {
    edits.iter().fold((0, 0), |(a, r), e| (a + e.added, r + e.removed))
}

#[cfg(test)]
mod tests {
    use core::prelude::v1::test;
    use serde_json::json;
    use super::*;

    fn marked(old: &str, new: &str) -> Vec<String> {
        diff(old, new).into_iter().map(|l| format!("{}{}", match l.op { Op::Same => ' ', Op::Del => '-', Op::Add => '+' }, l.text)).collect()
    }

    #[test]
    fn changed_block_lists_removals_before_additions_with_both_numbers() {
        let lines = diff("a\nb\nc\nd", "a\nB\nC\nd");
        assert_eq!(lines.iter().map(|l| (l.op, l.old, l.new)).collect::<Vec<_>>(), vec![
            (Op::Same, Some(1), Some(1)), (Op::Del, Some(2), None), (Op::Del, Some(3), None),
            (Op::Add, None, Some(2)), (Op::Add, None, Some(3)), (Op::Same, Some(4), Some(4)),
        ]);
    }

    #[test]
    fn diff_is_minimal_and_rebuilds_both_sides() {
        let (old, new) = ("x\na\nb\nc\ny\nz", "a\nb\nq\nc\nz\nw");
        let lines = diff(old, new);
        let side = |keep: Op| lines.iter().filter(|l| l.op != keep).map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n");
        assert_eq!(side(Op::Add), old);
        assert_eq!(side(Op::Del), new);
        assert_eq!(lines.iter().filter(|l| l.op != Op::Same).count(), 4, "x, y saem; q, w entram; a b c z ficam");
    }

    #[test]
    fn edges_pure_insert_remove_identical_and_trailing_newline() {
        assert_eq!(marked("", "a\nb"), ["+a", "+b"]);
        assert_eq!(marked("a\nb", ""), ["-a", "-b"]);
        assert_eq!(marked("a\nb\n", "a\nb"), [" a", " b"]);
        assert_eq!(marked("a\r\nb", "a\nb"), [" a", " b"]);
        assert!(diff("", "").is_empty());
    }

    #[test]
    fn past_the_limit_falls_back_to_a_block_without_hanging() {
        let old: String = (0..3000).map(|i| format!("o{i}\n")).collect();
        let new: String = (0..3000).map(|i| format!("n{i}\n")).collect();
        let lines = diff(&format!("head\n{old}tail"), &format!("head\n{new}tail"));
        assert_eq!(lines.len(), 6002);
        assert_eq!((lines[0].op, lines[1].op, lines[3001].op, lines[6001].op), (Op::Same, Op::Del, Op::Add, Op::Same));
    }

    #[test]
    fn extracts_edit_multiedit_pi_and_write() {
        let one = edits(Some("Edit"), Some(&json!({"file_path": "/a.rs", "old_string": "a\n\tb", "new_string": "a\n\tc"}))).unwrap();
        assert_eq!((one[0].path.as_str(), one[0].added, one[0].removed, one[0].new.as_str()), ("/a.rs", 1, 1, "a\n    c"));
        let multi = edits(Some("MultiEdit"), Some(&json!({"file_path": "/a", "edits": [
            {"old_string": "x", "new_string": "y"}, {"old_string": "p", "new_string": "p\nq"}]}))).unwrap();
        assert_eq!(totals(&multi), (2, 1));
        let pi = edits(Some("edit"), Some(&json!({"path": "/p", "edits": [{"oldText": "a", "newText": "b"}]}))).unwrap();
        assert_eq!((pi[0].path.as_str(), totals(&pi)), ("/p", (1, 1)));
        let write = edits(Some("Write"), Some(&json!({"file_path": "/w", "content": "1\n2\n3\n"}))).unwrap();
        assert_eq!(totals(&write), (3, 0));
        assert!(edits(Some("Write"), Some(&json!({"file_path": "/w", "content": ""}))).is_none());
        assert!(edits(Some("Edit"), Some(&json!({"file_path": "/a"}))).is_none());
        assert!(edits(Some("Read"), Some(&json!({"file_path": "/a"}))).is_none());
    }

    #[test]
    fn apply_patch_splits_files_and_skips_format_marks() {
        let patch = ["*** Begin Patch", "*** Update File: /a.py", "*** Move to: /b.py", "@@ def f():", " keep", "-um",
            "\\ No newline at end of file", "+dois", "+*** divisor ***", "*** End of File", "*** Add File: /c.md", "+novo",
            "*** Delete File: /d.txt", "*** End Patch"].join("\n");
        let found = edits(Some("apply_patch"), Some(&json!({"code": patch, "file_path": ["/a.py", "/c.md"]}))).unwrap();
        assert_eq!(found.iter().map(|e| (e.path.as_str(), e.old.as_str(), e.new.as_str())).collect::<Vec<_>>(), vec![
            ("/a.py", "keep\num", "keep\ndois\n*** divisor ***"), ("/c.md", "", "novo"),
        ]);
        assert!(edits(Some("apply_patch"), Some(&json!({"code": "nada disso"}))).is_none());
    }
}
