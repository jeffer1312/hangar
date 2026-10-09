# Cópia modificada do gpui-base 0.7.0

Origem: crate `gpui-base` 0.7.0 do crates.io (repositório `longbridge/gpui-kit`), licença Apache-2.0 (`LICENSE-APACHE`).
Entra no build por `[patch.crates-io]` em `desktop-native/Cargo.toml`.

Mudanças, todas marcadas com "Modified for Hangar":

1. `src/text/style.rs` e `src/text/node.rs`: três campos novos no `TextViewStyle`, **desligados por padrão**
   (`None`), então toda view que não os liga continua com o desenho do kit.
   - `with_list_marker_width`: o marcador da lista fica numa coluna dessa largura, alinhado à direita, e a lista
     aninhada e o parágrafo de continuação recuam a mesma largura (`render_list_item_row`, `render_list_item`).
     O número do marcador segue o `list_start` do kit (lista que começa em 3 mostra 3).
   - `with_list_marker_color`: números e marcadores da lista pintados nessa cor, com ou sem coluna.
   - `with_code_language_band`: o bloco de código ganha uma faixa no topo com a linguagem (ou o rótulo dado,
     quando a cerca não nomeia nenhuma) e as ações do bloco à direita, em vez de no canto sobre o código
     (`CodeBlock::render`). O código dentro da faixa mantém os destaques de trecho e o `reveal` do kit.
   - Testes: os campos entram no `PartialEq` e nascem `None` (testes do `style.rs`).

   Motivo: o kit não tem gancho para o recuo e a cor do marcador nem para a faixa de linguagem; só o estilo da
   conversa do app liga os três.

2. `src/text/style.rs` e `src/text/node.rs`: `inline_code_font_family`, opcional e desligada
   por padrão, permite que uma view preserve a fonte dos trechos de código quando o tema muda.
   O campo participa do `PartialEq` (e por isso invalida o cache de texto do parágrafo) e da medição dos mesmos
   trechos que desenha, porque todos passam por `mark_highlight`.
   Motivo: as prévias de Nova sessão mantêm sua tipografia própria.

Removido na 0.7.0: o conserto de `src/text_selection.rs` (participante de view `.cached` reusada varrido no fim do
quadro). O kit passou a resolver isso com o `RenderedMarker` guardado no estado do elemento
(`text_selection.rs:206-245`), então o arquivo voltou ao original.

Ao subir a versão do gpui-kit, reaplicar estas mudanças na versão nova, ou remover a cópia se o kit já trouxer o conserto.

Os testes que montam `KeyDownEvent` passam `physical_digit: None`, o campo novo da cópia de gpui-pre.

Acessibilidade: `Slider::aria_label` dá nome ao slider; o parágrafo do markdown (`text/inline_flow.rs`) entrega o texto
à árvore por `Window::a11y_text`.
