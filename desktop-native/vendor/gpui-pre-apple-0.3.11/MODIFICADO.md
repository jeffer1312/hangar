# Cópia do gpui-pre-apple 0.3.11

Origem: crate `gpui-pre-apple` 0.3.11 do crates.io, checksum `8c0bea8ffa563d3a1ada533e148bb55be7fb873c178979e889a3d1204f8e1b91`, licença Apache-2.0.

Na 0.3.11 o renderer trocou `metal`/`objc` por `objc2-metal`: o desfoque foi reescrito para essa API, com o kernel do MPS
num `Retained`. Não compilado fora do macOS.

Desfoque de fundo portado de `zeronsh/zui` na revisão `18a89af`. Arquivos de origem alterados:

- `src/metal_renderer.rs`: intercala o desfoque na ordem da cena, copia a região, aplica `MPSImageGaussianBlur` e recompõe no Metal.
- `src/shaders.metal`: desenha a região desfocada com recorte e cantos arredondados.
- `build.rs`: exporta os tipos para o cabeçalho do shader.
- `vendor/gpui/src/scene.rs`: espelha `BackdropBlur` para o `cbindgen` do crate.

`NOTICE` preserva o aviso de atribuição do zui.

Em 0.3.7 `MetalAtlas::metal_texture` passou a devolver `Option` e o handler de conclusão trocou `block` por `block2`;
o desfoque não usa nenhum dos dois, então nada mudou aqui. Não compilado fora do macOS.
