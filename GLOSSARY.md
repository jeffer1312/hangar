# Glossário

Termos de domínio do Hangar. Quando um nome já existe com outro sentido no código, cada contexto fica separado explicitamente para evitar colisão entre interface, rotas e crates.

## Conector (de trabalho / Missão)

- **Definição:** programa local cadastrado por manifesto (`manifest.json`) que o `hangar-server` executa por JSON-RPC em `stdin`/`stdout` para alimentar o painel de Missão com tickets, ações e revisões de código.
- **Exemplos:** conector de Jira/Help/Linear que lista tickets (`work_tracking/v1`), ações (`work_actions/v1`) e revisões (`work_review/v1`) na tela do Hangar.
- **Limites:** não expõe ferramentas MCP para os agentes, não carrega segredos no manifesto e não gerencia login OAuth no Hangar.
- **Referências:** [crates/hangar-connectors/src/manifest.rs](crates/hangar-connectors/src/manifest.rs), [docs/connectors/README.md](docs/connectors/README.md).

## Integração (Integração MCP)

- **Definição:** conexão a um servidor MCP externo — remoto (HTTP com OAuth ou token) ou local (`stdio` / comando na máquina, iniciado sob demanda) — cadastrada uma vez no `hangar-server` e compartilhada com todos os agentes que consomem o `/mcp/` do Hangar (v1: Claude, Codex e Antigravity). Na interface aparece como **Integrações**; no código usa `integrations` / `mcp_hub`. Cada cadastro tem um `id` único, permitindo múltiplas contas/instâncias do mesmo serviço (ex.: `gmail-pessoal` e `gmail-trabalho`).
- **Exemplos:** Gmail/Google Drive/Agenda via MCP remoto oficial do Google, Notion, Linear, Slack, Figma, Atlassian (Jira/Confluence), GitHub, Cloudflare, Sentry, Supabase ou um servidor MCP local executado via `npx` / binário.
- **Limites:** distinta de **Conector** (de trabalho/Missão). Na v1 expõe apenas ferramentas (`tools`), sem `resources` nem `prompts`. Segredos (tokens OAuth, chaves, `Client Secret`) ficam protegidos no **Armazenamento cifrado de integrações** e nunca são expostos aos agentes. Bloqueada por padrão em sessões com convidado ou par de fora.
- **Referências:** [crates/hangar-server/src/mcp/mod.rs](crates/hangar-server/src/mcp/mod.rs), [crates/hangar-registration/src/mcp_register.rs](crates/hangar-registration/src/mcp_register.rs), [crates/hangar-server/src/vault/store.rs](crates/hangar-server/src/vault/store.rs), [docs/decisoes/integracoes-mcp.md](docs/decisoes/integracoes-mcp.md).

## Descoberta sob demanda (e Integração fixada)

- **Definição:** modo padrão de exposição das **Integrações** no `/mcp/` do Hangar. Em vez de injetar todos os schemas de todas as integrações em toda conversa, o `/mcp/` expõe poucas ferramentas fixas (listar integrações, consultar ferramentas de uma integração e executar uma ferramenta). Uma **Integração fixada** é uma integração favorita que o dono marca para aparecer diretamente no catálogo do `/mcp/`.
- **Exemplos:** ter Gmail, Drive, Notion, Linear e Slack conectados no Hangar gastando apenas poucas ferramentas no contexto inicial, e deixar só o Linear fixado se for usado em quase toda sessão.
- **Limites:** evita sobrecarga de contexto nos agentes e não depende de recarga dinâmica de catálogo (`list_changed`) em todos os harnesses. Na v1 cobre apenas `tools`.
- **Referências:** [crates/hangar-server/src/mcp/mod.rs](crates/hangar-server/src/mcp/mod.rs), [docs/decisoes/integracoes-mcp.md](docs/decisoes/integracoes-mcp.md).

## Política de permissão da integração

- **Definição:** regra configurada por **Integração** que define quais chamadas dos agentes passam direto e quais exigem confirmação do dono no Hangar.
  - **Leitura livre + aprovação na escrita (padrão):** chamadas marcadas pelo servidor MCP com `readOnlyHint: true` passam direto; qualquer outra chamada (escrita explícita ou sem anotação `readOnlyHint`) abre cartão de aprovação no Hangar (no molde do Cofre), com opção **"Sempre liberar esta ferramenta"** naquela integração.
  - **Tudo liberado:** todas as chamadas da integração passam sem cartão do Hangar.
  - **Só leitura:** chamadas que não sejam de leitura confirmada são recusadas pelo Hangar.
- **Exemplos:** permitir que qualquer sessão sua busque e leia e-mails no Gmail sem interromper, abrir cartão no desktop/celular antes de enviar um e-mail ou apagar um arquivo no Drive, e liberar pontualmente uma ferramenta de um MCP local antigo que não envia `readOnlyHint`.
- **Limites:** aplica-se apenas a sessões do dono nesta máquina (respeitando eventual **Filtro por projeto**) e à **Assistente de voz** (leitura livre, escrita com confirmação); sessões com convidado ou par de fora permanecem bloqueadas por padrão, como ocorre no WhatsApp.
- **Referências:** [crates/hangar-server/src/vault/requests.rs](crates/hangar-server/src/vault/requests.rs), [crates/hangar-server/src/mcp/whatsapp.rs](crates/hangar-server/src/mcp/whatsapp.rs), [crates/hangar-server/src/mcp/identity.rs](crates/hangar-server/src/mcp/identity.rs).

## Filtro por projeto

- **Definição:** restrição opcional configurada em uma **Integração** para limitar em quais projetos (diretórios/workspaces) ela fica disponível para os agentes. Sem filtro, a integração vale para todas as sessões do dono nesta máquina.
- **Exemplos:** deixar o Gmail pessoal disponível em todas as sessões, mas restringir o MCP do banco ou Jira da empresa apenas às sessões abertas nos repositórios daquele trabalho.
- **Limites:** não libera acesso para sessões com convidado ou par de fora.
- **Referências:** [crates/hangar-projects/Cargo.toml](crates/hangar-projects/Cargo.toml).

## Armazenamento cifrado de integrações

- **Definição:** armazenamento local cifrado com `AES-256-GCM` usando chave no chaveiro do sistema operacional (mesmo mecanismo criptográfico do Cofre), inicializado automaticamente pelo Hangar para guardar tokens OAuth, chaves Bearer e `Client Secret` das integrações.
- **Exemplos:** guardar o `refresh_token` do Google ou o token OAuth do Linear sem exigir que o dono tenha ativado o Cofre manual antes.
- **Limites:** estritamente isolado das entradas manuais do Cofre — tokens e segredos de integrações nunca aparecem em `vault_list` nem podem ser lidos pelas ferramentas `vault_*` dos agentes.
- **Referências:** [crates/hangar-server/src/vault/store.rs](crates/hangar-server/src/vault/store.rs).

## Proteção de conteúdo externo

- **Definição:** camada aplicada pelo `hangar-server` nas respostas vindas de servidores MCP externos: limite de tamanho (corte configurado, ex.: 64 KiB) e marcação explícita avisando o agente de que o conteúdo veio de terceiros e não deve ter instruções obedecidas.
- **Exemplos:** impedir que um e-mail malicioso no Gmail ou um comentário em issue no GitHub dê ordens escondidas ao agente (prompt injection) ou despeje megabytes no contexto.
- **Limites:** segue a mesma linha de defesa já usada nas ferramentas de WhatsApp; não substitui a política de permissão de escrita.
- **Referências:** [crates/hangar-server/src/mcp/whatsapp.rs](crates/hangar-server/src/mcp/whatsapp.rs).
