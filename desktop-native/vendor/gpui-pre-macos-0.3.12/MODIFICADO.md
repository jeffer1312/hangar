# Cópia do gpui-pre-macos 0.3.12

Origem: crate `gpui-pre-macos` 0.3.12 do crates.io, checksum `2de7d3f4ad51ac387e822495db363fcc1188d349793365731d054444c4cb06ee`, licença Apache-2.0.

Esta cópia entra no build por `[patch.crates-io]` e usa a cópia modificada de `gpui-pre-apple`.

Tecla física: `physical_digit(key_code)` (`src/events.rs`) mapeia os códigos virtuais `kVK_ANSI_1` a `kVK_ANSI_0` para
`KeyDownEvent::physical_digit`; o `KeyDownEvent` do `do_command_by_selector` (`src/window.rs`) passa `None`. Não
compilado aqui; o CI do `native.yml` compila no macOS.
