# Cópia modificada do gpui-pre-linux 0.3.11

Origem: crate `gpui-pre-linux` 0.3.11 do crates.io, checksum `79cac5cbd3485c1e3db8eafd5dc69019188f5cb0a39846946af772efa386f624`, licença Apache-2.0 (`LICENSE-APACHE`).
Entra no build por `[patch.crates-io]` em `desktop-native/Cargo.toml`.

`src/linux/wayland/window.rs` e `src/linux/x11/window.rs` implementam `PlatformWindow::draw_with_damage`, que repassa
ao renderer wgpu a região da cena que mudou desde o último quadro. Portado do PR #62455 de zed-industries/zed
(revisão `d9c29a3`), Apache-2.0. Cada arquivo alterado traz um aviso no cabeçalho.

Bandeja: `PlatformWindow::set_hidden` nos dois backends. No Wayland (`src/linux/wayland/window.rs`) esconder destrói
o toplevel e o `xdg_surface`, tira o buffer da `wl_surface` e cria os dois de novo nela, sem commit; mostrar reaplica
título, `app_id`, tamanhos e modo de decoração e faz o commit inicial, que traz o configure como na criação da janela.
A superfície e o renderer são os mesmos do começo ao fim, e o laço de quadros fica em `Unconfigured` enquanto a janela
está escondida. Vale só para a janela principal (sem pai e sem diálogo), e não toca nos callbacks, porque o app chama
de dentro do aviso de fechar. Só desmapear com buffer nulo não serve: o Hyprland não manda configure no commit
seguinte. No X11
(`src/linux/x11/window.rs`) é `UnmapWindow`/`MapWindow`.

Colar arquivo: com `text/uri-list` na oferta, o `read()` do CLIPBOARD no Wayland (`src/linux/wayland/clipboard.rs`)
e o `get_any` do X11 (`src/linux/x11/clipboard.rs`) devolvem `ExternalPaths` mais o texto que o dono ofereceu, como o
macOS e o Windows já fazem; sem texto do dono, os caminhos um por linha. A seleção primária continua só texto e o
arrastar não muda. Leitura das linhas e montagem do item em `parse_uri_list` e `file_list_item`
(`src/linux/platform.rs`). No X11 o `Inner::read` foi dividido em `targets` + `read_from` para o colar consultar o
TARGETS uma vez só e escolher dele o uri-list e o texto.

Tecla física: `physical_digit(keycode)` (`src/linux/platform.rs`) mapeia os códigos xkb 10 a 19 da fileira de números
para `KeyDownEvent::physical_digit`, no Wayland (tecla e repetição) e no X11. O commit do IME passa `None`.
