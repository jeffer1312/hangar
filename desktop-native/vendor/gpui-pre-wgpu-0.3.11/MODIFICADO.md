# Cópia do gpui-pre-wgpu 0.3.11

Origem: crate `gpui-pre-wgpu` 0.3.11 do crates.io, checksum `211cf352de1a0b0c9bea3f105df5572ce1d2c899389b924a888e5af36b50f93d`, licença Apache-2.0 (`LICENSE-APACHE`).
Entra no build por `[patch.crates-io]` em `desktop-native/Cargo.toml`. Arquivos alterados: `src/wgpu_renderer.rs`,
`src/shaders.wgsl`, `src/gpui_wgpu.rs`, `src/wgpu_context.rs` e o novo `src/blur_kernel.rs`.

O desfoque de fundo foi portado do zui `18a89af`: kernel, WGSL e renderer wgpu. O renderer intercala as passadas pela ordem da cena, usa `COPY_SRC` na superfície quando disponível ou um alvo copiável intermediário, reutiliza o scratch e o libera após 30 quadros sem desfoque.

O redesenho parcial foi portado do PR #62455 de zed-industries/zed (`d9c29a3`, Apache-2.0): `draw_with_damage` desenha
numa textura que persiste entre quadros, redesenha só as regiões que mudaram (cada draw sob o scissor da região) e copia
a textura para a imagem da swapchain, que precisa aceitar `COPY_DST`; sem isso, volta ao desenho inteiro. Diferente do
Zed, a região que toca o que um desfoque lê cresce até a área inteira dele (incluindo o alcance das amostras das
passadas reduzidas), porque os pixels guardados ali já estão compostos, e desfoque fora das regiões não é refeito.
`GPUI_EXPERIMENTAL_PARTIAL_RENDER=0` (lido no `gpui-pre`) desliga.

Na 0.3.7 o renderer foi dividido em `WgpuRenderer` (superfície) e `WgpuRendererCore` (dispositivo, usado também pelo
renderer headless de teste). O desfoque, o scratch e a textura persistente moram no core e somem com ele; as dimensões
vêm da textura do quadro, não da configuração da superfície. O dano pendente e as flags `COPY_SRC`/`COPY_DST` da
superfície ficam no `WgpuRenderer`.

Navegador embutido (Linux): o WPE WebKit entrega quadros em DMA-BUF, importados sem cópia como `wgpu::Texture` no
dispositivo do próprio GPUI. Para isso `wgpu_context.rs` cria o dispositivo Vulkan com
`VK_EXT_image_drm_format_modifier` quando o adaptador suporta (`as_hal` + `open_with_callback` +
`create_device_from_hal`; senão, `request_device` como antes) e guarda uma cópia do par em
`WgpuContext::shared_device()`, trocada a cada contexto novo (recuperação de GPU inclusive); quando cai no
`request_device` por adaptador sem Vulkan ou sem a extensão, o motivo sai no log. O renderer desenha
`PrimitiveBatch::Surfaces` como sprite policromático no pipeline `poly_sprites` (`draw_surfaces`), com a textura da
superfície no lugar do atlas. Opacidade negativa marca a textura como opaca e o `fs_poly_sprite` força alpha 1, porque
em XRGB/XR24 o byte X não é alpha. Fora do Linux, `Surfaces` continua sem desenho.

Textura `Rgba8Unorm` é a exceção: vem de quadro decodificado (o screencast do Chromium) com alpha de verdade, não
pré-multiplicado, e vai com opacidade 1; o `blend_color` multiplica o rgb pelo alpha quando o alvo é pré-multiplicado.
A página da conversa usa isso para o fundo transparente; o JPEG do painel tem alpha 255 e sai igual. O DMA-BUF do WPE
entrava como `Bgra8Unorm` e seguiria opaco.
