//! Só para provas: com HANGAR_NATIVE_UI_MAP=<arquivo>, grava a cada quadro que mudar a lista de
//! elementos com id (caminho, bounds lógicos da janela, visível). HANGAR_NATIVE_FRAME_TIMES=1
//! registra o instante de cada quadro no stderr. Sem as variáveis nada é instalado.
//! O mapa não leva texto da tela: só ids, que já são rótulos da interface.

use gpui_kit::{ElementId, ElementRecord, Window};
use serde_json::{json, Value};
use std::io::Write;
use std::path::PathBuf;

pub fn install(window: &mut Window) {
    let paths = std::env::var_os("HANGAR_NATIVE_UI_MAP").map(PathBuf::from).map(|path| {
        let tmp = { let mut name = path.clone().into_os_string(); name.push(".tmp"); PathBuf::from(name) };
        (path, tmp)
    });
    let frames = std::env::var_os("HANGAR_NATIVE_FRAME_TIMES").is_some();
    if paths.is_none() && !frames { return; }
    window.set_element_map_sink(map_sink(paths, frames, std::io::stderr()));
}

fn map_sink(paths: Option<(PathBuf, PathBuf)>, mut frames: bool, mut trace: impl Write + 'static) -> impl FnMut(&[ElementRecord]) {
    let epoch = std::time::Instant::now();
    let (mut last, mut seq) = (Value::Null, 0u64);
    let mut map_failed = false;
    move |records| {
        if frames {
            if let Err(error) = write_trace(&mut trace, format_args!("frame {}", epoch.elapsed().as_micros())) {
                frames = false;
                crate::log_line(&format!("registro de quadros desativado após falha no stderr: {error}"));
            }
        }
        let Some((path, tmp)) = &paths else { return };
        let elements = snapshot(records);
        if elements == last { return; }
        seq += 1;
        let text = json!({ "seq": seq, "elements": &elements }).to_string();
        // Quem lê nunca vê o arquivo pela metade: grava ao lado e troca pelo rename.
        // ponytail: grava no próprio quadro, bloqueando; só vale para prova, nunca ligado no uso.
        match std::fs::write(&tmp, text).and_then(|_| std::fs::rename(&tmp, &path)) {
            Ok(()) => { last = elements; map_failed = false; }
            Err(error) if !map_failed => {
                map_failed = true;
                crate::log_line(&format!("falha ao gravar mapa de elementos em {}: {error}", path.display()));
            }
            Err(_) => {}
        }
    }
}

fn write_trace(writer: &mut impl Write, line: std::fmt::Arguments<'_>) -> std::io::Result<()> {
    writeln!(writer, "{line}")
}

fn snapshot(records: &[ElementRecord]) -> Value {
    // Ids de view mudam a cada execução e não identificam controle: ficam fora do caminho.
    let named = |id: &ElementId| (!matches!(id, ElementId::View(_))).then(|| id.to_string());
    Value::Array(records.iter().filter_map(|record| {
        let id = named(record.path.last()?)?;
        let b = record.bounds;
        Some(json!({
            "id": id,
            "path": record.path.iter().filter_map(named).collect::<Vec<_>>(),
            "x": f32::from(b.origin.x), "y": f32::from(b.origin.y),
            "w": f32::from(b.size.width), "h": f32::from(b.size.height),
            "visible": record.visible,
        }))
    }).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    // O glob pode trazer o `test` da gpui, que colide com o atributo padrão; o nome explícito vence o glob.
    use core::prelude::v1::test;
    use gpui_kit::{bounds, point, px, size, EntityId};

    #[test]
    fn frame_trace_returns_failure_without_panicking_when_writer_is_full() {
        use std::{cell::Cell, rc::Rc};
        struct Full(Rc<Cell<usize>>);
        impl Write for Full {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                self.0.set(self.0.get() + 1);
                Err(std::io::Error::from_raw_os_error(28))
            }
            fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
        }
        let writes = Rc::new(Cell::new(0));
        assert_eq!(write_trace(&mut Full(writes.clone()), format_args!("frame {}", 100)).unwrap_err().raw_os_error(), Some(28));
        let mut output = Vec::new();
        write_trace(&mut output, format_args!("frame {}", 100)).unwrap();
        assert_eq!(output, b"frame 100\n");

        let mut sink = map_sink(None, true, Full(writes.clone()));
        sink(&[]);
        assert_eq!(writes.get(), 2);
        sink(&[]);
        sink(&[]);
        assert_eq!(writes.get(), 2);
    }

    #[test]
    fn map_sink_retries_after_write_failure_and_recovers() {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("hangar-ui-map-{}-{stamp}", std::process::id()));
        let path = dir.join("map.json");
        let mut sink = map_sink(Some((path.clone(), dir.join("map.json.tmp"))), false, std::io::sink());
        sink(&[]);
        sink(&[]);
        assert!(!path.exists());
        std::fs::create_dir(&dir).unwrap();
        sink(&[]);
        let map: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(map, json!({ "seq": 3, "elements": [] }));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn snapshot_keeps_ids_bounds_and_visibility_without_view_ids() {
        let view = ElementId::View(EntityId::from(7u64));
        let record = |path: Vec<ElementId>, visible| ElementRecord {
            path: path.into(), bounds: bounds(point(px(10.), px(20.)), size(px(30.), px(40.))), visible,
        };
        let map = snapshot(&[
            record(vec![view.clone()], true),
            record(vec![view.clone(), "settings".into(), "font-1".into()], false),
        ]);
        assert_eq!(map, json!([{ "id": "font-1", "path": ["settings", "font-1"],
            "x": 10.0, "y": 20.0, "w": 30.0, "h": 40.0, "visible": false }]));
    }
}
