# Múltiplas contas Codex no Hangar

Estudo em 09/09/2026, Codex CLI 0.153.4. Implementação ainda não realizada.

## Comportamento pedido

Cadastrar mais de uma conta ChatGPT para o Codex, herdar as configurações da instalação Codex padrão e selecionar a conta ao abrir uma sessão. A referência é a experiência das contas Claude; o mecanismo de sincronização precisa ser próprio do Codex.

A fonte é o Codex padrão, inclusive configurações e plugins exclusivos dele. Rodar novamente a importação Claude → Codex em cada conta não atende esse requisito.

## Resultado do estudo

Usar um `CODEX_HOME` por conta e iniciar o app-server da sessão nesse diretório. Login, logout e consulta da conta podem usar o protocolo nativo do Codex. O Hangar mantém os nomes das contas e a política de herança; o Codex mantém a autenticação.

O schema gerado pelo CLI instalado oferece `account/login/start` com `chatgpt` e `chatgptDeviceCode`. O segundo permite mostrar endereço e código no celular, sem depender de callback de navegador no computador. A conclusão deve ser confirmada por `account/login/completed` e `account/read`, não pela mera abertura do navegador.

`--profile` seleciona uma camada de configuração, não uma conta autenticada independente.

## O que herdar

| Conteúdo | Tratamento |
|---|---|
| Modelo, esforço, preferências, definições de MCP e agentes | Mesclar a partir da conta padrão antes de abrir; mudanças na secundária não escrevem na padrão |
| Instruções, skills e scripts de hooks | Materializar recursos gerenciados e conferir referências; não ligar a pasta inteira |
| Definições dos hooks | Herdar conteúdo; preservar a aprovação própria da conta |
| Plugins e marketplaces | Reproduzir seleção e origens usando o gerenciador nativo; confirmar instalação |
| `auth.json` | Exclusivo da conta, com armazenamento `file` |
| Conversas, SQLite, índices, locks, IPC, caches e estado dos plugins | Exclusivos da conta |
| `hooks.state`, confiança de projetos e regras de autorização persistidas | Não copiar como se fossem preferências |

O `config.toml` real contém aprovações em `hooks.state`, com identificadores que incluem caminhos absolutos. Portanto uma cópia integral do arquivo não é a herança desejada. Uma sincronização deve preservar o estado local da conta e alterar apenas o conteúdo gerenciado.

Plugins também têm estado em `plugins/data/`. O campo `plugins.<id>.enabled` não demonstra instalação. Nesta máquina há ainda um marketplace nativo apontando para `.tmp/bundled-marketplaces` do padrão: esse caminho temporário não pode virar dependência permanente da segunda conta. A preparação deve distinguir catálogos nativos de catálogos adicionados pelo usuário.

## Fluxo proposto

1. Em Contas e modelos, cadastrar uma conta Codex com nome e iniciar login nativo no diretório dela. Uma conta sem login confirmado permanece identificada como desconectada.
2. Preparar a herança das configurações do Codex padrão. Recursos inválidos ou falhas de instalação aparecem como pendência; não apresentar sincronização completa nessas condições.
3. Em Nova sessão, oferecer as contas Codex no web e no app nativo. Levar a escolha pela API até o lançador e seu app-server.
4. Registrar a conta e seu diretório junto da conversa. Fechar, renomear, trocar modelo, reiniciar o backend e retomar não podem apagar essa identidade.
5. No Arquivo e na identificação das sessões, consultar as contas cadastradas e mostrar a conta real. Uma conversa arquivada conserva a origem, mesmo após o sidecar da sessão viva ser removido.

O escopo não inclui troca automática por cota nem migração de uma conversa em andamento para outra conta.

## Pontos do Hangar que precisam mudar

- `backend/app/contas.py`: referência do comportamento Claude, não política a copiar.
- `backend/app/oauth_codex.py`: o login atual grava cofre global e propaga para Codex, Pi e OMP; não pode receber a segunda conta sem separar o destino. Preferir o login nativo para o novo fluxo.
- `backend/app/api.py`, `registry.py` e `scripts/hangar-codex-tui`: transportar a conta até o processo. O lançador já lê `CODEX_HOME`, mas a criação pelo app não transporta essa escolha.
- `backend/app/adapters/codex/sessions.py`: preservar identidade da conta nas escritas, renomeações e alterações de modelo; o pretrust atual também usa o diretório padrão fixo.
- `backend/app/archive_providers.py` e identificação de conta em `registry.py`: deixar de consultar uma única conta global.
- `backend/app/codex_integracao.py`: a reconciliação atual parte do Claude; não substitui a herança Codex padrão → contas. O pedido HTTP de integração do lançador hoje também não informa a conta.
- `packages/core/src/api.ts` e tipos correspondentes: contrato compartilhado de contas e criação.
- `frontend/src/components/CreateSessionSheet.svelte`, `mobile/src/features/create/CreateSessionSheet.tsx` e telas de Contas e modelos: cadastro, login, seleção e apresentação das pendências nas duas interfaces.

Existem mecanismos de escrita atômica, trava e interface com o gerenciador nativo que podem servir à implementação. Isso não implica compartilhar a política específica de contas Claude.

## Evidência e limites

Sonda executada com dois app-servers simultâneos, mesma HOME temporária e `CODEX_HOME` diferentes:

- Cada servidor gravou seu próprio `auth.json`, usando uma chave fictícia distinta, sem autenticação remota.
- `config/value/write` alterou o esforço da segunda conta e deixou os bytes do `config.toml` da primeira intactos.
- `account/logout` na segunda manteve a primeira autenticada localmente.
- `hooks/list` encontrou a mesma definição nas duas pastas, com identificadores próprios e `trustStatus=untrusted` em ambas.
- Os processos foram encerrados e a pasta temporária removida.

Essa sonda comprova separação de armazenamento e processos. A prova real posterior está registrada
abaixo. Renovação OAuth e funcionamento no Windows continuam fora do que foi medido.

## Critério de conclusão da implementação

Duas contas com login independente, preferências herdadas da padrão, escolha correta no web e no app nativo, sessões simultâneas e retomada conservando a conta. Uma alteração ou logout na secundária não pode modificar a padrão. Configuração inválida, plugin ausente ou hook pendente deve ficar visível. Verificar atualização da herança e origem das conversas arquivadas, além dos testes de API e interfaces.

Fontes oficiais: [Autenticação](https://learn.chatgpt.com/docs/auth), [Configuração avançada](https://learn.chatgpt.com/docs/config-file/config-advanced), [App-server e login nativo](https://learn.chatgpt.com/docs/app-server).

## Implementação atual e provas

O fluxo implementado está em [cadastro e origem das contas](../backend/app/codex_contas.py),
[herança seletiva](../backend/app/codex_contas_sync.py), [login nativo e rotas](../crates/hangar-server/src/accounts/http.rs)
e [plugins](../backend/app/codex_contas_plugins.py). O
[guia de uso](USAGE.md#contas-codex-chatgpt) descreve cadastro, seleção e retomada. A prova repetível
de separação usa o [teste nativo isolado](../backend/tests/test_codex_contas_native.py).

Provas registradas para esta etapa:

- A suíte completa do backend teve **4168 passados, 60 ignorados e 14 avisos de PTY** antes das
  correções posteriores; essa contagem não é uma rodada completa final pós-fix.
- A regressão focada da Task 10 teve **15 testes passados** e a revisão 3 aprovou a reprodução
  independente de `chosen.md` seguido de `ExitPlanMode`.
- Integração nativa: **16 passaram**, com HOME temporária, armazenamento `file`, chaves fictícias e
  sem iniciar modelos.
- `npm run test` da raiz passou com **472 no core, 1236 no web e 126 no app nativo**.
- `npm run check` da raiz e `npm run build -w frontend` passaram; os avisos registrados eram
  existentes em `AdicionarMaquina.svelte` e de chunks/imports do build.
- As fixtures desktop/mobile da Task 10 foram conferidas na revisão final. As fixtures web cobrem o
  fluxo visual, mas não login OAuth real.

As duas correções posteriores de integração foram concluídas. Os testes focados de
`test_codex_contas_sync.py` e `test_create_modelo_api.py` tiveram **51 passados**. A conferência
estrita do modelo Astra aprovou as duas correções e encerrou a revisão de integração.

Em 10/09/2026, o fluxo real preservou a conta padrão Pro e conectou uma secundária Plus por código
de dispositivo; os e-mails foram omitidos desta documentação pública. A preparação terminou `ready`, com
20/20 plugins iguais em versão e habilitação. O CLI 0.153.4 não reconheceu plugins quando apenas o
cache ou o catálogo embutido foi ligado por symlink; a implementação passou a materializar o catálogo
reservado dentro do `CODEX_HOME` secundário. Os hooks só foram aprovados depois de autorização explícita.
Uma sessão mínima em `gpt-5.6-luna` respondeu `OK`, gravou o rollout em `~/.codex-google`, apareceu no
Arquivo depois de fechada e foi retomada pela conta `google`.

Teste físico do app e Windows não foram verificados. Sem dispositivo conectado, o AVD do Hangar não
foi iniciado. Renovação OAuth também não foi medida. A rodada intermediária da Task 6 pode ter enumerado
histórico pessoal; ela foi corrigida e repetida, portanto não se afirma isolamento retroativo de todos
os testes.
