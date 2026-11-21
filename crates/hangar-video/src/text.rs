use crate::error::VideoError;

/// O cabeçalho do dono para a rota do vídeo: o token vai no pedido, nunca na URL (ela aparece no log do mpv).
pub fn bearer(token: &str) -> (String, String) { ("Authorization".to_owned(), format!("Bearer {}", token.trim())) }

/// Um `Nome: valor` por cabeçalho, para o `http-header-fields-append` do mpv, que não divide o valor na vírgula.
/// Quebra de linha ou NUL viraria outro cabeçalho no pedido, e dois-pontos no nome mudaria qual é o nome.
pub(crate) fn header_fields(headers: &[(String, String)]) -> Result<Vec<String>, VideoError> {
    let unsafe_text = |text: &str| text.contains(['\r', '\n', '\0']);
    headers.iter().map(|(name, value)| {
        let name = name.trim();
        if name.is_empty() || name.contains(':') || unsafe_text(name) || unsafe_text(value) { return Err(VideoError::InvalidHeader); }
        Ok(format!("{name}: {}", value.trim()))
    }).collect()
}

/// Relógio do player: `0:07`, `12:34`, `1:02:03`. Sem valor (ainda carregando, transmissão sem fim) é `--:--`.
pub fn clock(seconds: Option<f64>) -> String {
    let Some(seconds) = seconds.filter(|s| s.is_finite() && *s >= 0.) else { return "--:--".to_owned() };
    let total = seconds.floor() as u64;
    let (hours, minutes, secs) = (total / 3600, total / 60 % 60, total % 60);
    if hours > 0 { format!("{hours}:{minutes:02}:{secs:02}") } else { format!("{minutes}:{secs:02}") }
}

#[cfg(test)]
mod tests {
    use super::{bearer, clock, header_fields};
    use crate::error::VideoError;

    #[test]
    fn bearer_header_trims_the_token() {
        assert_eq!(bearer(" abc \n"), ("Authorization".to_owned(), "Bearer abc".to_owned()));
    }

    #[test]
    fn header_fields_keep_commas_and_refuse_line_breaks() {
        let ok = header_fields(&[bearer("t0k"), ("X-Tags".into(), "a, b".into())]).unwrap();
        assert_eq!(ok, ["Authorization: Bearer t0k", "X-Tags: a, b"]);
        for bad in [("X".into(), "a\r\nHost: evil".into()), ("X\n".into(), "a".into()), ("".into(), "a".into()), ("A:B".into(), "c".into()),
            ("X".into(), "a\0".into())] {
            assert_eq!(header_fields(&[bad]), Err(VideoError::InvalidHeader));
        }
    }

    #[test]
    fn clock_formats_minutes_and_hours() {
        assert_eq!(clock(Some(0.)), "0:00");
        assert_eq!(clock(Some(7.9)), "0:07");
        assert_eq!(clock(Some(754.)), "12:34");
        assert_eq!(clock(Some(3723.)), "1:02:03");
        assert_eq!(clock(None), "--:--");
        assert_eq!(clock(Some(f64::NAN)), "--:--");
        assert_eq!(clock(Some(-1.)), "--:--");
    }
}
