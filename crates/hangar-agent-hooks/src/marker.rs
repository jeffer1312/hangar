//! Gravação dos marcadores por arquivo temporário + troca: o leitor nunca pega metade, e o
//! observador de pastas do servidor vê a escrita pelo `rename`.
use std::fs;
use std::io;
use std::path::Path;

/// Grava `<dir>/<key>.json`. O temporário leva o pid (`<key>.json.<pid>.tmp`): dois escritores do mesmo
/// marcador (o hook e o ator do servidor) nunca dividem o mesmo arquivo, e a poda reconhece a sobra.
/// Sem `fsync`, como o hook do Python: o marcador é refeito a cada evento.
pub fn write(dir: &Path, key: &str, bytes: &[u8]) -> io::Result<()> {
    let target = dir.join(format!("{key}.json"));
    let tmp = dir.join(format!("{key}.json.{}.tmp", std::process::id()));
    let result = fs::write(&tmp, bytes).and_then(|()| hangar_core::fs::replace(&tmp, &target));
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}
