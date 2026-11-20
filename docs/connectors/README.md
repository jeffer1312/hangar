# Conectores do Hangar

Um conector é um programa que o Hangar executa para ler trabalho de um sistema externo (Help, Jira,
Linear). O host é Rust (`crates/hangar-connectors`) e conversa com o conector por JSON-RPC no
stdin/stdout. A linguagem do conector é livre: o Hangar não instala interpretador para ele.

O host não conhece fornecedor. Ele conhece o manifesto, o transporte e o contrato de cada
**capacidade**. A primeira é `work_tracking/v1`: listar e detalhar tickets, só leitura
([contrato](work-tracking-v1.md)). Um conector não ganha acesso às funções internas do Hangar, à
interface nem à abertura de sessões.

- [Contrato `work_tracking/v1`](work-tracking-v1.md)
- [Catálogo de ações `work_actions/v1`](work-actions-v1.md)
- [Revisão do trabalho `work_review/v1`](work-review-v1.md)
- [Testar um conector](testing.md), inclusive em outra máquina

## Manifesto

```json
{
  "manifest_version": 1,
  "id": "tickets-demo",
  "name": "Tickets de exemplo",
  "version": "0.1.0",
  "protocol_version": 1,
  "capabilities": { "work_tracking": 1 },
  "command": ["./bin/tickets-connector"],
  "start_prompt": "Planeje o trabalho do ticket {key}: {title}"
}
```

- `id`: de 1 a 64 caracteres, minúsculas, dígitos e hífen, começando por letra; único no servidor.
- `command`: vetor executado direto, sem shell. O primeiro item é o programa ou o interpretador.
  Programa relativo com separador de caminho (`./bin/x`) parte da pasta do manifesto; nome simples
  (`node`) usa o PATH do servidor. A pasta do manifesto é a pasta de trabalho do processo, então os
  arquivos relativos dos argumentos também partem dela.
- `start_prompt` (opcional): primeira mensagem da sessão que o Mission Control abre a partir de um
  ticket. `{key}`, `{id}`, `{title}` e `{url}` viram os campos do ticket, numa passada só; marcador
  desconhecido fica como está. Sem ele, o app propõe um texto com a chave, o título e o link.
- Campo fora desta lista é recusado: o manifesto não leva token, segredo, código de shell nem
  comando de instalação. A configuração do fornecedor é do próprio conector.

Há um conector mínimo em JavaScript ([`examples/connectors/`](../../examples/connectors/)), com
`"command": ["node", "./work_tracking.mjs"]`.

## Distribuição

O Hangar distribui o host genérico e o protocolo, sem conectores de fornecedores embutidos.
Cada fornecedor distribui seu conector, manifesto e instruções de instalação e atualização.
O dono instala o programa na máquina do servidor e cadastra o manifesto por
`hangar-server connectors add <manifesto>`. O host executa o comando cadastrado, sem substituí-lo
por um programa interno ou acrescentar capacidades. Credenciais e login ficam com o conector.

## Transporte

- JSON-RPC 2.0, uma mensagem UTF-8 por linha. O stdout é só do protocolo; log vai ao stderr (o host
  o descarta).
- O host abre a vida com `initialize` (`{"protocol_version": 1, "host": {"name": "hangar"}}`).
  Quando o manifesto declara qualquer capacidade além de `work_tracking/v1` (`work_actions/v1`,
  `work_review/v1`), o pedido acrescenta `capabilities` com todas as capacidades cadastradas, para
  ativar as extensões. Sem esse campo, a negociação é somente `work_tracking/v1`; o conector deve
  aceitar essa forma básica. O
  conector responde `{"protocol_version": 1, "id": "<id do manifesto>", "capabilities": {...}}`;
  versão, `id` ou capacidades diferentes do manifesto recusam o conector.
- Métodos: `initialize`, `work_tracking.list`, `work_tracking.get`, o catálogo opcional
  `work_actions.list`, a revisão opcional `work_review.get`, `work_review.asset` e
  `work_review.decide`, e `shutdown`. Um pedido por vez; responda cada um com o mesmo `id`.
- Erro: `{"code": <número>, "message": "...", "data": {"code": "<código>"}}`. Códigos:
  `invalid_params`, `unsupported_capability`, `unsupported_version`, `invalid_cursor`,
  `auth_required`, `permission_denied`, `not_found`, `unavailable`, `timeout`, `protocol_error`,
  `payload_too_large` e `stale_review` (a revisão mudou de rodada; HTTP 409).
- Teto de 4 MiB por mensagem; maior é `payload_too_large`, nunca cortada.

## Ciclo de vida

- Cadastrar, listar, ligar ou reiniciar o servidor não inicia processo. O primeiro pedido de dados
  lança e negocia (prazo de 10 s); cada chamada tem 30 s, incluindo a espera pela vez.
- Uma vida por conector, reaproveitada entre pedidos. 60 s sem pedido encerram o processo; o
  próximo pedido abre outro.
- Desligar, remover ou encerrar o host cancela o pedido em andamento e termina a árvore inteira do
  processo (filhos incluídos): pedido de `shutdown`, até 3 s de espera e encerramento forçado.
- Prazo esgotado, saída inválida ou processo que caiu encerram aquela vida. A chamada que falhou
  não é repetida em silêncio; a próxima abre uma vida nova.
- O conector roda com os direitos do dono, sem sandbox, mas não herda a senha do Hangar nem o
  segredo interno do servidor.

## Mission Control

O painel do app nativo que acompanha os tickets pelas etapas do trabalho:
[`docs/mission/`](../mission/README.md).

## Administração

Rotas exclusivas do dono (convidado e token errado são recusados antes de ler o corpo):

| Método | Rota | Resultado |
|---|---|---|
| GET | `/api/connectors` | Cadastrados e `running`, sem iniciar processos, e `capabilities`: as capacidades e versões que este servidor aceita no manifesto (ausente num servidor antigo, que só conhece `work_tracking` e `work_actions`). O instalador do conector confere antes de declarar uma capacidade nova; `hangar-server connectors list` devolve o mesmo |
| POST | `/api/connectors` | `{"manifest": "<caminho absoluto no servidor>"}` |
| PATCH | `/api/connectors/{id}` | `{"enabled": false}` desliga e encerra |
| DELETE | `/api/connectors/{id}` | Para e remove |
| GET | `/api/connectors/{id}/tickets` | `query`, `assignee`, `state` (`open` ou `done`), `limit` (padrão 20), `cursor` |
| GET | `/api/connectors/{id}/tickets/{ticket_id}` | Detalhe, com `start_prompt` já preenchido quando o manifesto o tem; o ID vai codificado como um componente |

Erros saem como `{"detail": {"code": "...", "msg": "..."}}`. Falta de login no fornecedor
(`auth_required`, `permission_denied`) volta 424, nunca 401, para não parecer senha do Hangar errada;
`stale_review` volta 409.

O mesmo pelo terminal, falando com o servidor desta máquina:

```bash
hangar-server connectors add /caminho/do/manifesto.json
hangar-server connectors list
hangar-server connectors tickets tickets-demo --assignee me --limit 20
hangar-server connectors tickets tickets-demo --cursor '<next_cursor>'
hangar-server connectors ticket tickets-demo ENG-123
hangar-server connectors disable tickets-demo
hangar-server connectors remove tickets-demo
```

`--registry <arquivo>` antes do comando usa um host isolado, com catálogo próprio, sem servidor
(ver [testing.md](testing.md)). Saída em JSON; erro em JSON no stderr com código de saída diferente
de zero (1 recusa, 2 uso, 3 servidor fora do ar).

O catálogo do servidor fica em `~/.hangar/connectors.json`, privado e gravado de forma atômica.
