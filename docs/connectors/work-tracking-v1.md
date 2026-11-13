# Capacidade `work_tracking/v1`

Leitura de tickets, independente do fornecedor. O host valida o pedido que manda e a resposta que
recebe e repassa só os campos daqui: campo extra do conector não atravessa. Nenhum formato de chave
(PM, issue) é imposto.

## `work_tracking.list`

Pedido:

```json
{ "filter": { "query": "impressão", "assignee": "me" }, "limit": 20, "cursor": null }
```

- `query` e `assignee` são opcionais. `assignee` é `me` ou o `id` opaco de uma pessoa, o mesmo que
  o conector devolve em `assignee.id`.
- `limit` vai de 1 a 100.
- Filtro que o fornecedor não oferece é `invalid_params`, nunca ignorado.
- O cursor é opaco para o Hangar e vale só para os mesmos filtros: cursor de outra consulta é
  `invalid_cursor`. Ele não promete fotografia da fonte; a lista pode mudar entre páginas.

Resposta: `{ "items": [TicketSummary], "next_cursor": "..." | null, "total": número | null }`.
Mais itens que `limit` ou o mesmo `id` duas vezes na página é `protocol_error`.

## `TicketSummary`

| Campo | Tipo | Regra |
|---|---|---|
| `id` | texto | Opaco e estável; a identidade no Hangar é `(conector, id)` |
| `key` | texto | Referência legível para a pessoa |
| `title` | texto | |
| `url` | texto | HTTP/HTTPS absoluta |
| `status` | `{id, name, category}` | `category`: `todo`, `in_progress`, `done`, `canceled` ou `unknown`; `id` e `name` são os do fornecedor |
| `assignee` | `{id, name}` ou `null` | |
| `project` | `{id, name}` ou `null` | Sem projeto inequívoco, `null` |
| `updated_at` | texto ou `null` | RFC 3339 com fuso |

## `work_tracking.get`

Pedido: `{ "ticket_id": "<id>" }`. Resposta: o resumo mais `description_markdown` (texto, vazio
quando não há). Comentários, histórico e anexos não entram.

## Respostas HTTP

O host acrescenta `fetched_at` (RFC 3339, relógio do servidor) à lista e ao detalhe.

## Fora desta versão

Atualização é uma nova leitura: não há poller, eventos, ações, transições nem início de sessão a
partir do ticket. O status do ticket é do fornecedor; atividade do agente e etapas do plano são fatos
do Hangar, separados. Ampliação futura entra como capacidade nova e explícita no manifesto.
