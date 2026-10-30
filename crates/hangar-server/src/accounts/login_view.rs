//! Transmissão da janela de login: mostra e opera o Chrome normal pela tela do sistema. O Chrome não
//! recebe ligação remota, porque ele a recusa no perfil que a pessoa usa no dia a dia.
use serde::Deserialize;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeskWindow {
    pub id: String,
    pub title: String,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseAction {
    Move,
    Down,
    Up,
}

/// Só as teclas de navegar e editar: atalho do sistema alcançaria o resto da máquina.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum DeskKey {
    Enter,
    Tab,
    Backspace,
    Delete,
    Escape,
    ArrowLeft,
    ArrowRight,
    ArrowUp,
    ArrowDown,
    Home,
    End,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum DeskInput {
    Mouse { action: MouseAction, x: f64, y: f64 },
    Wheel { x: f64, y: f64, down: bool },
    Key { key: DeskKey },
    Text { text: String },
    Back,
}

/// Bloqueia: quem chama faz fora da thread do runtime.
pub trait Desk: Send + Sync {
    fn windows(&self) -> Vec<DeskWindow>;
    fn present(&self, window: &DeskWindow);
    fn capture(&self, window: &DeskWindow) -> Option<Vec<u8>>;
    /// Falso quando o sistema não entrega entrada: sessão desconectada do Windows.
    fn can_input(&self) -> bool;
    fn input(&self, window: &DeskWindow, input: &DeskInput);
    fn close(&self, window: &DeskWindow);
    /// Reativa a sessão desconectada. Deixa o desktop aberto na tela física, por isso só a pedido.
    fn activate(&self) -> bool {
        false
    }
}

/// Coordenada da imagem para a tela, sem nunca sair da janela do login.
pub fn window_point(window: &DeskWindow, x: f64, y: f64) -> (i32, i32) {
    let inside = |value: f64, size: i32| {
        let value = if value.is_finite() { value } else { 0.0 };
        value.clamp(0.0, f64::from((size - 1).max(0))).round() as i32
    };
    (window.x + inside(x, window.w), window.y + inside(y, window.h))
}

pub fn detect() -> Option<Arc<dyn Desk>> {
    #[cfg(target_os = "linux")]
    return hyprland::detect();
    #[cfg(windows)]
    return Some(Arc::new(windows_desk::WindowsDesk));
    #[cfg(not(any(target_os = "linux", windows)))]
    None
}

#[cfg(target_os = "linux")]
mod hyprland {
    use super::*;
    use std::process::{Command, Output};

    pub struct HyprDesk {
        env: Vec<(String, String)>,
    }

    #[derive(Deserialize)]
    struct Client {
        address: String,
        class: String,
        title: String,
        at: [i32; 2],
        size: [i32; 2],
        #[serde(default = "mapped_default")]
        mapped: bool,
    }

    fn mapped_default() -> bool {
        true
    }

    pub fn chrome_windows(clients: &[u8]) -> Vec<DeskWindow> {
        let clients: Vec<Client> = serde_json::from_slice(clients).unwrap_or_default();
        clients
            .into_iter()
            .filter(|client| client.mapped && client.class.to_lowercase().contains("chrome"))
            .map(|client| DeskWindow {
                id: client.address,
                title: client.title,
                x: client.at[0],
                y: client.at[1],
                w: client.size[0],
                h: client.size[1],
            })
            .collect()
    }

    fn on_path(name: &str) -> bool {
        std::env::var_os("PATH").is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(name).is_file()))
    }

    pub fn detect() -> Option<Arc<dyn Desk>> {
        if !["hyprctl", "grim", "wtype", "ydotool"].into_iter().all(on_path) {
            return None;
        }
        let mut env = crate::display_env::for_launch(|key| std::env::var(key).ok());
        let value = |env: &[(String, String)], key: &str| {
            env.iter().find(|(name, _)| name == key).map(|(_, value)| value.clone()).or_else(|| std::env::var(key).ok())
        };
        value(&env, "HYPRLAND_INSTANCE_SIGNATURE")?;
        let socket = std::path::PathBuf::from(value(&env, "XDG_RUNTIME_DIR")?).join(".ydotool_socket");
        if !socket.exists() {
            return None;
        }
        env.push(("YDOTOOL_SOCKET".to_owned(), socket.to_string_lossy().into_owned()));
        Some(Arc::new(HyprDesk { env }))
    }

    impl HyprDesk {
        fn run(&self, program: &str, args: &[&str]) -> Option<Output> {
            let output = Command::new(program).args(args).envs(self.env.iter().map(|(key, value)| (key, value))).output().ok()?;
            output.status.success().then_some(output)
        }

        fn focus(&self, window: &DeskWindow) {
            self.run("hyprctl", &["dispatch", "focuswindow", &format!("address:{}", window.id)]);
        }
    }

    impl Desk for HyprDesk {
        fn windows(&self) -> Vec<DeskWindow> {
            self.run("hyprctl", &["clients", "-j"]).map(|out| chrome_windows(&out.stdout)).unwrap_or_default()
        }

        fn present(&self, window: &DeskWindow) {
            let address = format!("address:{}", window.id);
            let batch = format!(
                "dispatch focuswindow {address}; dispatch setfloating {address}; dispatch resizewindowpixel exact 1100 700,{address}; dispatch centerwindow 1"
            );
            self.run("hyprctl", &["--batch", &batch]);
        }

        fn capture(&self, window: &DeskWindow) -> Option<Vec<u8>> {
            let area = format!("{},{} {}x{}", window.x, window.y, window.w, window.h);
            self.run("grim", &["-g", &area, "-t", "jpeg", "-q", "75", "-"]).map(|out| out.stdout)
        }

        fn can_input(&self) -> bool {
            true
        }

        fn input(&self, window: &DeskWindow, input: &DeskInput) {
            let pointer = |x: f64, y: f64| {
                let (x, y) = window_point(window, x, y);
                self.run("hyprctl", &["dispatch", "movecursor", &x.to_string(), &y.to_string()]);
            };
            match input {
                DeskInput::Mouse { action, x, y } => {
                    pointer(*x, *y);
                    match action {
                        MouseAction::Move => {}
                        MouseAction::Down => {
                            self.focus(window);
                            self.run("ydotool", &["click", "0x40"]);
                        }
                        MouseAction::Up => {
                            self.run("ydotool", &["click", "0x80"]);
                        }
                    }
                }
                DeskInput::Wheel { x, y, down } => {
                    pointer(*x, *y);
                    self.run("ydotool", &["mousemove", "--wheel", "-x", "0", "-y", if *down { "-1" } else { "1" }]);
                }
                DeskInput::Key { key } => {
                    self.focus(window);
                    let name = match key {
                        DeskKey::Enter => "Return",
                        DeskKey::Tab => "Tab",
                        DeskKey::Backspace => "BackSpace",
                        DeskKey::Delete => "Delete",
                        DeskKey::Escape => "Escape",
                        DeskKey::ArrowLeft => "Left",
                        DeskKey::ArrowRight => "Right",
                        DeskKey::ArrowUp => "Up",
                        DeskKey::ArrowDown => "Down",
                        DeskKey::Home => "Home",
                        DeskKey::End => "End",
                    };
                    self.run("wtype", &["-k", name]);
                }
                DeskInput::Text { text } => {
                    self.focus(window);
                    self.run("wtype", &["--", text]);
                }
                DeskInput::Back => {
                    self.focus(window);
                    self.run("wtype", &["-M", "alt", "-k", "Left", "-m", "alt"]);
                }
            }
        }

        fn close(&self, window: &DeskWindow) {
            self.run("hyprctl", &["dispatch", "closewindow", &format!("address:{}", window.id)]);
        }
    }
}

#[cfg(windows)]
mod windows_desk {
    use super::*;
    use std::os::windows::process::CommandExt;
    use windows_sys::Win32::{
        Foundation::{CloseHandle, HWND, LPARAM, RECT},
        Graphics::Gdi::{
            BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject,
            SelectObject,
        },
        Storage::Xps::PrintWindow,
        System::{
            RemoteDesktop::ProcessIdToSessionId,
            StationsAndDesktops::{CloseDesktop, OpenInputDesktop},
            Threading::{GetCurrentProcessId, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW},
        },
        UI::{
            HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext},
            Input::KeyboardAndMouse::{
                INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, MOUSE_EVENT_FLAGS,
                MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_WHEEL, SendInput, VIRTUAL_KEY, VK_BACK, VK_DELETE, VK_DOWN,
                VK_END, VK_ESCAPE, VK_HOME, VK_LEFT, VK_MENU, VK_RETURN, VK_RIGHT, VK_TAB, VK_UP, mouse_event,
            },
            WindowsAndMessaging::{
                EnumWindows, GetClassNameW, GetWindowRect, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible, PostMessageW,
                SW_MAXIMIZE, SetCursorPos, SetForegroundWindow, ShowWindow, WM_CLOSE,
            },
        },
    };

    const PW_RENDERFULLCONTENT: u32 = 2;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const DESKTOP_READOBJECTS: u32 = 0x0001;

    pub struct WindowsDesk;

    fn handle(window: &DeskWindow) -> HWND {
        window.id.parse::<isize>().unwrap_or(0) as HWND
    }

    /// Por thread: a geometria, a captura e o clique precisam falar em pixel real.
    fn aware() {
        // SAFETY: troca só a consciência de DPI desta thread, sem ponteiro envolvido.
        unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    }

    fn is_chrome_process(pid: u32) -> bool {
        // SAFETY: o handle é conferido e fechado aqui, e o buffer vive durante a chamada.
        unsafe {
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if process.is_null() {
                return false;
            }
            let mut path = [0u16; 1024];
            let mut size = path.len() as u32;
            let read = QueryFullProcessImageNameW(process, 0, path.as_mut_ptr(), &mut size);
            CloseHandle(process);
            read != 0 && String::from_utf16_lossy(&path[..size as usize]).to_lowercase().ends_with("\\chrome.exe")
        }
    }

    unsafe extern "system" fn collect(window: HWND, found: LPARAM) -> i32 {
        // SAFETY: `found` é o `Vec` de `windows`, vivo durante o `EnumWindows`.
        unsafe {
            let found = &mut *(found as *mut Vec<DeskWindow>);
            if IsWindowVisible(window) == 0 {
                return 1;
            }
            let mut class = [0u16; 64];
            let length = GetClassNameW(window, class.as_mut_ptr(), class.len() as i32);
            if String::from_utf16_lossy(&class[..length.max(0) as usize]) != "Chrome_WidgetWin_1" {
                return 1;
            }
            let mut pid = 0u32;
            GetWindowThreadProcessId(window, &mut pid);
            if !is_chrome_process(pid) {
                return 1;
            }
            let mut title = [0u16; 512];
            let length = GetWindowTextW(window, title.as_mut_ptr(), title.len() as i32);
            if length <= 0 {
                return 1;
            }
            let mut rect = RECT { left: 0, top: 0, right: 0, bottom: 0 };
            GetWindowRect(window, &mut rect);
            found.push(DeskWindow {
                id: (window as isize).to_string(),
                title: String::from_utf16_lossy(&title[..length as usize]),
                x: rect.left,
                y: rect.top,
                w: rect.right - rect.left,
                h: rect.bottom - rect.top,
            });
            1
        }
    }

    fn keyboard(vk: VIRTUAL_KEY, scan: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk, wScan: scan, dwFlags: flags, time: 0, dwExtraInfo: 0 } },
        }
    }

    fn send(inputs: &[INPUT]) {
        // SAFETY: a lista vive durante a chamada, e o tamanho é o do tipo passado.
        unsafe { SendInput(inputs.len() as u32, inputs.as_ptr(), size_of::<INPUT>() as i32) };
    }

    fn mouse(flags: MOUSE_EVENT_FLAGS, data: i32) {
        // SAFETY: sem ponteiro; a posição já foi dada por `SetCursorPos`.
        unsafe { mouse_event(flags, 0, 0, data, 0) };
    }

    fn own_session() -> Option<u32> {
        let mut session = 0u32;
        // SAFETY: a saída é uma variável local.
        (unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut session) } != 0).then_some(session)
    }

    impl Desk for WindowsDesk {
        fn windows(&self) -> Vec<DeskWindow> {
            aware();
            let mut found = Vec::new();
            // SAFETY: `collect` só usa o `Vec` enquanto esta chamada durar.
            unsafe { EnumWindows(Some(collect), &mut found as *mut Vec<DeskWindow> as LPARAM) };
            found
        }

        fn present(&self, window: &DeskWindow) {
            // SAFETY: operações sobre um handle de janela; handle morto só faz a chamada falhar.
            unsafe {
                ShowWindow(handle(window), SW_MAXIMIZE);
                SetForegroundWindow(handle(window));
            }
        }

        fn capture(&self, window: &DeskWindow) -> Option<Vec<u8>> {
            aware();
            let (width, height) = (window.w, window.h);
            if width <= 0 || height <= 0 {
                return None;
            }
            // Em DIB próprio, sem o DC da tela: é o que continua valendo na sessão desconectada.
            // SAFETY: os objetos GDI são conferidos e liberados aqui, e `bits` vale até o `DeleteObject`.
            unsafe {
                let dc = CreateCompatibleDC(std::ptr::null_mut());
                if dc.is_null() {
                    return None;
                }
                let mut info: BITMAPINFO = std::mem::zeroed();
                info.bmiHeader = BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width,
                    biHeight: -height,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB,
                    ..std::mem::zeroed()
                };
                let mut bits = std::ptr::null_mut();
                let bitmap = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
                if bitmap.is_null() || bits.is_null() {
                    DeleteDC(dc);
                    return None;
                }
                let previous = SelectObject(dc, bitmap);
                let printed = PrintWindow(handle(window), dc, PW_RENDERFULLCONTENT) != 0;
                let bgra = std::slice::from_raw_parts(bits as *const u8, (width as usize) * (height as usize) * 4);
                let rgb: Vec<u8> = bgra.chunks_exact(4).flat_map(|pixel| [pixel[2], pixel[1], pixel[0]]).collect();
                SelectObject(dc, previous);
                DeleteObject(bitmap);
                DeleteDC(dc);
                if !printed {
                    return None;
                }
                let mut jpeg = Vec::new();
                image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 75)
                    .encode(&rgb, width as u32, height as u32, image::ExtendedColorType::Rgb8)
                    .ok()?;
                Some(jpeg)
            }
        }

        fn can_input(&self) -> bool {
            // SAFETY: o handle é conferido e fechado aqui.
            unsafe {
                let desktop = OpenInputDesktop(0, 0, DESKTOP_READOBJECTS);
                if desktop.is_null() {
                    return false;
                }
                CloseDesktop(desktop);
                true
            }
        }

        fn input(&self, window: &DeskWindow, input: &DeskInput) {
            aware();
            let pointer = |x: f64, y: f64| {
                let (x, y) = window_point(window, x, y);
                // SAFETY: sem ponteiro.
                unsafe { SetCursorPos(x, y) };
            };
            let front = || {
                // SAFETY: handle morto só faz a chamada falhar.
                unsafe { SetForegroundWindow(handle(window)) };
            };
            match input {
                DeskInput::Mouse { action, x, y } => {
                    pointer(*x, *y);
                    match action {
                        MouseAction::Move => {}
                        MouseAction::Down => {
                            front();
                            mouse(MOUSEEVENTF_LEFTDOWN, 0);
                        }
                        MouseAction::Up => mouse(MOUSEEVENTF_LEFTUP, 0),
                    }
                }
                DeskInput::Wheel { x, y, down } => {
                    pointer(*x, *y);
                    mouse(MOUSEEVENTF_WHEEL, if *down { -120 } else { 120 });
                }
                DeskInput::Key { key } => {
                    front();
                    let vk = match key {
                        DeskKey::Enter => VK_RETURN,
                        DeskKey::Tab => VK_TAB,
                        DeskKey::Backspace => VK_BACK,
                        DeskKey::Delete => VK_DELETE,
                        DeskKey::Escape => VK_ESCAPE,
                        DeskKey::ArrowLeft => VK_LEFT,
                        DeskKey::ArrowRight => VK_RIGHT,
                        DeskKey::ArrowUp => VK_UP,
                        DeskKey::ArrowDown => VK_DOWN,
                        DeskKey::Home => VK_HOME,
                        DeskKey::End => VK_END,
                    };
                    send(&[keyboard(vk, 0, 0), keyboard(vk, 0, KEYEVENTF_KEYUP)]);
                }
                DeskInput::Text { text } => {
                    front();
                    let inputs: Vec<INPUT> = text
                        .encode_utf16()
                        .flat_map(|unit| {
                            [keyboard(0, unit, KEYEVENTF_UNICODE), keyboard(0, unit, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP)]
                        })
                        .collect();
                    send(&inputs);
                }
                DeskInput::Back => {
                    front();
                    send(&[
                        keyboard(VK_MENU, 0, 0),
                        keyboard(VK_LEFT, 0, 0),
                        keyboard(VK_LEFT, 0, KEYEVENTF_KEYUP),
                        keyboard(VK_MENU, 0, KEYEVENTF_KEYUP),
                    ]);
                }
            }
        }

        fn close(&self, window: &DeskWindow) {
            // SAFETY: handle morto só faz a chamada falhar.
            unsafe { PostMessageW(handle(window), WM_CLOSE, 0, 0) };
        }

        fn activate(&self) -> bool {
            let Some(session) = own_session() else { return false };
            std::process::Command::new("tscon")
                .args([&session.to_string(), "/dest:console"])
                .creation_flags(CREATE_NO_WINDOW)
                .status()
                .is_ok_and(|status| status.success())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window() -> DeskWindow {
        DeskWindow { id: "w".into(), title: "t".into(), x: 100, y: 50, w: 800, h: 600 }
    }

    #[test]
    fn point_stays_inside_the_login_window() {
        assert_eq!(window_point(&window(), 10.0, 20.0), (110, 70));
        assert_eq!(window_point(&window(), -500.0, 5000.0), (100, 649));
        assert_eq!(window_point(&window(), 5000.0, -1.0), (899, 50));
        assert_eq!(window_point(&window(), f64::NAN, f64::INFINITY), (100, 50));
    }

    #[test]
    fn keys_outside_the_list_are_refused() {
        assert!(serde_json::from_str::<DeskInput>(r#"{"t":"key","key":"Enter"}"#).is_ok());
        assert!(serde_json::from_str::<DeskInput>(r#"{"t":"key","key":"Super_L"}"#).is_err());
        assert!(serde_json::from_str::<DeskInput>(r#"{"t":"shell","cmd":"x"}"#).is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn hyprland_list_keeps_only_mapped_chrome_windows() {
        let clients = br#"[
            {"address":"0x1","class":"google-chrome","title":"Claude","at":[1,2],"size":[3,4],"mapped":true},
            {"address":"0x2","class":"kitty","title":"t","at":[0,0],"size":[1,1],"mapped":true},
            {"address":"0x3","class":"google-chrome","title":"h","at":[0,0],"size":[1,1],"mapped":false}
        ]"#;
        let windows = hyprland::chrome_windows(clients);
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0], DeskWindow { id: "0x1".into(), title: "Claude".into(), x: 1, y: 2, w: 3, h: 4 });
    }
}
