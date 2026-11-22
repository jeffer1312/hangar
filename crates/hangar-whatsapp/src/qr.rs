//! QR do pareamento em SVG, desenhado no servidor: os clientes só exibem a imagem.
//! Mesmo desenho do `machine_peers::network::qr_svg` do servidor, que este crate não pode usar.

/// `None` quando o texto não cabe num QR.
pub fn qr_svg(text: &str) -> Option<String> {
    let qr = qrcode::QrCode::new(text.as_bytes()).ok()?;
    let n = qr.width();
    let mut path = String::new();
    for y in 0..n {
        for x in 0..n {
            if qr[(x, y)] == qrcode::Color::Dark {
                path.push_str(&format!("M{},{}h1v1h-1z", x + 1, y + 1));
            }
        }
    }
    Some(format!("<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {} {}\"><path d=\"{path}\"/></svg>", n + 2, n + 2))
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_pairing_text_becomes_an_svg() {
        let svg = super::qr_svg("2@abc,def,ghi,jkl").unwrap();
        assert!(svg.starts_with("<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 "), "{svg}");
        assert!(svg.contains("<path d=\"M"), "{svg}");
        assert_eq!(super::qr_svg(&"x".repeat(8000)), None, "longo demais");
    }
}
