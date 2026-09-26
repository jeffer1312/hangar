# Cópia do gpui-pre-windows 0.3.6

Origem: crate `gpui-pre-windows` 0.3.6 do crates.io, checksum `27662e65bfcf4445d2cccfad91f1570ec07f4b1b5de98400a9f207fd19c464cf`, licença Apache-2.0.

Primeira parte do desfoque de fundo portada de `zeronsh/zui` na revisão `18a89af`. Arquivos alterados:

- `build.rs`: inclui os shaders de desfoque no build Windows de release.
- `src/directx_renderer.rs`: reserva o estado do desfoque e registra seus shaders.
- `src/shaders.hlsl`: adiciona as passadas de desfoque e composição.
- `src/directx_renderer/backdrop.rs`: prepara texturas, shaders e buffers DirectX sob demanda.

O desfoque usa o registrador b2 porque b1 pertence a `BatchParams` nesta versão. A Task 80 deve preservar b1 ao restaurar o estado do DirectX.
`NOTICE` preserva o aviso de atribuição do zui.

O desenho das passadas e a intercalação por ordem da cena ficam para a Task 80. Até lá, o módulo entra no build do Windows, mas o desfoque segue sem uso. Não conferido em Windows.
