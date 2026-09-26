# Cópia do gpui-pre-wgpu 0.3.6

Origem: crate `gpui-pre-wgpu` 0.3.6 do crates.io, licença Apache-2.0 (`LICENSE-APACHE`).
Entra no build por `[patch.crates-io]` em `desktop-native/Cargo.toml`.

O desfoque de fundo foi portado do zui `18a89af`: kernel, WGSL e renderer wgpu. O renderer intercala as passadas pela ordem da cena, usa `COPY_SRC` na superfície quando disponível ou um alvo copiável intermediário, reutiliza o scratch e o libera após 30 quadros sem desfoque.
