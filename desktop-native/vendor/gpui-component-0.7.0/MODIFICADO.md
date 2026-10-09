# Cópia modificada do gpui-component 0.7.0

Origem: crate `gpui-component` 0.7.0 do crates.io (repositório `longbridge/gpui-kit`), licença Apache-2.0 (`LICENSE-APACHE`). Entra pelo `[patch.crates-io]` de `desktop-native/Cargo.toml`.

- `src/menu/popup_menu.rs`: `PopupMenuAppearance` oferece refinamentos da superfície, lista, linhas e separadores, altura de linha e renderizadores de título e tecla. Só se aplica por `PopupMenu::appearance`; sem opt-in, permanece o desenho original. Foco, teclado, seleção, submenu, dispensa e acessibilidade permanecem no kit.
- `PopupMenu::replace_item`: substitui uma linha sem reconstruir irmãos ou zerar a seleção; índice inexistente retorna `false`. Permite atualizar silenciamento sem fechar o submenu de branches.
- `src/menu/mod.rs`: exporta `PopupMenuAppearance`.
- Com apresentação opt-in, a primeira seta para baixo, a volta ao início e a entrada no submenu por ←/→ escolhem a primeira ação disponível, pulando títulos. Sem opt-in, permanece a seleção original do kit.
- A verificação de teclado e atualização com submenu aberto é feita na prova real do app. Os testes originais do crate foram preservados; o crate copiado não faz parte do workspace de testes do app.
- Na 0.7.0 o `popup_menu.rs` do kit não mudou; as correções de vazamento (#3224, #3267, #3279) ficaram em
  `context_menu.rs`, `dropdown_menu.rs` e no `Popover` do gpui-base. Os acréscimos acima não guardam entidade nem
  inscrição: os renderizadores ficam dentro do próprio `PopupMenu`.

- `src/text/style.rs` e `src/text/compat.rs`: `inline_code_font_family` opcional no estilo,
  repassada ao gpui-base (`resolve_component_style`, junto do `with_heading` que a 0.7.0 passou a usar) e incluída
  no `PartialEq`. Sem escolha local, preserva o tema.
  Motivo: as prévias de Nova sessão mantêm a fonte antiga ao mudar a fonte global de código.

- `src/dialog/dialog.rs`: `background_painter` opcional pinta o fundo após o layout e antes
  do conteúdo, cobrindo todo o cartão. Na 0.7.0 o cartão fica dentro do `Positioner::corner` do gpui-base e o
  pintor continua como primeiro filho do cartão. Sem opt-in, o fundo original permanece. Motivo:
  permitir vidro desfocado no diálogo do app sem alterar o comportamento do kit.

- `src/popover.rs` e `src/menu/popup_menu.rs`: global opcional `PopupSurface` com um pintor do app. Com ele,
  `dropdown_popup` (Select, Combobox, DatePicker) e o `PopupMenu` tiram o próprio fundo e a sombra, e o pintor roda
  sob o conteúdo, na camada da superfície. Sem o global, o desenho original permanece. Motivo: vidro desfocado e a
  superfície dos popovers do app nos menus e listas do kit, sem mexer em cada tela que os usa.

- `src/popover.rs` (`dropdown_popup`), `src/menu/context_menu.rs` e `src/dialog/dialog.rs`: entradas com os tempos do
  catálogo de movimento do Zeron que o app usa (`desktop-native/src/motion.rs`): listas suspensas e menus de contexto no
  `menu-in` (140 ms `ease`, 2 px acima e opacidade de 30%), diálogos no `dialog-in` (180 ms `ease`, subindo 2 px enquanto
  aparecem, no lugar da descida desde o topo). Motivo: um só movimento em todos os popovers e diálogos do app.
  Na 0.7.0: o menu de contexto passou a ser montado no `DeferredMenu::build_menu` (só quando é desenhado) e a entrada
  foi para lá; no diálogo, a subida de 2 px vai na animação `slide-down` do `Positioner`, que substituiu o `top` do
  cartão.

Ao atualizar gpui-kit, reaplicar na nova versão com `vendor/rebase.sh` (ver `vendor/PATCHES.md`) ou remover esta cópia
quando a API equivalente existir no kit.

Os testes que montam `KeyDownEvent` passam `physical_digit: None`, o campo novo da cópia de gpui-pre.

Acessibilidade do `Button`: sem rótulo nem `accessibility_label`, o nome acessível é o texto da dica (botão só de
ícone); `Button::aria_selected` marca a escolha (abas, modo) sem o visual de `selected`. Botão com `selected` anuncia selecionado. `Slider::aria_label`
repassa o nome ao slider de gpui-base. O item de menu com a marca de escolhido (`checked`) anuncia marcado, e o
invólucro do diálogo (`dialog/dialog.rs`) tem o papel de diálogo.
