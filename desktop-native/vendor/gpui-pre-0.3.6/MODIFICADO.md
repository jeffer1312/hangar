# Cópia modificada do gpui-pre 0.3.6

Origem: crate `gpui-pre` 0.3.6 do crates.io, licença Apache-2.0 (`LICENSE-APACHE`).
Entra no build por `[patch.crates-io]` em `desktop-native/Cargo.toml`.

`src/scene.rs` e `src/window.rs` foram modificados pelo Hangar para receber regiões de desfoque do conteúdo já pintado. O mecanismo de ordem, repetição de cenas guardadas e a entrada de pintura foram portados de `zeronsh/zui` na revisão `18a89af`, também Apache-2.0. Cada arquivo alterado traz um aviso no cabeçalho.

O renderer usa essas regiões em uma etapa posterior. Sem suporte no renderer, `paint_backdrop_blur` não muda os pixels; quem chama deve manter um preenchimento translúcido como reserva.
