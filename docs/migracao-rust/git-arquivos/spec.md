# Git e arquivos no Rust

## Alcance

As capacidades existentes de Git, árvore da sessão, leitura/edição, busca, arquivos citados,
conteúdo servido e seletor de pastas passam ao Rust. A interface HTTP mantém rotas, nomes,
status e campos. Contas, adaptadores, custos, uploads e a coordenação de sessões continuam
nas partes responsáveis da migração.

## Arquitetura

- `hangar-workspace`: regras e operações tipadas, independente da interface gráfica e do HTTP.
- `hangar-api`: contratos compartilhados de conteúdo, entradas e erros.
- O desktop usa o núcleo nos caminhos locais existentes; servidor remoto continua pela API.
- O `hangar-server` atende diretamente o dono. A autorização dos convidados continua no Python,
  que pode delegar suas operações autorizadas pela ponte privada.
- `GET /internal/workspace/context` fornece raízes e identidade/cwd/transcript das sessões.
  Não transfere credenciais nem conteúdo dos arquivos ao Rust.
- A porta privada já anunciada em `terminal_address` recebe `POST /__hangar_server/workspace`.
  IP de loopback e segredo são conferidos antes do corpo; a porta pública sempre responde 404.
  Operações privadas recebem o contexto resolvido e não consultam novamente o registro.
- Protocolo interno 10, nos dois lados, após integrar a parte 2B (7); as partes 3 e 2D reservam 8 e 9.

## Invariantes

- Raiz real da sessão, autorização por citação e área interna do Git são políticas distintas.
- Texto limitado a 512 KiB, diretório a 1.000 entradas, busca a 200 resultados; digest dos bytes
  integra o salvamento. Arquivo alterado desde a leitura recebe 409 sem substituir o conteúdo.
- Git recebe argumentos, nunca comandos montados com dados do usuário. Prazos locais e de rede
  são separados, e descendentes acompanham a vida do executor, inclusive em uma queda abrupta.
- POSIX usa um monitor com stdin do executor e grupo de processos; Windows usa Job Object com
  encerramento ao fechar o handle e inicia o comando suspenso até vinculá-lo.
- Metadados, leituras e alterações têm vagas separadas. Cache de resumo é compartilhado por cwd,
  sem serializar repositórios independentes. Falha conserva o último resumo válido na listagem.
- Alteração possivelmente enviada nunca é repetida na reserva. O usuário recebe aviso de
  resultado sem confirmação. A implementação Python fica disponível durante a migração.
- Arquivos ativos mantêm isolamento, Range, cache condicionado à autorização e ausência do token
  no documento interno do HTML.

## Correções necessárias para preservar as capacidades

Nomes retornados pelo Git são literais, incluindo acentos e escapes; limites de diff usam
caracteres. A árvore conserva separadores físicos do sistema e usa a forma do Git só nas marcas.
O modal de arquivo no PWA passa o rascunho e o estado de salvamento ao visor, como o outro
hospedeiro já fazia; sem essa ligação, o salvamento existente não podia ser acionado.
