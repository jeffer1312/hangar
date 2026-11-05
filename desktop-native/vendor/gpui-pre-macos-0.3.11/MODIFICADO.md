# Cópia do gpui-pre-macos 0.3.11

Origem: crate `gpui-pre-macos` 0.3.11 do crates.io, checksum `d7410cfdaa1fa55a414346cc492d18ce340e3a8b6d0ac3213b984ed690c82b92`, licença Apache-2.0.

Esta cópia entra no build por `[patch.crates-io]` e usa a cópia modificada de `gpui-pre-apple`.

Tecla física: `physical_digit(key_code)` (`src/events.rs`) mapeia os códigos virtuais `kVK_ANSI_1` a `kVK_ANSI_0` para
`KeyDownEvent::physical_digit`; o `KeyDownEvent` do `do_command_by_selector` (`src/window.rs`) passa `None`. Não
compilado aqui; o CI do `native.yml` compila no macOS.
