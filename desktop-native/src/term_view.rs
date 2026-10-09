//! Grade de terminal alimentada por bytes; o painel da sessão é dono do redesenho.
use std::{cell::RefCell, rc::Rc};
use alacritty_terminal::{
    event::{Event, EventListener},
    grid::{Dimensions, Scroll},
    index::{Column, Line, Point as TermPoint, Side},
    selection::{Selection, SelectionRange, SelectionType},
    term::{Config, Term, TermMode, cell::{Cell, Flags}, color::COUNT},
    vte::ansi::{Color, CursorShape, NamedColor, Processor, Rgb},
};
use gpui_kit::{App, Bounds, ContentMask, Element, ElementId, GlobalElementId, Hsla, InspectorElementId,
    IntoElement, KeyDownEvent, LayoutId, Pixels, ScrollWheelEvent, ShapedLine, StrikethroughStyle, Style, TextAlign, TextRun, UnderlineStyle,
    Window, fill, font, point, px, relative, rgb, size};

pub(crate) fn terminal_font(window: &Window) -> gpui_kit::Font {
    match crate::appearance::get().terminal_font {
        crate::appearance::CodeFont::JetBrainsMono => return font(crate::theme::CODE_MONO),
        crate::appearance::CodeFont::Named(name) => return font(name.0),
        crate::appearance::CodeFont::System => {}
    }
    // A fonte do kit já pode ter sido trocada pela preferência de código da conversa.
    static SYSTEM_MONO: std::sync::OnceLock<gpui_kit::SharedString> = std::sync::OnceLock::new();
    font(SYSTEM_MONO.get_or_init(|| {
        let default = gpui_kit::base::TypographyTokens::default().mono;
        let installed = window.text_system().all_font_names();
        [default.as_ref(), "Menlo", "Consolas", "DejaVu Sans Mono", "Noto Sans Mono", "Liberation Mono", crate::theme::CODE_MONO]
            .into_iter().find(|name| installed.iter().any(|installed| installed == name))
            .unwrap_or(crate::theme::CODE_MONO).to_owned().into()
    }).clone())
}

fn terminal_size() -> f32 { crate::appearance::get().terminal_size as f32 }
fn terminal_line_height(font_size: f32) -> f32 { font_size * 19. / 13. }
// Cores ANSI são dados do protocolo; texto e fundo padrão vêm do tema ativo.
const ANSI16: [u32; 16] = [
    0x2e3436, 0xcc0000, 0x4e9a06, 0xc4a000, 0x3465a4, 0x75507b, 0x06989a, 0xd3d7cf,
    0x555753, 0xef2929, 0x8ae234, 0xfce94f, 0x729fcf, 0xad7fa8, 0x34e2e2, 0xeeeeec,
];

struct GridSize { cols: usize, rows: usize }

impl Dimensions for GridSize {
    fn total_lines(&self) -> usize { self.rows }
    fn screen_lines(&self) -> usize { self.rows }
    fn columns(&self) -> usize { self.cols }
}

#[derive(Clone, Default)]
struct Output(Rc<RefCell<Vec<Vec<u8>>>>);

impl Output {
    fn push(&self, bytes: &[u8]) { self.0.borrow_mut().push(bytes.to_vec()); }
    fn drain(&self) -> Vec<Vec<u8>> { self.0.borrow_mut().drain(..).collect() }
}

impl EventListener for Output {
    fn send_event(&self, event: Event) {
        if let Event::PtyWrite(text) = event { self.push(text.as_bytes()); }
    }
}

#[derive(PartialEq, Eq)]
struct Snapshot {
    cells: Vec<Cell>,
    colors: [Option<Rgb>; COUNT],
    cursor: alacritty_terminal::term::RenderableCursor,
    selection: Option<SelectionRange>,
    offset: usize,
    cols: usize,
    rows: usize,
}

pub struct TermView {
    term: Term<Output>,
    parser: Processor,
    output: Output,
    cell_width: f32,
    line_height: f32,
    typography: (crate::appearance::CodeFont, u16),
    selection_anchor: Option<TermPoint>,
    wheel_remainder: f32,
    fixture_loopback: bool,
}

pub struct KeyResult { pub handled: bool, pub redraw: bool }

impl TermView {
    pub fn new(cols: usize, rows: usize) -> Self {
        let appearance = crate::appearance::get();
        let size = GridSize { cols: cols.max(2), rows: rows.max(1) };
        let output = Output::default();
        Self { term: Term::new(Config::default(), &size, output.clone()), parser: Processor::new(),
            output, cell_width: terminal_size() * 0.6, line_height: terminal_line_height(terminal_size()),
            typography: (appearance.terminal_font, appearance.terminal_size),
            selection_anchor: None, wheel_remainder: 0., fixture_loopback: false }
    }

    /// A fixture é opt-in; nenhum dado da sessão real entra nesta vista de prova.
    pub fn from_fixture_env(cols: usize, rows: usize) -> std::io::Result<Option<Self>> {
        let Some(path) = std::env::var_os("HANGAR_NATIVE_TERM_FIXTURE") else { return Ok(None) };
        let bytes = std::fs::read(path)?;
        let mut view = Self::new(cols, rows);
        view.fixture_loopback = true;
        view.feed(&bytes);
        Ok(Some(view))
    }

    /// O dono da vista chama notify apenas quando esta resposta for true.
    pub fn feed(&mut self, bytes: &[u8]) -> bool {
        if bytes.is_empty() { return false; }
        let before = self.snapshot();
        self.parser.advance(&mut self.term, bytes);
        if self.fixture_loopback {
            for reply in self.output.drain() {
                self.parser.advance(&mut self.term, &reply);
                let escaped: String = reply.iter().flat_map(|byte| std::ascii::escape_default(*byte)).map(char::from).collect();
                self.parser.advance(&mut self.term, format!("\r\n[terminal] {escaped}\r\n").as_bytes());
            }
        }
        let after = self.snapshot();
        let cursor_changed = if before.cursor.shape == CursorShape::Hidden && after.cursor.shape == CursorShape::Hidden {
            false
        } else { before.cursor != after.cursor };
        before.cols != after.cols || before.rows != after.rows || before.offset != after.offset
            || before.selection != after.selection
            || before.colors != after.colors || cursor_changed
            || !before.cells.iter().zip(&after.cells).all(|(a, b)| {
                const PAINTED: Flags = Flags::from_bits_retain(Flags::INVERSE.bits() | Flags::BOLD.bits() | Flags::DIM.bits()
                    | Flags::ITALIC.bits() | Flags::ALL_UNDERLINES.bits() | Flags::HIDDEN.bits() | Flags::STRIKEOUT.bits()
                    | Flags::WIDE_CHAR.bits() | Flags::WIDE_CHAR_SPACER.bits());
                a.c == b.c && a.fg == b.fg && a.bg == b.bg && a.flags & PAINTED == b.flags & PAINTED
                    && a.zerowidth() == b.zerowidth() && a.underline_color() == b.underline_color()
            })
    }

    fn snapshot(&self) -> Snapshot {
        let visible = self.term.renderable_content();
        Snapshot {
            cells: visible.display_iter.map(|indexed| indexed.cell.clone()).collect(),
            colors: std::array::from_fn(|index| visible.colors[index]),
            cursor: visible.cursor,
            selection: visible.selection,
            offset: visible.display_offset,
            cols: self.term.grid().columns(),
            rows: self.term.grid().screen_lines(),
        }
    }

    pub fn element(&self) -> impl IntoElement {
        TerminalGrid { snapshot: self.snapshot(), foreground: crate::theme::text(), background: crate::theme::background() }
    }

    /// A T66 envia cada bloco drenado pelo mesmo WebSocket do terminal.
    pub fn take_output(&self) -> Vec<Vec<u8>> { self.output.drain() }

    fn send_input(&mut self, bytes: &[u8]) -> bool {
        if self.fixture_loopback {
            let mut echo = Vec::with_capacity(bytes.len());
            for byte in bytes {
                match *byte {
                    b'\r' => echo.extend_from_slice(b"\r\n"),
                    0x7f => echo.extend_from_slice(b"\x08 \x08"),
                    _ => echo.push(*byte),
                }
            }
            self.feed(&echo)
        } else { self.output.push(bytes); false }
    }

    pub fn typography_changed(&self) -> bool {
        let appearance = crate::appearance::get();
        self.typography != (appearance.terminal_font, appearance.terminal_size)
    }

    pub fn resize_to_bounds(&mut self, bounds: Bounds<Pixels>, window: &mut Window) -> bool {
        let appearance = crate::appearance::get();
        self.typography = (appearance.terminal_font, appearance.terminal_size);
        let font_size = terminal_size();
        let cell_width = window.text_system().shape_line("M".into(), px(font_size),
            &[TextRun { len: 1, font: terminal_font(window), color: crate::theme::text(), ..Default::default() }], None).width;
        self.cell_width = f32::from(cell_width).max(1.);
        self.line_height = terminal_line_height(font_size);
        let cols = (f32::from(bounds.size.width) / self.cell_width).floor().max(2.) as usize;
        let rows = (f32::from(bounds.size.height) / self.line_height).floor().max(1.) as usize;
        self.resize_grid(cols, rows)
    }

    fn resize_grid(&mut self, cols: usize, rows: usize) -> bool {
        if self.term.grid().columns() == cols && self.term.grid().screen_lines() == rows { return false; }
        self.term.resize(GridSize { cols, rows });
        true
    }

    pub fn dimensions(&self) -> (u16, u16) {
        (self.term.grid().columns() as u16, self.term.grid().screen_lines() as u16)
    }

    /// A T66 impede a propagação se handled e redesenha apenas se redraw.
    pub fn key_down(&mut self, event: &KeyDownEvent) -> KeyResult {
        let key = event.keystroke.key.as_str();
        let modifiers = event.keystroke.modifiers;
        if modifiers.shift && matches!(key, "pageup" | "pagedown") {
            return KeyResult { handled: true, redraw: self.scroll(if key == "pageup" { Scroll::PageUp } else { Scroll::PageDown }) };
        }
        let Some(bytes) = key_bytes(event, self.term.mode().contains(TermMode::APP_CURSOR)) else {
            return KeyResult { handled: false, redraw: false };
        };
        let redraw = self.term.grid().display_offset() != 0;
        self.term.scroll_display(Scroll::Bottom);
        let echoed = self.send_input(&bytes);
        KeyResult { handled: true, redraw: redraw || echoed }
    }

    pub fn paste(&mut self, text: &str) -> bool {
        let redraw = self.term.grid().display_offset() != 0;
        self.term.scroll_display(Scroll::Bottom);
        if self.term.mode().contains(TermMode::BRACKETED_PASTE) {
            let mut bytes = b"\x1b[200~".to_vec();
            bytes.extend_from_slice(text.replace('\x1b', "").as_bytes());
            bytes.extend_from_slice(b"\x1b[201~");
            let echoed = self.send_input(&bytes);
            redraw || echoed
        } else {
            let echoed = self.send_input(text.as_bytes());
            redraw || echoed
        }
    }

    pub fn scroll_lines(&mut self, lines: i32) -> bool { self.scroll(Scroll::Delta(lines)) }

    pub fn scroll_wheel(&mut self, event: &ScrollWheelEvent) -> bool {
        self.wheel_remainder += f32::from(event.delta.pixel_delta(px(self.line_height)).y) / self.line_height;
        let lines = self.wheel_remainder.trunc() as i32;
        self.wheel_remainder -= lines as f32;
        self.scroll_lines(lines)
    }

    fn scroll(&mut self, amount: Scroll) -> bool {
        let before = self.term.grid().display_offset();
        self.term.scroll_display(amount);
        before != self.term.grid().display_offset()
    }

    fn point_at(&self, x: f32, y: f32) -> TermPoint {
        let col = (x / self.cell_width).floor().max(0.) as usize;
        let row = (y / self.line_height).floor().max(0.) as i32;
        TermPoint::new(Line(row.min(self.term.grid().screen_lines() as i32 - 1)
            - self.term.grid().display_offset() as i32), Column(col.min(self.term.grid().columns() - 1)))
    }

    /// Coordenadas relativas à grade; click_count >= 2 seleciona a palavra.
    pub fn mouse_down(&mut self, x: f32, y: f32, click_count: usize) -> bool {
        let point = self.point_at(x, y);
        let before = self.term.selection.as_ref().and_then(|selection| selection.to_range(&self.term));
        self.term.selection = Some(Selection::new(if click_count >= 2 { SelectionType::Semantic }
            else { SelectionType::Simple }, point, Side::Left));
        self.selection_anchor = Some(point);
        before != self.term.selection.as_ref().and_then(|selection| selection.to_range(&self.term))
    }

    pub fn mouse_drag(&mut self, x: f32, y: f32) -> bool {
        let Some(anchor) = self.selection_anchor else { return false; };
        let point = self.point_at(x, y);
        let before = self.term.selection.as_ref().and_then(|selection| selection.to_range(&self.term));
        let kind = self.term.selection.as_ref().map_or(SelectionType::Simple, |selection| selection.ty);
        let reverse = point < anchor;
        let mut selection = Selection::new(kind, anchor, if reverse { Side::Right } else { Side::Left });
        selection.update(point, if reverse { Side::Left } else { Side::Right });
        self.term.selection = Some(selection);
        before != self.term.selection.as_ref().and_then(|selection| selection.to_range(&self.term))
    }

    pub fn mouse_up(&mut self) { self.selection_anchor = None; }

    /// O chamador decide quando copiar; a fixture nunca toca a área de transferência.
    pub fn selected_text(&self) -> Option<String> { self.term.selection_to_string() }
}

fn key_bytes(event: &KeyDownEvent, app_cursor: bool) -> Option<Vec<u8>> {
    let stroke = &event.keystroke;
    let mods = stroke.modifiers;
    if mods.platform { return None; }
    let key = stroke.key.as_str();
    let modifier = 1 + u8::from(mods.shift) + 2 * u8::from(mods.alt) + 4 * u8::from(mods.control);
    let csi = |tail: &str| if modifier == 1 { format!("\x1b[{tail}") }
        else { format!("\x1b[1;{modifier}{tail}") };
    let sequence = match key {
        "enter" => "\r".to_string(), "backspace" => "\x7f".to_string(), "tab" if mods.shift => "\x1b[Z".to_string(),
        "tab" => "\t".to_string(), "escape" => "\x1b".to_string(),
        "space" if mods.control => "\0".to_string(), "space" => " ".to_string(),
        "up" | "down" | "right" | "left" => {
            let suffix = match key { "up" => "A", "down" => "B", "right" => "C", _ => "D" };
            if modifier == 1 && app_cursor { format!("\x1bO{suffix}") } else { csi(suffix) }
        },
        "home" | "end" => {
            let suffix = if key == "home" { "H" } else { "F" };
            if modifier == 1 && app_cursor { format!("\x1bO{suffix}") } else { csi(suffix) }
        },
        "insert" | "delete" | "pageup" | "pagedown" | "f5" | "f6" | "f7" | "f8" | "f9" | "f10" | "f11" | "f12" => {
            let number = match key { "insert" => 2, "delete" => 3, "pageup" => 5, "pagedown" => 6,
                "f5" => 15, "f6" => 17, "f7" => 18, "f8" => 19, "f9" => 20, "f10" => 21, "f11" => 23, _ => 24 };
            if modifier == 1 { format!("\x1b[{number}~") } else { format!("\x1b[{number};{modifier}~") }
        },
        "f1" | "f2" | "f3" | "f4" => {
            let suffix = match key { "f1" => "P", "f2" => "Q", "f3" => "R", _ => "S" };
            if modifier == 1 { format!("\x1bO{suffix}") } else { csi(suffix) }
        },
        _ if mods.control && !event.prefer_character_input => {
            let letter = key.bytes().next().filter(|_| key.len() == 1)?.to_ascii_uppercase();
            let code = match letter { b'A'..=b'Z' => letter - b'A' + 1, b' ' | b'@' => 0,
                b'[' => 27, b'\\' => 28, b']' => 29, b'^' => 30, b'_' => 31, b'?' => 127, _ => return None };
            String::from_utf8(vec![code]).ok()?
        },
        _ if mods.alt && !event.prefer_character_input && key.chars().count() == 1 =>
            if mods.shift { key.to_uppercase() } else { key.to_string() },
        _ => stroke.key_char.clone().or_else(|| (key.chars().count() == 1).then(|| key.to_string()))?,
    };
    let mut bytes = Vec::with_capacity(sequence.len() + 1);
    if mods.alt && !event.prefer_character_input
        && (key.chars().count() == 1 || matches!(key, "enter" | "backspace" | "tab" | "escape" | "space")) {
        bytes.push(0x1b);
    }
    bytes.extend_from_slice(sequence.as_bytes());
    Some(bytes)
}

fn indexed_rgb(index: u8) -> u32 {
    match index {
        0..=15 => ANSI16[index as usize],
        16..=231 => {
            let n = index as u32 - 16;
            let step = |v| if v == 0 { 0 } else { 55 + v * 40 };
            step(n / 36) << 16 | step(n / 6 % 6) << 8 | step(n % 6)
        },
        _ => {
            let gray = 8 + (index as u32 - 232) * 10;
            gray << 16 | gray << 8 | gray
        },
    }
}

fn dim_rgb(value: u32) -> u32 {
    let channel = |shift: u32| ((value >> shift & 0xff_u32) * 2_u32 / 3_u32) << shift;
    channel(16) | channel(8) | channel(0)
}

fn paint_color(color: Color, palette: &[Option<Rgb>; COUNT], fg: Hsla, bg: Hsla) -> Hsla {
    let packed = |value: Rgb| rgb((u32::from(value.r) << 16) | (u32::from(value.g) << 8) | u32::from(value.b)).into();
    match color {
        Color::Spec(value) => packed(value),
        Color::Indexed(index) => palette[index as usize].map_or_else(|| rgb(indexed_rgb(index)).into(), packed),
        Color::Named(NamedColor::Foreground) => palette[NamedColor::Foreground as usize].map_or(fg, packed),
        Color::Named(NamedColor::BrightForeground) => palette[NamedColor::BrightForeground as usize]
            .or(palette[NamedColor::Foreground as usize]).map_or(fg, packed),
        Color::Named(NamedColor::Background) => palette[NamedColor::Background as usize].map_or(bg, packed),
        Color::Named(NamedColor::Cursor) => palette[NamedColor::Cursor as usize].map_or(fg, packed),
        Color::Named(NamedColor::DimForeground) => palette[NamedColor::DimForeground as usize].map_or(crate::theme::muted(), packed),
        Color::Named(name) => {
            let index = name as usize;
            palette[index].map_or_else(|| {
                let value = if index < 16 { indexed_rgb(index as u8) } else { dim_rgb(indexed_rgb((index - 259) as u8)) };
                rgb(value).into()
            }, packed)
        },
    }
}

fn cell_colors(cell: &Cell, palette: &[Option<Rgb>; COUNT], fg: Hsla, bg: Hsla) -> (Hsla, Option<Hsla>) {
    let named = match cell.fg {
        Color::Named(name) if cell.flags.contains(Flags::BOLD) => Color::Named(name.to_bright()),
        Color::Named(name) if cell.flags.contains(Flags::DIM) => Color::Named(name.to_dim()),
        value => value,
    };
    let (ink, fill) = if cell.flags.contains(Flags::INVERSE) { (cell.bg, named) } else { (named, cell.bg) };
    let ink = paint_color(ink, palette, fg, bg);
    let fill = (fill != Color::Named(NamedColor::Background) || palette[NamedColor::Background as usize].is_some())
        .then(|| paint_color(fill, palette, fg, bg));
    (ink, fill)
}

struct TerminalGrid { snapshot: Snapshot, foreground: Hsla, background: Hsla }

impl IntoElement for TerminalGrid {
    type Element = Self;
    fn into_element(self) -> Self { self }
}

impl Element for TerminalGrid {
    type RequestLayoutState = ();
    type PrepaintState = (Pixels, Pixels, Vec<Vec<(usize, ShapedLine)>>);

    fn id(&self) -> Option<ElementId> { None }
    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> { None }

    fn request_layout(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, window: &mut Window, cx: &mut App)
        -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, bounds: Bounds<Pixels>, _: &mut (),
        window: &mut Window, _: &mut App) -> Self::PrepaintState {
        // A grade é pintada célula a célula, sem elemento de texto: o leitor recebe a tela visível como texto.
        if window.is_a11y_active() {
            let screen = self.snapshot.cells.chunks(self.snapshot.cols.max(1))
                .map(|cells| cells.iter().filter(|c| !c.flags.contains(Flags::WIDE_CHAR_SPACER))
                    .map(|c| if c.flags.contains(Flags::HIDDEN) { ' ' } else { c.c }).collect::<String>().trim_end().to_owned())
                .collect::<Vec<_>>().join("\n");
            window.a11y_text(screen.trim_end(), bounds);
        }
        let font = terminal_font(window);
        let font_size = terminal_size();
        let cell_width = window.text_system().shape_line("M".into(), px(font_size),
            &[TextRun { len: 1, font: font.clone(), color: self.foreground, ..Default::default() }], None).width;
        let cursor_row = self.snapshot.cursor.point.line.0 + self.snapshot.offset as i32;
        let cursor_col = self.snapshot.cursor.point.column.0;
        let lines = self.snapshot.cells.chunks(self.snapshot.cols).enumerate().map(|(row, cells)| {
            let mut text = String::new();
            let mut runs: Vec<TextRun> = Vec::new();
            let mut segments = Vec::new();
            let mut start_col = 0;
            for (col, cell) in cells.iter().enumerate() {
                if cell.flags.contains(Flags::WIDE_CHAR_SPACER) { continue; }
                if cell.flags.contains(Flags::WIDE_CHAR) && !text.is_empty() {
                    segments.push((start_col, window.text_system().shape_line(text.into(), px(font_size), &runs, None)));
                    text = String::new();
                    runs.clear();
                }
                if text.is_empty() { start_col = col; }
                let start = text.len();
                text.push(if cell.flags.contains(Flags::HIDDEN) { ' ' } else { cell.c });
                if let Some(marks) = cell.zerowidth() { text.extend(marks); }
                let (mut ink, _) = cell_colors(cell, &self.snapshot.colors, self.foreground, self.background);
                if self.snapshot.cursor.shape == CursorShape::Block && row as i32 == cursor_row && col == cursor_col {
                    ink = paint_color(Color::Named(NamedColor::Background), &self.snapshot.colors, self.foreground, self.background);
                }
                let mut glyph_font = font.clone();
                if cell.flags.contains(Flags::BOLD) { glyph_font = glyph_font.bold(); }
                if cell.flags.contains(Flags::ITALIC) { glyph_font = glyph_font.italic(); }
                let underline = cell.flags.intersects(Flags::ALL_UNDERLINES).then(|| UnderlineStyle {
                    thickness: px(1.),
                    color: cell.underline_color().map(|value| paint_color(value, &self.snapshot.colors, ink, self.background)),
                    wavy: cell.flags.contains(Flags::UNDERCURL),
                });
                let strikethrough = cell.flags.contains(Flags::STRIKEOUT).then(|| StrikethroughStyle { thickness: px(1.), color: None });
                let len = text.len() - start;
                if let Some(last) = runs.last_mut().filter(|run| run.color == ink && run.font == glyph_font
                    && run.underline == underline && run.strikethrough == strikethrough) { last.len += len; }
                else { runs.push(TextRun { len, font: glyph_font, color: ink, underline, strikethrough, ..Default::default() }); }
                if cell.flags.contains(Flags::WIDE_CHAR) {
                    segments.push((start_col, window.text_system().shape_line(text.into(), px(font_size), &runs, None)));
                    text = String::new();
                    runs.clear();
                }
            }
            if !text.is_empty() {
                segments.push((start_col, window.text_system().shape_line(text.into(), px(font_size), &runs, None)));
            }
            segments
        }).collect();
        (cell_width, px(terminal_line_height(font_size)), lines)
    }

    fn paint(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, bounds: Bounds<Pixels>, _: &mut (),
        state: &mut Self::PrepaintState, window: &mut Window, cx: &mut App) {
        let (cell_width, line_height, lines) = state;
        let line_height = *line_height;
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            for (row, cells) in self.snapshot.cells.chunks(self.snapshot.cols).enumerate() {
                for (col, cell) in cells.iter().enumerate() {
                    let (_, mut fill_color) = cell_colors(cell, &self.snapshot.colors, self.foreground, self.background);
                    let cell_point = TermPoint::new(Line(row as i32 - self.snapshot.offset as i32), Column(col));
                    if self.snapshot.selection.as_ref().is_some_and(|selection| selection.contains(cell_point)) {
                        fill_color = Some(crate::theme::accent_dim());
                    }
                    if let Some(color) = fill_color {
                        let origin = point(bounds.origin.x + *cell_width * col as f32, bounds.origin.y + line_height * row as f32);
                        window.paint_quad(fill(Bounds { origin, size: size(*cell_width, line_height) }, color));
                    }
                }
            }
            let cursor = self.snapshot.cursor;
            if let Ok(row) = usize::try_from(cursor.point.line.0 + self.snapshot.offset as i32) {
                if row < self.snapshot.rows && cursor.point.column.0 < self.snapshot.cols {
                    let col = cursor.point.column.0;
                    let wide = self.snapshot.cells[row * self.snapshot.cols + col].flags.contains(Flags::WIDE_CHAR);
                    let cursor_width = *cell_width * if wide { 2. } else { 1. };
                    let origin = point(bounds.origin.x + *cell_width * col as f32,
                        bounds.origin.y + line_height * row as f32);
                    let color = self.snapshot.colors[NamedColor::Cursor as usize]
                        .map_or(self.foreground, |value| paint_color(Color::Spec(value), &self.snapshot.colors, self.foreground, self.background));
                    let cursor_bounds = Bounds { origin, size: size(cursor_width, line_height) };
                    match cursor.shape {
                        CursorShape::Block => window.paint_quad(fill(cursor_bounds, color)),
                        CursorShape::Beam => window.paint_quad(fill(Bounds { size: size(px(2.), line_height), ..cursor_bounds }, color)),
                        CursorShape::Underline => window.paint_quad(fill(Bounds { origin: point(origin.x, origin.y + line_height - px(2.)), size: size(cursor_width, px(2.)) }, color)),
                        CursorShape::HollowBlock => {
                            window.paint_quad(fill(Bounds { size: size(cursor_width, px(1.)), ..cursor_bounds }, color));
                            window.paint_quad(fill(Bounds { origin: point(origin.x, origin.y + line_height - px(1.)), size: size(cursor_width, px(1.)) }, color));
                            window.paint_quad(fill(Bounds { size: size(px(1.), line_height), ..cursor_bounds }, color));
                            window.paint_quad(fill(Bounds { origin: point(origin.x + cursor_width - px(1.), origin.y), size: size(px(1.), line_height) }, color));
                        },
                        CursorShape::Hidden => {},
                    }
                }
            }
            for (row, segments) in lines.iter().enumerate() {
                for (col, line) in segments {
                    let origin = point(bounds.origin.x + *cell_width * *col as f32, bounds.origin.y + line_height * row as f32);
                    if let Err(error) = line.paint(origin, line_height, TextAlign::Left, None, window, cx) {
                        eprintln!("Falha ao desenhar texto do terminal: {error}");
                    }
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // O glob pode trazer o `test` da gpui, que colide com o atributo padrão; o nome explícito vence o glob.
    use core::prelude::v1::test;
    use gpui_kit::{Keystroke, Modifiers, ScrollDelta};

    fn key(name: &str, ch: Option<&str>, modifiers: Modifiers) -> KeyDownEvent {
        KeyDownEvent { keystroke: Keystroke { key: name.into(), key_char: ch.map(str::to_string), modifiers },
            is_held: false, prefer_character_input: false, physical_digit: None }
    }

    #[test]
    fn ansi_grid_keeps_colors_cursor_clear_wrap_and_wide_cells() {
        let mut view = TermView::new(8, 4);
        assert!(!view.feed(&[]));
        assert!(view.feed(b"\x1b[31mR\x1b[38;5;196mX\x1b[38;2;1;2;3mT"));
        let grid = view.snapshot();
        assert_eq!(grid.cells[0].fg, Color::Named(NamedColor::Red));
        assert_eq!(grid.cells[1].fg, Color::Indexed(196));
        assert_eq!(grid.cells[2].fg, Color::Spec(Rgb { r: 1, g: 2, b: 3 }));
        assert_eq!(indexed_rgb(196), 0xff0000);
        assert_ne!(dim_rgb(ANSI16[1]), ANSI16[1]);
        assert!(!view.feed(b"\x1b[4G")); // O cursor já está na quarta coluna após T.
        assert!(view.feed("界🙂abcdef".as_bytes()));
        let grid = view.snapshot();
        assert!(grid.cells.iter().any(|cell| cell.c == '界' && cell.flags.contains(Flags::WIDE_CHAR)));
        assert!(grid.cells.iter().any(|cell| cell.c == '🙂' && cell.flags.contains(Flags::WIDE_CHAR)));
        assert_eq!(grid.cells[8].c, 'b');
        assert!(view.feed(b"\x1b[1;5H"));
        let grid = view.snapshot();
        assert_eq!(grid.cursor.point.column.0, 3);
        assert!(grid.cells[3].flags.contains(Flags::WIDE_CHAR));
        assert!(view.feed(b"\x1b[2J"));
        assert!(view.snapshot().cells.iter().all(|cell| cell.c == ' '));
        assert!(view.feed(b"\x1b[?25l"));
        assert!(!view.feed(b"\x1b[2;2H"));
    }

    #[test]
    fn keys_and_terminal_replies_share_the_output_queue() {
        let mut view = TermView::new(8, 3);
        let cases = [
            ("a", Some("a"), Modifiers::default(), b"a".as_slice()),
            ("enter", None, Modifiers::default(), b"\r"),
            ("backspace", None, Modifiers::default(), b"\x7f"),
            ("tab", None, Modifiers::default(), b"\t"),
            ("up", None, Modifiers::default(), b"\x1b[A"),
            ("a", Some("a"), Modifiers { control: true, ..Default::default() }, b"\x01"),
            ("space", None, Modifiers { control: true, ..Default::default() }, b"\0"),
            ("x", Some("x"), Modifiers { alt: true, ..Default::default() }, b"\x1bx"),
            ("f", Some("ƒ"), Modifiers { alt: true, ..Default::default() }, b"\x1bf"),
            ("a", Some("A"), Modifiers { alt: true, shift: true, ..Default::default() }, b"\x1bA"),
            ("enter", None, Modifiers { alt: true, ..Default::default() }, b"\x1b\r"),
            ("f1", None, Modifiers::default(), b"\x1bOP"),
            ("f5", None, Modifiers::default(), b"\x1b[15~"),
            ("f12", None, Modifiers::default(), b"\x1b[24~"),
            ("left", None, Modifiers { control: true, ..Default::default() }, b"\x1b[1;5D"),
            ("tab", None, Modifiers { shift: true, ..Default::default() }, b"\x1b[Z"),
        ];
        for (name, ch, modifiers, expected) in cases {
            assert!(view.key_down(&key(name, ch, modifiers)).handled, "{name}");
            assert_eq!(view.take_output(), vec![expected.to_vec()], "{name}");
        }
        view.feed(b"\x1b[?1h\x1b[?2004h");
        assert_eq!(key_bytes(&key("up", None, Modifiers::default()), true), Some(b"\x1bOA".to_vec()));
        view.paste("oi\x1b[201~fim");
        assert_eq!(view.take_output(), vec![b"\x1b[200~oi[201~fim\x1b[201~".to_vec()]);
        view.feed(b"\x1b[6n");
        assert_eq!(view.take_output(), vec![b"\x1b[1;1R".to_vec()]);
    }

    #[test]
    fn terminal_selection_uses_resized_cell_metrics() {
        let mut view = TermView::new(20, 4);
        view.feed(b"first\r\nsecond\r\nthird");
        for font_size in [8., 12., 18., 24.] {
            view.cell_width = font_size * 0.6;
            view.line_height = terminal_line_height(font_size);
            assert_eq!(view.point_at(view.cell_width * 2.5, view.line_height * 1.5),
                TermPoint::new(Line(1), Column(2)));
        }
    }

    #[test]
    fn resize_selection_and_history_keep_visible_content() {
        let mut view = TermView::new(10, 2);
        view.feed(b"alpha beta\r\nsecond\r\nthird");
        assert!(view.scroll_lines(1));
        assert_eq!(view.snapshot().offset, 1);
        assert!(!view.scroll_lines(0));
        view.scroll_lines(-1);
        for _ in 0..3 {
            assert!(!view.scroll_wheel(&ScrollWheelEvent { delta: ScrollDelta::Lines(point(0., 0.25)), ..Default::default() }));
        }
        assert!(view.scroll_wheel(&ScrollWheelEvent { delta: ScrollDelta::Lines(point(0., 0.25)), ..Default::default() }));
        assert!(view.key_down(&key("z", Some("z"), Modifiers::default())).redraw);
        assert_eq!(view.snapshot().offset, 0);
        assert!(view.resize_grid(12, 3));
        assert!(view.snapshot().cells.iter().any(|cell| cell.c == 't'));
        assert!(!view.resize_grid(12, 3));
        assert!(view.mouse_down(2., 0., 2));
        assert_eq!(view.selected_text().as_deref(), Some("alpha"));
        view.mouse_up();
        assert!(view.mouse_down(0., 0., 1));
        assert!(view.mouse_drag(view.cell_width * 4.5, 0.));
        assert_eq!(view.selected_text().as_deref(), Some("alpha"));
        view.mouse_up();
        view.mouse_down(view.cell_width * 4.5, 0., 1);
        assert!(view.mouse_drag(0., 0.));
        assert_eq!(view.selected_text().as_deref(), Some("alpha"));
    }

    #[test]
    fn fixture_echoes_input_and_prints_terminal_replies() {
        let mut view = TermView::new(40, 4);
        view.fixture_loopback = true;
        assert!(view.key_down(&key("a", Some("a"), Modifiers::default())).redraw);
        assert_eq!(view.snapshot().cells[0].c, 'a');
        assert!(view.take_output().is_empty());
        view.feed(b"\x1b[6n\x1b[c");
        let visible: String = view.snapshot().cells.iter().map(|cell| cell.c).collect();
        assert!(visible.contains("[terminal] \\x1b[1;2R"));
        assert!(visible.contains("[terminal] \\x1b[?6c"));

        let mut input = TermView::new(40, 4);
        input.fixture_loopback = true;
        for (name, ch) in [("a", Some("a")), ("b", Some("b")), ("c", Some("c")),
            ("backspace", None), ("d", Some("d")), ("enter", None), ("o", Some("o")), ("k", Some("k"))] {
            assert!(input.key_down(&key(name, ch, Modifiers::default())).handled);
        }
        let cells = input.snapshot().cells;
        let lines: Vec<String> = cells.chunks(40).map(|row| row.iter().map(|cell| cell.c)
            .collect::<String>().trim_end().to_string()).collect();
        assert_eq!(&lines[..2], &["abd", "ok"]);
    }
}
