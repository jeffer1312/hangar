# Integrações MCP no Hangar (25/11/2026)

Decisões de arquitetura e domínio para o hub de **Integrações MCP** do `hangar-server`, que permite conectar servidores MCP externos (remotos e locais) uma única vez no Hangar e expor suas ferramentas a todos os agentes autorizados.

## Separação de domínio: Integrações vs Conectores

- **Contexto:** o Hangar já possui `crates/hangar-connectors` (`docs/connectors/README.md`, rotas `/api/connectors` e CLI `hangar-server connectors`), dedicado a programas locais de manifesto para o painel de Missão (`work_tracking/v1`, `work_actions/v1`, `work_review/v1`). Usar o mesmo nome para servidores MCP externos causaria ambiguidade no código, nas rotas e na interface.
- **Alternativas consideradas:**
  1. Reaproveitar o nome "Conectores" na tela separando visualmente "Conectores de trabalho" e "Conectores MCP".
  2. Unificar ambos numa tela única de "Conectores" com dois tipos internos.
  3. Separar explicitamente os conceitos: **Integrações** na interface (`integrations` / `mcp_hub` no código) e manter **Conectores** para o fluxo de tickets/revisão do painel de Missão.
- **Decisão:** adotada a alternativa 3. Na interface o novo recurso chama-se **Integrações**; no código usa `integrations` / `mcp_hub`. Os **Conectores** existentes permanecem intactos em `crates/hangar-connectors` e `/api/connectors`.
- **Consequências:** sem quebra de compatibilidade em rotas, CLI, documentação ou traduções existentes de `connectors`; vocabulário inequívoco entre tickets da Missão e ferramentas MCP compartilhadas entre agentes.

## Hub único no `/mcp/` com descoberta sob demanda e fixação opcional

- **Contexto:** o Hangar já registra um único servidor MCP (`/mcp/`, `crates/hangar-server/src/mcp/mod.rs`) em todas as contas de Claude, Codex e Antigravity via `hangar-server mcp-register`. Conectar vários serviços (Gmail, Drive, Agenda, Notion, Linear, Slack, Figma, MCPs locais) pode ultrapassar 150 ferramentas.
- **Alternativas consideradas:**
  1. Injetar todas as ferramentas de todas as integrações diretamente no catálogo de `list_tools` do `/mcp/`.
  2. Registrar cada integração como um servidor MCP separado na configuração de cada agente via `mcp-register`.
  3. Centralizar no `/mcp/` do Hangar usando **descoberta sob demanda** (poucas ferramentas fixas para listar integrações, inspecionar ferramentas de uma integração e executar uma ferramenta), com opção de marcar integrações favoritas como **fixadas** no catálogo direto.
- **Decisão:** adotada a alternativa 3, cobrindo apenas `tools` na v1 (`resources` e `prompts` ficam fora da v1) e aceitando múltiplas instâncias/contas do mesmo serviço por `id` único (ex.: `gmail-pessoal`, `gmail-trabalho`), tanto remotas (HTTP + OAuth/Token) quanto locais (`stdio` sob demanda com encerramento por inatividade).
- **Consequências:** funciona de forma homogênea em Claude, Codex e Antigravity sem depender de recarga dinâmica de catálogo (`notifications/tools/list_changed`) em cada harness; mantém baixo o custo de contexto quando o usuário não usa uma integração; centraliza auditoria, permissão e filtragem por projeto em um único ponto. Por outro lado, o uso de uma integração não fixada exige uma chamada inicial de descoberta pelo agente.

## Segurança fail-closed, armazenamento cifrado isolado e proteção de conteúdo externo

- **Contexto:** integrações dão acesso a dados pessoais e corporativos sensíveis (e-mails, documentos, tarefas, bancos locais) e a ações irreversíveis (enviar e-mail, apagar arquivos), inclusive em sessões rodando em modo sem confirmação do agente e na assistente de voz.
- **Alternativas consideradas:**
  1. Confiar na permissão nativa de cada agente e repassar chamadas e respostas sem filtro.
  2. Inferir leitura vs escrita por heurística de nome (`get_`, `list_`, `search_`).
  3. Tratar `readOnlyHint != true` como escrita por segurança com cartão de aprovação no Hangar, isolar segredos em armazenamento cifrado próprio e aplicar proteção de conteúdo externo.
- **Decisão:** adotada a alternativa 3:
  - **Política por integração:** padrão **Leitura livre + aprovação na escrita** (ajustável para **Tudo liberado** ou **Só leitura**). Apenas ferramentas anotadas pelo servidor MCP com `readOnlyHint: true` passam como leitura; ferramentas sem anotação ou de escrita abrem cartão de aprovação no Hangar (mesmo molde do Cofre), com opção **"Sempre liberar esta ferramenta"** naquela integração.
  - **Alcance:** vale para todas as sessões do dono nesta máquina (respeitando **Filtro por projeto** opcional) e para a **Assistente de voz** (leitura livre, escrita com confirmação). Sessões com convidado ou par de fora ficam bloqueadas por padrão (mesma regra do WhatsApp).
  - **Segredos:** armazenados com o mesmo mecanismo cifrado do Cofre (`AES-256-GCM` com chave no chaveiro do SO), inicializado automaticamente mesmo sem Cofre manual ativo, mas **isolado das entradas manuais** — tokens OAuth, chaves Bearer e `Client Secret` nunca aparecem em `vault_list` nem são legíveis pelas ferramentas `vault_*`.
  - **Conteúdo externo:** respostas vindas de servidores MCP externos recebem corte de tamanho (ex.: 64 KiB) e aviso explícito de que o texto é de terceiros e não deve ter instruções obedecidas (defesa contra prompt injection, alinhada ao WhatsApp).
- **Consequências:** servidores locais/antigos sem `readOnlyHint` não conseguem executar escrita silenciosa; tokens não vazam para os agentes; e-mails e documentos externos não inflam o contexto nem ganham autoridade de instrução. Na primeira chamada de uma ferramenta de leitura sem `readOnlyHint`, o dono precisa clicar uma vez em "Sempre liberar esta ferramenta".

## OAuth 100% local (sem relay em nuvem)

- **Contexto:** o Hangar é single-user e roda na máquina do dono (acessado por localhost no Desktop nativo e por LAN/Tailscale no Mobile Expo). Provedores como o Google exigem `Client ID` pré-cadastrado e restringem `redirect_uri` (aceitando `127.0.0.1` ou HTTPS válido, recusando IP de LAN HTTP).
- **Alternativas consideradas:**
  1. Login OAuth exclusivo no Desktop, sem fluxo de conexão no Mobile Expo.
  2. Relay/proxy de callback OAuth hospedado em `hangar.dev.br`.
  3. Callback OAuth sempre direto no `hangar-server` local: `http://127.0.0.1:<porta>/api/integrations/oauth/callback` no Desktop nativo; no Mobile Expo usa a URL HTTPS/Tailscale do servidor quando configurada e oferece concluir no navegador do desktop quando o provedor exigir `127.0.0.1` ou não houver HTTPS.
- **Decisão:** adotada a alternativa 3, suportando tanto OAuth 2.1 com registro automático (DCR/CIMD) para MCPs remotos abertos quanto `Client ID`/`Client Secret` manuais (caso do Google MCP oficial, com passo a passo curto para conta `@gmail.com` pessoal e Workspace) e erro explícito com botão **Reconectar** quando o token expirar (ex.: expiração de 7 dias em apps Google em modo Testing).
- **Consequências:** preserva a arquitetura 100% local do Hangar sem dependência de servidor intermediário de terceiros nem trânsito de códigos OAuth por nuvem; mantém a paridade funcional entre Desktop nativo e Mobile Expo dentro dos limites impostos pelos provedores OAuth.
