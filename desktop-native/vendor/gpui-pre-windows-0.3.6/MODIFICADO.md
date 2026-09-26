# Cópia do gpui-pre-windows 0.3.6

Origem: crate `gpui-pre-windows` 0.3.6 do crates.io, checksum `27662e65bfcf4445d2cccfad91f1570ec07f4b1b5de98400a9f207fd19c464cf`, licença Apache-2.0.

Desfoque de fundo portado de `zeronsh/zui` na revisão `18a89af`. Arquivos alterados:

- `build.rs`: inclui os shaders de desfoque no build Windows de release.
- `src/directx_renderer.rs`: intercala o desfoque pela ordem da cena, registra seus shaders e libera recursos ociosos ou após resize.
- `src/shaders.hlsl`: adiciona as passadas de desfoque e composição.
- `src/directx_renderer/backdrop.rs`: prepara texturas, shaders e buffers DirectX sob demanda, desenha e recompõe a região desfocada.

O desfoque usa o registrador b2 porque b1 pertence a `BatchParams` nesta versão; ao restaurar o estado do DirectX, preserva b1.
`NOTICE` preserva o aviso de atribuição do zui.

Não conferido em Windows.
