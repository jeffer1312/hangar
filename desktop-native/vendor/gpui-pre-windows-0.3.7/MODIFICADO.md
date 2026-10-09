# Cópia do gpui-pre-windows 0.3.7

Origem: crate `gpui-pre-windows` 0.3.7 do crates.io, checksum `f05592a6f9e3e6bb7a9f2a0d4779271cf747a948db1c07b02448de777ffff8f3`, licença Apache-2.0.

Desfoque de fundo portado de `zeronsh/zui` na revisão `18a89af`. Arquivos alterados:

- `build.rs`: inclui os shaders de desfoque no build Windows de release.
- `src/directx_renderer.rs`: intercala o desfoque pela ordem da cena, registra seus shaders e libera recursos ociosos ou após resize.
- `src/shaders.hlsl`: adiciona as passadas de desfoque e composição.
- `src/directx_renderer/backdrop.rs`: prepara texturas, shaders e buffers DirectX sob demanda, desenha e recompõe a região desfocada.

O desfoque usa o registrador b2 porque b1 pertence a `BatchParams` nesta versão; ao restaurar o estado do DirectX, preserva b1.
`NOTICE` preserva o aviso de atribuição do zui.

Navegador embutido: `src/directx_renderer.rs` cria o alvo do DirectComposition com `CreateTargetForHwnd(hwnd, false)`
(topmost=false). Com `true` a composição fica por cima das janelas filhas e esconde o WebView2 embutido; com `false` a
página aparece e o vidro da janela continua funcionando (provado numa VM Windows). O custo é que o GPUI não desenha
por cima de janelas filhas.

Compilada e conferida em Windows (VM sem GPU, D3D11 no WARP, via RDP).

Redesenho parcial (`src/directx_renderer.rs`, `src/window.rs`, `src/directx_renderer/backdrop.rs`):

- O quadro é desenhado numa textura persistente (`render_target` passou a ser ela) e copiado para o back buffer, que
  fica num campo próprio. Sem dano visível desde o último present, nada é copiado nem apresentado.
- `draw_with_damage` acumula o dano pendente; o plano junta tudo num só retângulo de tesoura, crescido até cobrir a
  área que cada desfoque de fundo amostra (`blur_footprint`, mesma folga do wgpu). Acima de 70% da janela, ou com a
  textura inválida (criação, resize, device lost, erro), o quadro é inteiro.
- Só a região é limpa (`ClearView`, quando o dispositivo declara suporte; sem ele, todo quadro é inteiro), inclusive
  a textura MSAA dos caminhos. Desfoque fora da região é pulado. A tesoura vale só dentro do `render`, porque o
  rasterizador de emoji colorido usa o mesmo contexto.
- `Present1` leva a região como retângulo sujo, o que deixa o DWM e o RDP mandarem só ela.
- `GPUI_DX_PARTIAL_RENDER=0` força o quadro inteiro, para comparar.

Em 0.3.7 `DirectXAtlas::get_texture_view` passou a devolver `Option`; o desfoque não usa o atlas, então nada mudou aqui.

Bandeja: `PlatformWindow::set_hidden` em `src/window.rs`, com `ShowWindowAsync` (`SW_HIDE`/`SW_SHOW`).

Tecla física: `physical_digit(lparam)` (`src/events.rs`) mapeia os códigos de varredura 0x02 a 0x0B (sem o bit de
tecla estendida) para `KeyDownEvent::physical_digit`.
