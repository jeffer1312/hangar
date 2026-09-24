//! Tabela markdown que vira gráfico sob demanda, como no web: uma coluna numérica de cada vez, porque
//! numa tabela real as colunas têm unidades diferentes e juntas na mesma escala uma some.

#[derive(Clone, Debug, PartialEq)]
pub struct Column {
    pub title: String,
    pub values: Vec<f64>,
    /// Toda célula termina em "%": o rótulo do gráfico devolve a unidade.
    pub percent: bool,
}

/// Tabela que dá gráfico: linhas `start..end` do texto, o rótulo de cada linha e as colunas numéricas.
#[derive(Clone, Debug, PartialEq)]
pub struct Table { pub start: usize, pub end: usize, pub labels: Vec<String>, pub columns: Vec<Column> }

/// "46,9M", "16k", "1.234,56", "-3,5%" → número; `None` quando a célula não é número. `decimal` é o separador
/// decimal do idioma da tela: decide o caso ambíguo de um ponto só seguido de 3 dígitos ("1.234" em pt é mil).
pub fn parse_number(raw: &str, decimal: char) -> Option<f64> {
    let s = raw.trim().trim_end_matches('%').trim_end();
    let (body, mult) = match s.chars().last()? {
        'k' | 'K' => (&s[..s.len() - 1], 1e3),
        'm' | 'M' => (&s[..s.len() - 1], 1e6),
        'b' | 'B' => (&s[..s.len() - 1], 1e9),
        _ => (s, 1.),
    };
    let body: String = body.chars().filter(|c| !c.is_whitespace()).collect();
    let digits = body.trim_start_matches(['+', '-']);
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit() || c == '.' || c == ',') || !digits.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    // O separador que aparece por último é o decimal: "1.234,56" e "1,234.56" dão o mesmo número.
    let normal = match (body.rfind(','), body.rfind('.')) {
        (Some(comma), Some(dot)) if comma > dot => body.replace('.', "").replace(',', "."),
        (Some(_), Some(_)) => body.replace(',', ""),
        // Só vírgula: decimal com até 2 dígitos depois ("46,9"), senão milhar ("1,234").
        (Some(comma), None) => if body.len() - comma - 1 <= 2 { body.replacen(',', ".", 1) } else { body.replace(',', "") },
        (None, Some(_)) if body.matches('.').count() > 1 => {
            // Vários pontos só são milhar em grupos de 3; "1.6.32" é versão, não número.
            let groups: Vec<&str> = body.split('.').collect();
            if !groups[1..].iter().all(|g| g.len() == 3) { return None; }
            groups.concat()
        }
        // Um ponto só com 3 dígitos depois: milhar onde a vírgula é o decimal.
        (None, Some(dot)) if decimal == ',' && body.len() - dot - 1 == 3 => body.replace('.', ""),
        _ => body,
    };
    normal.parse::<f64>().ok().filter(|n| n.is_finite()).map(|n| n * mult)
}

/// Células de uma linha de tabela: bordas externas opcionais e `\|` como barra dentro da célula.
/// `None` quando a linha não tem nenhuma barra de coluna.
fn cells(line: &str) -> Option<Vec<String>> {
    let line = line.trim();
    let mut out = vec![String::new()];
    let mut pipes = 0;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek() == Some(&'|') => { out.last_mut()?.push('|'); chars.next(); }
            '|' => { pipes += 1; out.push(String::new()); }
            _ => out.last_mut()?.push(c),
        }
    }
    if pipes == 0 { return None; }
    if line.starts_with('|') { out.remove(0); }
    if line.ends_with('|') && !line.ends_with("\\|") { out.pop(); }
    Some(out.into_iter().map(|c| c.trim().to_owned()).collect())
}

fn is_separator(cells: &[String]) -> bool {
    cells.iter().all(|c| {
        let dashes = c.trim_start_matches(':').trim_end_matches(':');
        !dashes.is_empty() && dashes.chars().all(|ch| ch == '-')
    })
}

/// Tabelas do texto com pelo menos 2 linhas e uma coluna toda numérica. A primeira coluna que não é
/// número dá o nome das linhas; sem ela, as linhas são numeradas.
pub fn read(markdown: &str, decimal: char) -> Vec<Table> {
    let lines: Vec<&str> = markdown.lines().collect();
    let mut tables = Vec::new();
    let mut fence = false;
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i].trim();
        if line.starts_with("```") || line.starts_with("~~~") { fence = !fence; }
        let header = (!fence).then(|| cells(line)).flatten();
        // Como no GFM: a linha de baixo é o separador, com o mesmo número de colunas do cabeçalho.
        let separated = header.as_ref().is_some_and(|h| lines.get(i + 1).and_then(|l| cells(l))
            .is_some_and(|s| s.len() == h.len() && is_separator(&s)));
        let (Some(header), true) = (header, separated) else { i += 1; continue };
        let mut rows = Vec::new();
        let mut end = i + 2;
        while let Some(row) = lines.get(end).filter(|l| !l.trim().is_empty()).and_then(|l| cells(l)) { rows.push(row); end += 1; }
        let numeric = |col: usize| rows.iter().map(|r| r.get(col).and_then(|c| parse_number(c, decimal))).collect::<Option<Vec<f64>>>();
        let columns: Vec<Column> = (0..header.len()).filter_map(|col| numeric(col).map(|values| Column {
            title: header[col].clone(), values,
            percent: rows.iter().all(|r| r.get(col).is_some_and(|c| c.trim_end().ends_with('%'))),
        })).collect();
        if rows.len() >= 2 && !columns.is_empty() {
            let label_col = (0..header.len()).find(|&col| numeric(col).is_none());
            let labels = rows.iter().enumerate()
                .map(|(n, r)| label_col.and_then(|c| r.get(c)).cloned().unwrap_or_else(|| (n + 1).to_string())).collect();
            tables.push(Table { start: i, end, labels, columns });
        }
        i = end;
    }
    tables
}

/// "46,9M", "16k", "35%": o valor do gráfico no separador do idioma, com sufixo ou a unidade da coluna.
pub fn short(value: f64, decimal: char, percent: bool) -> String {
    let (n, suffix) = match value.abs() {
        _ if percent => (value, "%"),
        v if v >= 1e9 => (value / 1e9, "B"),
        v if v >= 1e6 => (value / 1e6, "M"),
        v if v >= 1e3 => (value / 1e3, "k"),
        _ => (value, ""),
    };
    let text = format!("{n:.1}");
    let text = text.strip_suffix(".0").unwrap_or(&text).replace('.', &decimal.to_string());
    format!("{text}{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_follow_the_web_rules_and_the_screen_language() {
        assert_eq!(parse_number("46,9M", ','), Some(46_900_000.));
        assert_eq!(parse_number("1.234,56", ','), Some(1234.56));
        assert_eq!(parse_number("1,234.56", '.'), Some(1234.56));
        assert_eq!(parse_number("1,234", '.'), Some(1234.));
        assert_eq!(parse_number("1.234", ','), Some(1234.));
        assert_eq!(parse_number("1.234", '.'), Some(1.234));
        assert_eq!(parse_number("1.5", ','), Some(1.5));
        assert_eq!(parse_number("-3,5%", ','), Some(-3.5));
        assert_eq!(parse_number("16 k", ','), Some(16_000.));
        assert_eq!(parse_number("1.234.567", '.'), Some(1_234_567.));
        assert_eq!(parse_number("1.6.32", ','), None);
        assert_eq!(parse_number("Kimi", ','), None);
        assert_eq!(parse_number("", ','), None);
    }

    #[test]
    fn table_needs_two_rows_and_a_numeric_column() {
        let md = "Custo:\n\n| Conta | Chamadas | Bruto |\n|---|--:|--:|\n| Kimi | 327 | 46,9M |\n| Claude | 120 | 5,3M |\n\nfim\n\n| a | b |\n|---|---|\n| x | 1 |\n";
        let tables = read(md, ',');
        assert_eq!(tables.len(), 1);
        assert_eq!((tables[0].start, tables[0].end), (2, 6));
        assert_eq!(tables[0].labels, ["Kimi", "Claude"]);
        assert_eq!(tables[0].columns.iter().map(|c| c.title.as_str()).collect::<Vec<_>>(), ["Chamadas", "Bruto"]);
        assert!(read("```\n| a | 1 |\n|---|---|\n| x | 1 |\n| y | 2 |\n```", ',').is_empty());
        assert!(read("| Nome | Cor |\n|---|---|\n| a | azul |\n| b | verde |", ',').is_empty());
        assert_eq!(short(46_900_000., ',', false), "46,9M");
        assert_eq!(short(327., ',', false), "327");
    }

    #[test]
    fn table_without_outer_pipes_keeps_escaped_pipe_and_percent() {
        let md = "Conta | Uso | Custo\n--- | ---: | ---:\nCodex \\| nuvem | 35% | 1.234\nKimi | 65% | 980";
        let tables = read(md, ',');
        assert_eq!(tables.len(), 1);
        assert_eq!(tables[0].labels, ["Codex | nuvem", "Kimi"]);
        let uso = &tables[0].columns[0];
        assert_eq!((uso.title.as_str(), uso.percent, uso.values.clone()), ("Uso", true, vec![35., 65.]));
        assert_eq!(tables[0].columns[1].values, [1234., 980.]);
        assert_eq!(short(35., ',', true), "35%");
        // Linha de texto com barra, sem separador embaixo, não é tabela.
        assert!(read("a | b\nsó texto\nc | d", ',').is_empty());
    }
}
