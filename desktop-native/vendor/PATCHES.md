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

## Estado na atualização 0.6.6 → 0.7.0 (gpui-pre 0.3.6 → 0.3.7)

| Ajuste | Onde | Estado | Por quê |
|---|---|---|---|
| Desfoque de fundo (vidro) | gpui-pre `scene.rs`/`window.rs`, gpui-pre-wgpu, -apple, -windows | adaptado no wgpu, mantido nos demais | a 0.3.7 dividiu o renderer wgpu em superfície (`WgpuRenderer`) e dispositivo (`WgpuRendererCore`); buffers, scratch e o alvo copiável foram para o núcleo |
| Redesenho parcial e pular quadro igual | gpui-pre `scene_damage.rs`/`window.rs`/`platform.rs`, gpui-pre-linux, gpui-pre-wgpu, gpui-pre-windows `directx_renderer.rs`/`backdrop.rs`/`window.rs` | adaptado no wgpu e no windows, mantido nos demais | mesma divisão; `render_frame` recebe textura de destino e regiões. No DirectX 11: textura de quadro persistente copiada para o back buffer, um só retângulo de tesoura e `Present1` com esse retângulo sujo (`GPUI_DX_PARTIAL_RENDER=0` desliga). O upstream não tem nada equivalente |
| Mapa de elementos das provas, `text_input_focused`, `LineCap` | gpui-pre `window.rs`/`element.rs`/`path_builder.rs` | mantido | upstream não mexeu nesses trechos; duas funções nossas em `window.rs` estavam entre a doc e o `#[inline(always)]` de outra e foram movidas |
| Seleção de texto em view guardada (`retain_cached_view`) | gpui-base `text_selection.rs` | descartado | a 0.7.0 mantém o participante de uma view guardada registrado (`with_rendered_element`, `text_selection.rs:206-245`); o app deixou de chamar o remendo |
| Coluna do marcador de lista, cor do marcador | gpui-base `text/node.rs`/`style.rs` | adaptado | o marcador passou a usar `list_start`, então a lista numerada que começa em outro número (#3204) vale com a coluna ligada |
| Faixa de linguagem do bloco de código | gpui-base `text/node.rs`/`style.rs` | adaptado | o código dentro da faixa usa a estrutura nova do kit (`leaf_key`, `range_backgrounds`, `reveal`) |
| Fonte própria do código inline | gpui-base `text/*`, gpui-component `text/style.rs`/`compat.rs` | mantido | `compat.rs` ficou com o `with_heading` da 0.7.0 mais o repasse da fonte |
| `PopupSurface` (vidro nas listas e menus do kit) | gpui-component `popover.rs`, `menu/popup_menu.rs` | mantido | `dropdown_popup` e `popup_menu.rs` não mudaram na 0.7.0 |
| `PopupMenuAppearance`, `replace_item` | gpui-component `menu/popup_menu.rs`/`mod.rs` | mantido | idem; os consertos de vazamento (#3267, #3279) ficaram em outros arquivos e os acréscimos não guardam entidade |
| Entrada `menu-in` do menu de contexto | gpui-component `menu/context_menu.rs` | adaptado | a 0.7.0 só monta o menu no desenho (`DeferredMenu::build_menu`); a animação foi para lá |
| Fundo pintado e entrada `dialog-in` do diálogo | gpui-component `dialog/dialog.rs` | adaptado | o cartão agora fica no `Positioner::corner`; a subida de 2 px substitui a descida do `slide-down` |

Não compilados aqui: gpui-pre-apple/-macos (revisados só por leitura).

## Navegador embutido

| Ajuste | Onde | Por quê |
|---|---|---|
| `PaintSurface::texture` e `Window::paint_surface(bounds, texture)` no Linux | gpui-pre `scene.rs`/`window.rs` | a página do WPE WebKit chega como textura GPU externa; o upstream só pinta superfície no macOS (`CVPixelBuffer`) |
| Superfície sempre conta como dano | gpui-pre `scene_damage.rs` (já vinha do PR #62455; teste novo) | a primitiva fica idêntica enquanto os pixels da página mudam; sem isso o quadro seria pulado como igual |
| Dispositivo Vulkan com `VK_EXT_image_drm_format_modifier` e `WgpuContext::shared_device()` | gpui-pre-wgpu `wgpu_context.rs` | importar o DMA-BUF sem cópia exige o modificador DRM e o mesmo dispositivo que o GPUI usa para desenhar |
| `PrimitiveBatch::Surfaces` desenhado como sprite policromático, alpha forçado a 1 | gpui-pre-wgpu `wgpu_renderer.rs`/`shaders.wgsl` | o renderer wgpu ignorava superfícies; em XRGB o byte X não é alpha |
| `CreateTargetForHwnd(hwnd, false)` | gpui-pre-windows `directx_renderer.rs` | com topmost a composição cobre a janela filha do WebView2; o GPUI deixa de desenhar por cima dela |

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
| `Slider::aria_label` | gpui-base e gpui-component `slider.rs` | slider não tem texto próprio para ser nomeado |
