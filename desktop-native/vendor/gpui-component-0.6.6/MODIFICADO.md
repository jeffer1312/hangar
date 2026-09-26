# Cópia modificada do gpui-component 0.6.6

Origem: crate `gpui-component` 0.6.6 do crates.io (repositório `longbridge/gpui-kit`), licença Apache-2.0 (`LICENSE-APACHE`). Entra pelo `[patch.crates-io]` de `desktop-native/Cargo.toml`.

- `src/menu/popup_menu.rs`: `PopupMenuAppearance` oferece refinamentos da superfície, lista, linhas e separadores, altura de linha e renderizadores de título e tecla. Só se aplica por `PopupMenu::appearance`; sem opt-in, permanece o desenho original. Foco, teclado, seleção, submenu, dispensa e acessibilidade permanecem no kit.
- `PopupMenu::replace_item`: substitui uma linha sem reconstruir irmãos ou zerar a seleção; índice inexistente retorna `false`. Permite atualizar silenciamento sem fechar o submenu de branches.
- `src/menu/mod.rs`: exporta `PopupMenuAppearance`.
- Com apresentação opt-in, a primeira seta para baixo, a volta ao início e a entrada no submenu por ←/→ escolhem a primeira ação disponível, pulando títulos. Sem opt-in, permanece a seleção original do kit.
- A verificação de teclado e atualização com submenu aberto é feita na prova real do app. Os testes originais do crate foram preservados; o crate copiado não faz parte do workspace de testes do app.

Ao atualizar gpui-kit, reaplicar na nova versão ou remover esta cópia quando a API equivalente existir no kit.
