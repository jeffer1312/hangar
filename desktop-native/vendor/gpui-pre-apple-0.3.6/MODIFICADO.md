# Cópia do gpui-pre-apple 0.3.6

Origem: crate `gpui-pre-apple` 0.3.6 do crates.io, checksum `3af534f0746eb9e014114bcc0ee00dc250c4b8d75df2e437180c403aa65649c1`, licença Apache-2.0.

Desfoque de fundo portado de `zeronsh/zui` na revisão `18a89af`. Arquivos de origem alterados:

- `src/metal_renderer.rs`: intercala o desfoque na ordem da cena, copia a região, aplica `MPSImageGaussianBlur` e recompõe no Metal.
- `src/shaders.metal`: desenha a região desfocada com recorte e cantos arredondados.
- `build.rs`: exporta os tipos para o cabeçalho do shader.
- `vendor/gpui/src/scene.rs`: espelha `BackdropBlur` para o `cbindgen` do crate.

`NOTICE` preserva o aviso de atribuição do zui.
