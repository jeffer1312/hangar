# Cópia modificada do gpui-base 0.6.6

Origem: crate `gpui-base` 0.6.6 do crates.io (repositório `longbridge/gpui-kit`), licença Apache-2.0 (`LICENSE-APACHE`).
Entra no build por `[patch.crates-io]` em `desktop-native/Cargo.toml`.

Mudanças, todas marcadas com "Modified for Hangar":

1. `src/text_selection.rs`
   - O registro do participante guarda a view que o pintou (`window.current_view()`).
   - `TextSelection::view_rendered` e `TextSelection::retain_cached_view`: o participante de uma view guardada
     (`.cached`) que foi reusada no quadro não é mais varrido no fim do quadro, e continua selecionável.
   - Um teste: `reused_cached_view_keeps_its_participants`.

   Motivo: a varredura do fim do quadro apaga quem não se pintou, e uma view reusada do cache não pinta.

2. `src/text/style.rs` e `src/text/node.rs`: dois campos novos no `TextViewStyle`, **desligados por padrão**
   (`None`), então toda view que não os liga continua com o desenho do kit.
   - `with_list_marker_width`: o marcador da lista fica numa coluna dessa largura, alinhado à direita, e a lista
     aninhada e o parágrafo de continuação recuam a mesma largura (`render_list_item_row`, `render_list_item`).
   - `with_code_language_band`: o bloco de código ganha uma faixa no topo com a linguagem (ou o rótulo dado,
     quando a cerca não nomeia nenhuma) e as ações do bloco à direita, em vez de no canto sobre o código
     (`CodeBlock::render`).
   - Testes: os dois campos entram no `PartialEq` e nascem `None` (testes do `style.rs`).

   Motivo: o kit não tem gancho para o recuo do marcador nem para a faixa de linguagem; só o estilo da conversa
   do app liga os dois.

Ao subir a versão do gpui-kit, reaplicar estas mudanças na versão nova, ou remover a cópia se o kit já trouxer o conserto.
