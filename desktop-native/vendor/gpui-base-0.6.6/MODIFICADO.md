# Cópia modificada do gpui-base 0.6.6

Origem: crate `gpui-base` 0.6.6 do crates.io (repositório `longbridge/gpui-kit`), licença Apache-2.0 (`LICENSE-APACHE`).
Entra no build por `[patch.crates-io]` em `desktop-native/Cargo.toml`.

Mudança, só em `src/text_selection.rs` (marcada com "Modified for Hangar"):

- O registro do participante guarda a view que o pintou (`window.current_view()`).
- `TextSelection::view_rendered` e `TextSelection::retain_cached_view`: o participante de uma view guardada
  (`.cached`) que foi reusada no quadro não é mais varrido no fim do quadro, e continua selecionável.
- Um teste: `reused_cached_view_keeps_its_participants`.

Motivo: a varredura do fim do quadro apaga quem não se pintou, e uma view reusada do cache não pinta.
Ao subir a versão do gpui-kit, reaplicar estas mudanças na versão nova, ou remover a cópia se o kit já trouxer o conserto.
