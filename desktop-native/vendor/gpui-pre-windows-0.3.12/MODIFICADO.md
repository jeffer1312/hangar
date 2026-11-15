# Cópia do gpui-pre-windows 0.3.12

Origem: crate `gpui-pre-windows` 0.3.12 do crates.io, checksum `7201762420222d5b5ce5bdf0056996199212a3349bfb2457a3a0238c688681f3`, licença Apache-2.0.

Desfoque de fundo portado de `zeronsh/zui` na revisão `18a89af`. Arquivos alterados:

- `build.rs`: inclui os shaders de desfoque no build Windows de release.
- `src/directx_renderer.rs`: intercala o desfoque pela ordem da cena, registra seus shaders e libera recursos ociosos ou após resize.
- `src/shaders.hlsl`: adiciona as passadas de desfoque e composição.
- `src/directx_renderer/backdrop.rs`: prepara texturas, shaders e buffers DirectX sob demanda, desenha e recompõe a região desfocada.

O desfoque usa o registrador b2 porque b1 pertence a `BatchParams` nesta versão; ao restaurar o estado do DirectX, preserva b1.
`NOTICE` preserva o aviso de atribuição do zui.

Navegador embutido: o alvo do DirectComposition com `CreateTargetForHwnd(hwnd, false)` (topmost=false) deixou de ser
ajuste nosso: o upstream passou a criá-lo assim na 0.3.12. O WebView2 embutido depende disso: com `true` a composição
fica por cima das janelas filhas e o esconde.

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

Superfície externa (página da conversa): `SurfaceTexture` (`src/directx_devices.rs`) é uma textura RGBA no device em que
o GPUI desenha, criada de qualquer thread (`new`) e escrita na thread da janela (`write`, pelo contexto imediato).
`DirectXDevices::new` guarda o device atual para isso, trocado na perda do device; `is_current` avisa o dono para criar
outra. `draw_surfaces` (`src/directx_renderer.rs`) desenha cada `PaintSurface` como um sprite policromático cuja peça é a
textura inteira, num pipeline próprio (`surface_sprites`), e pula a textura de um device perdido.
