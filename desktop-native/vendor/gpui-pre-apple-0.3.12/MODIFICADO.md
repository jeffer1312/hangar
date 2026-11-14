# Cópia do gpui-pre-apple 0.3.12

Origem: crate `gpui-pre-apple` 0.3.12 do crates.io, checksum `a460605ec4e8cbcc9b213462a84b1d6cc3efe4ed8f0e18669659f07b2d5c6aec`, licença Apache-2.0.

Na 0.3.11 o renderer trocou `metal`/`objc` por `objc2-metal`: o desfoque foi reescrito para essa API, com o kernel do MPS
num `Retained`. Só passou por checagem de tipos para `aarch64-apple-darwin`, com o shader trocado por um arquivo vazio;
o `.metal` e o desfoque nunca rodaram num Mac.

Desfoque de fundo portado de `zeronsh/zui` na revisão `18a89af`. Arquivos de origem alterados:

- `src/metal_renderer.rs`: intercala o desfoque na ordem da cena, copia a região, aplica `MPSImageGaussianBlur` e recompõe no Metal.
- `src/shaders.metal`: desenha a região desfocada com recorte e cantos arredondados.
- `build.rs`: exporta os tipos para o cabeçalho do shader.
- `vendor/gpui/src/scene.rs`: espelha `BackdropBlur` para o `cbindgen` do crate.

`NOTICE` preserva o aviso de atribuição do zui.
