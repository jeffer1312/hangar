# Crates vendorizados e os ajustes do Hangar

Cada `vendor/<crate>-<versão>` é o crate original do crates.io com os ajustes do Hangar por cima, e entra no build pelo
`[patch.crates-io]` de `desktop-native/Cargo.toml`. O detalhe de cada ajuste está no `MODIFICADO.md` da cópia; o diff
exato contra o original está em `patches/<crate>-<versão>.patch` (aplica com `patch -p0` num diretório que contenha o
original extraído).

## Atualizar para uma versão nova

1. `vendor/rebase.sh bump <crate> <versão-atual> <versão-nova>` para cada crate. O script baixa os dois originais,
   cria a cópia nova e funde cada ajuste por 3 vias. Arquivo que o upstream não tocou entra limpo; onde os dois mexeram
   ficam marcadores `<<<<<<< upstream` / `>>>>>>> ours`.
2. Resolver os conflitos portando a intenção do ajuste para a estrutura nova, não escolhendo um lado. Conferir também os
   arquivos que entraram limpos contra o que o upstream mudou em volta deles.
3. Apontar o `[patch.crates-io]` e a fixação do `gpui-kit` para as versões novas, compilar e adaptar o app.
4. `vendor/rebase.sh export` para regravar os `.patch`, e apagar as cópias antigas.
5. Atualizar a tabela abaixo.

Depois de qualquer mudança à mão num crate vendorizado, rodar `vendor/rebase.sh export`: o `.patch` é o registro do que
fizemos, e só vale se acompanhar a cópia.

## Estado na atualização 0.9.0 → 0.9.1 (gpui-pre 0.3.11 → 0.3.12)

Todos os ajustes seguem, menos um. Onde os dois lados mexeram:

| Ajuste | Onde | Estado | Por quê |
|---|---|---|---|
| `CreateTargetForHwnd(hwnd, false)` | gpui-pre-windows `directx_renderer.rs` | descartado | a 0.3.12 já cria o alvo com `false` |
| Folga e régua do fim da lista | gpui-pre `elements/list.rs` | adaptado | a lista passou a descontar recuo e borda horizontais da largura do item; a nossa limpeza da folga ficou antes dessa conta |
| Nós de acessibilidade do desenho adiado | gpui-pre `window.rs` | adaptado | o upstream passou a guardar no desenho adiado as entidades lidas; a captura dos nós envolve a mesma chamada |
| Número digitado do item de lista (`value`) | gpui-base `text/node.rs`, `format/*`, `editor_host.rs` | adaptado | `BlockNode::ListItem` ganhou `task_marker` do kit; os dois campos convivem |
| Blocos fora da tela não montados | gpui-base `text/mod.rs`/`state.rs` | mantido | só imports ao lado do bloco de superfície novo do kit |
| Aparência do menu | gpui-component `menu/popup_menu.rs` | mantido | só imports |

## Estado na atualização 0.7.0 → 0.9.0 (gpui-pre 0.3.7 → 0.3.11)

| Ajuste | Onde | Estado | Por quê |
|---|---|---|---|
| Desfoque de fundo (vidro) | gpui-pre `scene.rs`/`window.rs`, gpui-pre-wgpu, -apple, -windows | reescrito no -apple, mantido nos demais | a 0.3.11 trocou `metal`/`objc` por `objc2-metal` no renderer do Mac; o desfoque foi portado para essa API e o kernel do MPS passou a viver num `Retained` |
| Redesenho parcial e pular quadro igual | gpui-pre `scene_damage.rs`/`window.rs`/`platform.rs`, gpui-pre-linux, gpui-pre-wgpu, gpui-pre-windows `directx_renderer.rs`/`backdrop.rs`/`window.rs` | adaptado | o renderer do Linux virou `Option` nas janelas, o `present` passou para a fila e a configuração da superfície saiu de `new_internal`. O upstream não tem nada equivalente |
| `text_input_focused` | gpui-pre `window.rs` | adaptado | lê `focused_text_input`, o campo novo do upstream |
| Coluna do marcador de lista, cor do marcador | gpui-base `text/node.rs`/`style.rs` | adaptado | o kit passou a ter a coluna e `with_list_style`, que dá largura e cor ao marcador e também contorna a caixa desmarcada de lista de tarefas. Só o recuo do conteúdo ficou nosso (`with_list_marker_width`): com a coluna do kit, que cresce, o item 10 sairia do alinhamento |
| Imagem por referência no markdown | gpui-base `text/format/markdown.rs` | descartado | a 0.9.0 resolve sozinha |
| Rótulo de acessibilidade do `Slider` | gpui-base e gpui-component `slider.rs` | descartado | o kit passou a ter `accessibility_label` |
| Marca de escolhido e primeira opção do menu | gpui-component `menu/menu_item.rs`/`popup_menu.rs` | descartado | o kit anuncia `checked` e pula títulos na primeira seta |
| Foco do menu de contexto | gpui-component `menu/context_menu.rs` | descartado | o kit foca o menu antes do primeiro quadro, como fazíamos |
| Faixa de linguagem do bloco de código | gpui-base `text/node.rs`/`style.rs` | mantido | a faixa convive com o rótulo de linguagem novo do kit, que só aparece sem ela |
| Fonte própria do código inline | gpui-base `text/*`, gpui-component `text/style.rs`/`compat.rs` | adaptado | a fonte da view vence a do `with_inline_code_style` novo do kit |
| Piso da coluna da tabela | gpui-base `text/node.rs` | adaptado | a tabela do kit foi reescrita com cache de larguras; a largura da maior palavra entrou nesse cache |
| Entrada `dialog-in` do diálogo | gpui-component `dialog/dialog.rs` | adaptado | o kit trocou `Animation` por `Transition`; a curva `ease` e a subida de 2 px foram para lá |
| Diálogo centrado (`Positioner::centered`) | gpui-base `positioner.rs`, gpui-component `dialog/dialog.rs` | descartado | o kit centra pela altura real e mantém o deslocamento entre diálogos empilhados |
| `KeyBinding::load_with_mapper` | gpui-pre `keymap/binding.rs` | novo | `load` passou a pedir `App`, e a config de atalhos e seus testes só têm o mapper |

gpui-pre-apple/-macos: só checagem de tipos para `aarch64-apple-darwin`, com o shader trocado por um arquivo vazio. O `.metal` e o desfoque nunca rodaram num Mac; o build completo é do `native-macos.yml`.

## Navegador embutido

| Ajuste | Onde | Por quê |
|---|---|---|
| `PaintSurface::texture` e `Window::paint_surface(bounds, texture)` no Linux | gpui-pre `scene.rs`/`window.rs` | a página do WPE WebKit chega como textura GPU externa; o upstream só pinta superfície no macOS (`CVPixelBuffer`) |
| Superfície sempre conta como dano | gpui-pre `scene_damage.rs` (já vinha do PR #62455; teste novo) | a primitiva fica idêntica enquanto os pixels da página mudam; sem isso o quadro seria pulado como igual |
| Dispositivo Vulkan com `VK_EXT_image_drm_format_modifier` e `WgpuContext::shared_device()` | gpui-pre-wgpu `wgpu_context.rs` | importar o DMA-BUF sem cópia exige o modificador DRM e o mesmo dispositivo que o GPUI usa para desenhar |
| `PrimitiveBatch::Surfaces` desenhado como sprite policromático, alpha forçado a 1 | gpui-pre-wgpu `wgpu_renderer.rs`/`shaders.wgsl` | o renderer wgpu ignorava superfícies; em XRGB o byte X não é alpha |
| `PaintSurface::texture`/`paint_surface` também no Windows | gpui-pre `scene.rs`/`window.rs` | a página da conversa chega do screencast da WebView2 como textura externa, como no Linux |
| `SurfaceTexture` e `draw_surfaces` desenhando a textura como sprite policromático | gpui-pre-windows `directx_devices.rs`/`directx_renderer.rs` | o DirectX ignorava superfícies; a textura é criada no device do GPUI e escrita na thread da janela |

## Bandeja

| Ajuste | Onde | Por quê |
|---|---|---|
| `PlatformWindow::set_hidden` e `Window::set_hidden` | gpui-pre `platform.rs`/`window.rs`, gpui-pre-linux `wayland/window.rs`/`x11/window.rs`, gpui-pre-windows `window.rs` | o upstream não esconde janela; fechar para a bandeja precisa da janela viva, porque a tela do app não sobrevive a fechar e reabrir |

## Colar arquivo

| Ajuste | Onde | Por quê |
|---|---|---|
| `read()` do CLIPBOARD devolve `ExternalPaths` + texto quando a oferta tem `text/uri-list` | gpui-pre-linux `platform.rs`, `wayland/clipboard.rs`, `x11/clipboard.rs` (`Inner::read` dividido em `targets` + `read_from`) | o upstream no Linux só lê texto e imagem; arquivo copiado no gerenciador não chegava ao composer. Terminal e navegador seguem pelo texto (`ClipboardItem::text` ignora os caminhos quando há texto) |

## Tecla física

| Ajuste | Onde | Por quê |
|---|---|---|
| `KeyDownEvent::physical_digit` | gpui-pre `interactive.rs` (e os construtores), gpui-pre-linux `platform.rs`/`wayland/client.rs`/`x11/client.rs`, gpui-pre-windows `events.rs`, gpui-pre-macos `events.rs`/`window.rs`, testes do gpui-base e do gpui-component | com Shift a tecla de número chega como símbolo, e só a tabela de layouts conhecidos o traduzia de volta; a posição física vale em qualquer layout (Ctrl+Shift+número escolhe a sessão) |

## Acessibilidade

| Ajuste | Onde | Por quê |
|---|---|---|
| View em cache reemite os nós de acessibilidade (`A11yCapture`, `A11y::replay`, quadro anterior em `prev_*`) | gpui-pre `view.rs`/`window/a11y.rs` | o reaproveitamento pulava o prepaint dos filhos e os nós sumiam; a árvore lida pelo AT-SPI oscilava entre poucos e todos os controles |
| `author_id` padrão = caminho dos ids nomeados do elemento, separados por `›` (sem ids de view, números, nomes de tipo e local no código; número de entidade vira o nome acessível) | gpui-pre `element.rs` (`author_path`) | id estável por controle para automação (`AccessibleId` no AT-SPI, `AutomationId` no UIA) sem marcar elemento por elemento |
| Texto visível vira folha `Label` (`Window::a11y_text`, chamado pelos elementos de texto, pelo parágrafo do markdown e pela grade do terminal), com id pelo pai e pela posição | gpui-pre `elements/text.rs`/`window/a11y.rs`, gpui-base `text/inline_flow.rs` | texto sem id não entrava na árvore: rótulos, explicações e valores não chegavam ao leitor nem à voz |
| Controle sem rótulo ganha o texto de dentro como nome; área clicável sem papel vira botão (grupo, se embrulha controles) | gpui-pre `window/a11y.rs` (`A11yNodeBuilder::pop`), `elements/div.rs` (`a11y_role`) | itens de lista, linhas clicáveis e disparadores saíam sem nome ou nem apareciam |
| Desenho adiado (diálogo, popover, menu) volta para o nó que o adiou e é reemitido quando reaproveitado | gpui-pre `window.rs` (`DeferredDraw::a11y_parent`/`a11y`, `prepaint_deferred_draws`), `window/a11y.rs` (`reparent`) | o conteúdo ficava solto na raiz e sumia quando a camada era reaproveitada |
| `Window::a11y_snapshot(root)` e `Window::retain_a11y_tree` | gpui-pre `window/a11y.rs`, `window/a11y/snapshot.rs` | o app lê a tela do último quadro em texto, sem leitor de tela ligado |
| Clique pela acessibilidade só quando o elemento do nó está sob o ponto | gpui-pre `window.rs` (`handle_a11y_action`), `elements/div.rs` | o clique é por coordenada; coberto por popup ele acionava o controle de cima |
| Nome do `Button` cai na dica; `Button::aria_selected`; botão escolhido (`selected`) anuncia selecionado | gpui-component `button/button.rs` | botão só de ícone saía sem nome; aba, modo e opção escolhidos precisam do estado |

## Tabela do markdown

| Ajuste | Onde | Por quê |
|---|---|---|
| Coluna encolhe até a maior palavra (teto 160 px) e barra horizontal sempre visível quando a tabela rola | gpui-base `text/node.rs` (`measure_table_column_widths`, `render_table`), `scrollable_mask.rs` (`horizontal_scroll_area`) | o piso do kit (metade do conteúdo, 160 a 480 px) fazia tabela de prosa de 4 colunas passar da coluna do chat, e a rolagem não tinha barra |

## Rolagem da conversa

| Ajuste | Onde | Por quê |
|---|---|---|
| Blocos fora da tela não são montados em documento sem rolagem própria | gpui-base `text/block_column.rs`, `text/document.rs`, `text/state.rs` | a linha da conversa guarda a mensagem inteira: cada quadro montava, posicionava e pintava até o que estava fora da tela |
