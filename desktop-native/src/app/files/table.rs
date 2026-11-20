//! CSV e TSV em tabela, como no T3: cabeçalho fixo, número da linha e só as linhas à vista desenhadas.
use super::*;
use std::{ops::Range, sync::Arc};

const MAX_COLUMNS: usize = 1024;
const MAX_CELL: usize = 1024;
const ROW_HEIGHT: f32 = 29.;
const NUMBER_WIDTH: f32 = 56.;
/// Acima disso a tabela não é montada: o arquivo abre como texto, que é leve.
pub(super) const TABLE_MAX_BYTES: usize = 32 * 1024 * 1024;

pub(super) struct TableData { header: Vec<String>, rows: Vec<Vec<String>>, widths: Vec<f32>, numeric: Vec<bool> }

pub(super) fn delimiter(path: &str) -> Option<char> {
    match crate::composer::extension(path).as_str() {
        "csv" => Some(','),
        "tsv" | "tab" => Some('\t'),
        _ => None,
    }
}

/// Campos entre aspas podem ter o separador, aspas dobradas e quebra de linha (guardada como espaço, que é como a
/// célula aparece). Cada célula guarda até MAX_CELL caracteres e cada linha até MAX_COLUMNS colunas.
pub(super) fn parse(text: &str, delim: char) -> TableData {
    let (mut rows, mut row, mut field) = (Vec::new(), Vec::new(), String::new());
    let (mut quoted, mut chars, mut len) = (false, text.chars().peekable(), 0usize);
    let push = |field: &mut String, row: &mut Vec<String>, len: &mut usize| {
        if row.len() < MAX_COLUMNS { row.push(std::mem::take(field)); } else { field.clear(); }
        *len = 0;
    };
    while let Some(ch) = chars.next() {
        if quoted {
            if ch == '"' {
                if chars.peek() == Some(&'"') { chars.next(); } else { quoted = false; continue; }
            }
        } else if ch == '"' && field.is_empty() { quoted = true; continue; }
        else if ch == delim { push(&mut field, &mut row, &mut len); continue; }
        else if ch == '\n' || ch == '\r' {
            if ch == '\r' && chars.peek() == Some(&'\n') { chars.next(); }
            push(&mut field, &mut row, &mut len);
            rows.push(std::mem::take(&mut row));
            continue;
        }
        if len < MAX_CELL { field.push(if ch == '\n' || ch == '\r' { ' ' } else { ch }); }
        len += 1;
    }
    if !field.is_empty() || !row.is_empty() { push(&mut field, &mut row, &mut len); rows.push(row); }
    let mut rows = rows.into_iter();
    let header = rows.next().unwrap_or_default();
    let rows: Vec<Vec<String>> = rows.collect();
    let columns = rows.iter().map(Vec::len).chain([header.len()]).max().unwrap_or(0);
    let widths = (0..columns).map(|col| {
        let longest = std::iter::once(&header).chain(rows.iter().take(200))
            .filter_map(|row| row.get(col)).map(|cell| cell.chars().count()).max().unwrap_or(0);
        (longest as f32 * 7.2 + 24.).clamp(128., 320.)
    }).collect();
    // Coluna só de números (nas primeiras 200 linhas) alinha à direita, para comparar grandezas.
    let numeric = (0..columns).map(|col| {
        let mut cells = rows.iter().take(200).filter_map(|row| row.get(col)).map(|cell| cell.trim()).filter(|cell| !cell.is_empty()).peekable();
        cells.peek().is_some() && cells.all(|cell| cell.parse::<f64>().is_ok())
    }).collect();
    TableData { header, rows, widths, numeric }
}

fn cell(text: &str, width: f32, numeric: bool) -> Div {
    div().w(px(width)).flex_shrink_0().h_full().px_3().flex().items_center().when(numeric, |el| el.justify_end())
        .border_r_1().border_color(theme::border().opacity(0.4))
        .child(div().min_w_0().truncate().child(SharedString::from(text.to_owned())))
}

impl Hangar {
    pub(super) fn render_table(&self, data: Arc<TableData>, cx: &mut Context<Self>) -> AnyElement {
        if data.header.is_empty() && data.rows.is_empty() {
            return div().size_full().p_6().text_xs().text_color(theme::faint()).child(tr("file_table_empty")).into_any_element();
        }
        let total = NUMBER_WIDTH + data.widths.iter().sum::<f32>();
        let header = div().h(px(ROW_HEIGHT)).flex_shrink_0().flex().border_b_1().border_color(theme::border()).bg(theme::inset())
            .font_weight(FontWeight::MEDIUM)
            .child(div().w(px(NUMBER_WIDTH)).flex_shrink_0().border_r_1().border_color(theme::border()))
            .children(data.widths.iter().enumerate().map(|(col, width)| cell(data.header.get(col).map_or("", String::as_str), *width, data.numeric[col])));
        let rows = data.clone();
        let list = uniform_list("file-table-rows", data.rows.len(), cx.processor(move |_: &mut Self, range: Range<usize>, _, _| {
            range.map(|ix| div().id(("file-table-row", ix)).h(px(ROW_HEIGHT)).flex().border_b_1().border_color(theme::border().opacity(0.4))
                .child(div().w(px(NUMBER_WIDTH)).flex_shrink_0().h_full().px_2().flex().items_center().justify_end()
                    .border_r_1().border_color(theme::border()).text_color(theme::faint()).child((ix + 1).to_string()))
                .children(rows.widths.iter().enumerate().map(|(col, width)| cell(rows.rows[ix].get(col).map_or("", String::as_str), *width, rows.numeric[col]))))
                .collect::<Vec<_>>()
        })).flex_1();
        div().id("file-table").size_full().overflow_x_scroll().text_size(px(12.))
            .child(div().w(px(total)).h_full().flex().flex_col().child(header).child(list))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{delimiter, parse};

    #[test]
    fn quoted_fields_keep_separators_quotes_and_line_breaks() {
        let data = parse("nome,obs\r\n\"Silva, Ana\",\"disse \"\"oi\"\"\nde novo\"\nB,\n", ',');
        assert_eq!(data.header, ["nome", "obs"]);
        assert_eq!(data.rows, [vec!["Silva, Ana".to_owned(), "disse \"oi\" de novo".to_owned()], vec!["B".to_owned(), String::new()]]);
        assert_eq!(data.widths.len(), 2);
        assert_eq!(parse("a,b\n1.5,x\n,y\n20,z", ',').numeric, [true, false]);
    }

    #[test]
    fn tsv_and_csv_by_extension_only() {
        assert_eq!(delimiter("a/b.CSV"), Some(','));
        assert_eq!(delimiter("dados.tsv"), Some('\t'));
        assert_eq!(delimiter("leia.md"), None);
        assert_eq!(parse("a\tb\n1\t2", '\t').rows, [vec!["1".to_owned(), "2".to_owned()]]);
    }
}
