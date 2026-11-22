//! QR em SVG desenhado no servidor (pareamento de máquinas e do WhatsApp): os clientes só exibem a imagem.
use std::fmt::Write;

/// Um quadrado por módulo escuro, com um módulo de margem. `None` quando o texto não cabe num QR.
pub fn svg(text: &str) -> Option<String> {
    let qr = qrcode::QrCode::new(text.as_bytes()).ok()?;
    let n = qr.width();
    let mut svg = String::new();
    // Escrever numa `String` não falha.
    let _ = write!(svg, "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {side} {side}\"><path d=\"", side = n + 2);
    for y in 0..n {
        for x in 0..n {
            if qr[(x, y)] == qrcode::Color::Dark {
                let _ = write!(svg, "M{},{}h1v1h-1z", x + 1, y + 1);
            }
        }
    }
    svg.push_str("\"/></svg>");
    Some(svg)
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_text_becomes_an_svg_or_nothing() {
        let svg = super::svg("2@abc,def,ghi,jkl").unwrap();
        assert!(svg.starts_with("<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 "), "{svg}");
        assert!(svg.contains("<path d=\"M") && svg.ends_with("\"/></svg>"), "{svg}");
        assert_eq!(super::svg(&"x".repeat(8000)), None, "longo demais");
    }
}
