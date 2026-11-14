# Cópia modificada do gpui-base 0.9.1

Origem: crate `gpui-base` 0.9.1 do crates.io (repositório `longbridge/gpui-kit`), checksum `f3e9ae5237737cfd110d0ff513fcd9c20f97d98ebabd0a73164068177f0aca4f`, licença Apache-2.0 (`LICENSE-APACHE`).
Entra no build por `[patch.crates-io]` em `desktop-native/Cargo.toml`.

Mudanças, todas marcadas com "Modified for Hangar":

1. `src/text/style.rs` e `src/text/node.rs`: dois campos novos no `TextViewStyle`, **desligados por padrão**
   (`None`), então toda view que não os liga continua com o desenho do kit.
   - `with_list_marker_width`: a lista aninhada e o parágrafo de continuação recuam essa largura, para acompanhar a
     coluna fixa que o app dá ao marcador por `with_list_style`.
   - `with_code_language_band`: o bloco de código ganha uma faixa no topo com a linguagem (ou o rótulo dado,
     quando a cerca não nomeia nenhuma) e as ações do bloco à direita, em vez de no canto sobre o código
     (`CodeBlock::render`). O código dentro da faixa mantém os destaques de trecho e o `reveal` do kit.
   - Testes: os campos entram no `PartialEq` e nascem `None` (testes do `style.rs`).

   Motivo: o kit não tem gancho para o recuo da lista nem para a faixa de linguagem; só o estilo da conversa do app
   liga os dois.

2. `src/text/style.rs` e `src/text/node.rs`: `inline_code_font_family`, opcional e desligada
   por padrão, permite que uma view preserve a fonte dos trechos de código quando o tema muda.
   O campo participa do `PartialEq` (e por isso invalida o cache de texto do parágrafo) e da medição dos mesmos
   trechos que desenha, porque todos passam por `mark_highlight`.
   Motivo: as prévias de Nova sessão mantêm sua tipografia própria.

3. `src/text/node.rs` e `src/scrollable_mask.rs`: tabela com rolagem (`overflow.x = Scroll`) encolhe cada coluna
   até a maior palavra dela (`measure_table_columns` devolve também essa largura), com teto de 160 px, em vez do
   piso do kit (metade do conteúdo, de 160 a 480 px); e o `horizontal_scroll_area` ganha a barra horizontal sempre
   visível quando há o que rolar. Teste: `scroll_table_of_prose_fits_the_frame` (`text_view.rs`).
   Motivo: tabela de prosa de 4 colunas passava da coluna do chat (880 px) e a última coluna ficava cortada, sem
   barra que mostrasse que dava para rolar.

4. `src/text/document.rs`: no modo rolável, cada bloco entra no `gpui::list` dentro de um `div` sem estilo.
   Motivo: a lista mede cada item como raiz de layout, e a raiz perde a própria margem; sem o invólucro, o `mt`/`mb`
   que o app dá ao título (`with_heading`) não ocupava espaço, e o fio embaixo do título do leitor de documentos
   encostava no parágrafo seguinte. Estilo sem margem desenha igual.

5. `src/text/format/markdown.rs`, `src/text/node.rs` e `src/text/utils.rs`: lista numerada cujos números digitados
   sempre crescem (`1. 3. 7.`) mostra esses números, em vez de início + posição, em dígitos também na sublista. `BlockNode::ListItem` ganha
   `value` (lido da fonte na posição do item por `list_item_number`), e `ordered_list_ordinal`/`list_item_prefix`
   recebem esse número; a tela, a cópia da seleção e o `to_markdown` passam pela mesma função. Lista com número
   repetido ou fora de ordem (`1. 1. 1.`, `1. 1. 2.`) segue a contagem do CommonMark; o parser HTML não lê
   `<li value>`. Testes: `ordered_list_keeps_the_typed_numbers_only_when_they_grow` (`markdown.rs`) e
   `test_list_item_prefix` (`utils.rs`).
   Motivo: quem responde perguntas por número (`1. … 3. … 7. …`) via a bolha renumerar para 1, 2, 3.

Removido na 0.7.0: o conserto de `src/text_selection.rs` (participante de view `.cached` reusada varrido no fim do
quadro). O kit passou a resolver isso com o `RenderedMarker` guardado no estado do elemento
(`text_selection.rs:206-245`), então o arquivo voltou ao original.

Ao subir a versão do gpui-kit, reaplicar estas mudanças na versão nova, ou remover a cópia se o kit já trouxer o conserto.

Os testes que montam `KeyDownEvent` passam `physical_digit: None`, o campo novo da cópia de gpui-pre.

Acessibilidade: o parágrafo do markdown (`text/inline_flow.rs`) entrega o texto
à árvore por `Window::a11y_text`.

Rolagem: `text/block_column.rs` guarda onde cada bloco de um documento sem rolagem própria fica e, a partir de oito
blocos, monta só os que estão dentro da máscara de conteúdo. Seleção, revelação, fade do texto chegando, `max_lines` e
mudança de largura ou de altura voltam a montar todos.
