# Capacidade `work_review/v1`

Revisão do trabalho de um ticket: o que o agente está conferindo, as decisões que esperam a pessoa
e o que já foi corrigido ou mantido. O painel da sessão no Hangar mostra a revisão e devolve as
decisões ao conector, que as entrega ao agente. Todo texto (títulos, rótulos, etapas, escolhas) vem
do conector; o Hangar só escreve os textos genéricos (enviar, anterior e próxima, "há 12 min").
Declarar `"work_review": 1` no manifesto e no `initialize` para oferecer esta capacidade. Ela
requer `work_tracking/v1`.

## Qual ticket e quando o Hangar relê

O ticket da sessão é o vínculo do Mission Control, pela mesma regra da tool `mission`: o item ligado
à sessão, ou a um membro do grupo dela, preferindo o não entregue; sem ele, o último entregue. Sem vínculo, o Hangar chama `work_tracking.get` com a branch da sessão em cada
conector com `work_review` e aceita só a resposta cujo `key` é igual à branch; o painel mostra
"achado pela branch". O vínculo vence a branch. O resultado fica guardado até a branch mudar.

O conector não avisa o Hangar: o host só lê a resposta do pedido que fez. O Hangar relê o
`work_review.get`:

- ao abrir a aba Revisão do painel da sessão;
- no fim de cada turno da sessão, mesmo sem painel aberto (é o que atualiza o chip do card);
- depois de um `decide`;
- a cada chamada da tool `mission` pela sessão ligada, inclusive sem argumentos. Para a barra de
  progresso andar num turno longo, o agente chama `mission` depois de gravar o progresso.

## `work_review.get`

Pedido: `{"ticket_id": "<id opaco>"}`, com os limites de `work_tracking.get`. Resposta:
`{"review": null}` quando o ticket não tem revisão, ou:

```json
{"review": {
  "id": "rodada-7",
  "title": "Comparação com o mock",
  "subject": "Controle Bancário",
  "updated_at": "2026-11-18T10:00:00-03:00",
  "state": "waiting",
  "steps": {"labels": ["Prints", "Comparação", "Correções", "Sua decisão", "Pronto"], "current": 3},
  "headline": {"value": "2", "text": "diferenças esperam a sua decisão", "detail": "O agente já corrigiu 15 sozinho."},
  "badge": {"text": "mock: 2 para você", "tone": "warning"},
  "decisions": [{
    "id": "47",
    "tag": "Diferença 47",
    "title": "Janela da Movimentação menor que no mock",
    "reason_label": "Por que o agente não igualou",
    "reason": "padrão do kit: a tela usa o AreaComponente no tamanho padrão",
    "left":  {"label": "Mock", "value": "~1110 px", "asset": "mock.png:1731940000", "width": 1600, "height": 900, "box": [385, 263, 830, 372]},
    "right": {"label": "Tela", "value": "~830 px",  "asset": "tela.png:1731940000", "width": 1600, "height": 900, "box": [385, 263, 830, 372]},
    "choices": [
      {"id": "igualar", "label": "Igualar ao mock", "hint": "o agente corrige a tela"},
      {"id": "manter", "label": "Manter como está", "hint": "fica registrado o porquê",
       "requires_reason": true, "default_reason": "padrão do kit", "reason_prompt": "Por que manter? Vai para o relatório"}
    ]
  }],
  "resolved": {"label": "Corrigidas pelo agente", "count": 15, "items": [{"title": "Cabeçalho", "before": "a3", "after": "a4"}]},
  "kept": {"label": "Mantidas pelo agente, com motivo", "count": 3, "items": [{"title": "Selo Open Finance", "reason": "dado sem fonte no backend"}]},
  "links": [{"label": "Relatório completo", "url": "http://127.0.0.1:47613/c/x/", "local": true}]
}}
```

| Campo | Obrigatório | Regra |
|---|---|---|
| `id` | sim | Opaco; muda a cada rodada nova da revisão |
| `title`, `subject` | `title` | Até 200 caracteres |
| `updated_at` | sim | RFC 3339 com fuso |
| `state` | sim | `running`, `waiting`, `applying` ou `done`. A cor do painel sai daqui |
| `steps` | não | `labels` de 1 a 12; `current` de 0 ao total (igual ao total = todas feitas) |
| `headline` | não | `value` até 16 caracteres; `text` e `detail` (opcional) até 2.000 |
| `progress` | não | `{label, done, total}`, só em `running`; `total` ≥ 1 e `done` ≤ `total` |
| `badge` | não | `text` (obrigatório) até 200; `tone` (obrigatório) `warning`, `success` ou `info`, só para o chip do card |
| `decisions` | não | Até 50. Ao menos uma em `waiting`; vazio nos outros estados |
| `resolved` | não | `{label, count?, items}`: até 200 itens `{title, before?, after?}` (IDs de asset) |
| `kept` | não | `{label, count?, items}`: até 200 itens `{title, reason}`, os dois obrigatórios |
| `links` | não | Até 8 `{label, url, local?}`; `url` HTTP/HTTPS absoluta |

Listas (`decisions`, `links`, `steps.labels`, `resolved.items`, `kept.items`) vêm ausentes ou como
array, nunca `null`.

Decisão:

- `id` único na revisão; `title` obrigatório; `tag`, `reason_label` e `reason` opcionais.
- `left` e `right` são opcionais, mas `right` exige `left`. Só `left`: o painel mostra um lado, na
  largura toda. Sem os dois: só título, motivo e escolhas.
- Cada lado: `label` (obrigatório), `value` (opcional), `asset`, `width` e `height` (1 a 16384) e
  `box` opcional, `[x, y, largura, altura]` em px da imagem, que o painel amplia. O trecho tem
  largura e altura de ao menos 1 e cabe na imagem (`x + largura ≤ width`, `y + altura ≤ height`).
- `choices` de 1 a 4, com `id` único na decisão em `[a-z][a-z0-9-]{0,63}`. `label` obrigatório;
  `hint`, `default_reason` e `reason_prompt` opcionais. `requires_reason: true` faz o painel pedir o
  motivo, já preenchido com `default_reason`.

Limites gerais:

- IDs de revisão, decisão e asset: 1 a 128 bytes de `[A-Za-z0-9._:-]`.
- Textos: `title`, `subject`, `tag`, `reason_label`, `badge.text`, rótulos (`label` e
  `steps.labels`) e títulos de item até 200 caracteres; `headline.value` até 16; os demais até 2.000.
  Obrigatório vazio e NUL são recusados.
- `count` de `resolved` e `kept`, quando vem, é o total e não fica abaixo dos itens; maior que eles,
  o painel mostra "+N".
- Resposta inteira até 3 MiB, medida no JSON do `result`. O transporte recusa acima de 4 MiB, mas
  os tetos dos campos somados passariam disso.
- Fora de qualquer regra é `protocol_error`, nunca cortado, e encerra a vida do processo. Campos
  extras são descartados na resposta validada.
- `links[].local: true`: o endereço só abre na máquina do conector. Nas outras, o painel mostra o
  link com esse aviso.

## `work_review.asset`

Pedido: `{"ticket_id", "review_id", "asset"}`. Resposta: `{"mime": "image/png", "data": "<base64>"}`.

- `mime` é `image/png`, `image/jpeg` ou `image/webp`, e os bytes iniciais conferem com ele
  (PNG `89 50 4E 47 0D 0A 1A 0A`, JPEG `FF D8 FF`, WebP `RIFF` + 4 bytes + `WEBP`).
- `data` em base64 padrão (RFC 4648, com `=`), até 2 MiB depois de decodificado.
- As imagens vêm sempre pelo conector, nunca por URL: o app pode estar em outra máquina.
- **Imutável dentro do `review_id`:** o mesmo ID de asset devolve sempre a mesma imagem na mesma
  rodada, e o app guarda em cache por `review_id` + asset. Imagem nova = ID novo. O ID pode levar um
  carimbo (`<arquivo>:<carimbo>`): uma recaptura troca o ID, e pedir o ID antigo pode responder
  `stale_review`. O cliente então relê a revisão.

## `work_review.decide`

Pedido:

```json
{"ticket_id": "t-1", "review_id": "rodada-7",
 "decisions": [{"id": "47", "choice": "manter", "reason": "padrão do kit"}]}
```

- De 1 a 50 decisões, com `id` único; `choice` em `[a-z][a-z0-9-]{0,63}`; `reason` opcional, até
  2.000 caracteres, sem NUL.
- Resposta: `{"ok": true}`. Qualquer outra é `protocol_error`.
- **Em lotes:** o `get` mostra até 50 pendentes, e o `decide` leva as mostradas. O conector guarda
  cada decisão pelo `id` e a tira do próximo `get`, que continua em `waiting` com as restantes e o
  mesmo `review_id`. Quando a última pendente é decidida, o conector entrega todas ao agente de uma
  vez, e o `get` passa a `applying`.
- Antes de chamar o conector, o servidor relê o `get` e só segue com o mesmo `review_id`, as
  decisões mostradas, escolhas existentes e `reason` preenchido onde `requires_reason`. O conector
  confere de novo: `id` que não está pendente, `id` repetido no pedido, escolha inexistente ou
  motivo faltando são recusados.
- `review_id` diferente do atual: erro `stale_review` (HTTP 409); o cliente relê. Decisão recusada
  pelo conector: `invalid_params` ou `invalid_decision`, com a frase. O host aceita os dois nomes e
  trata `invalid_decision` como `invalid_params` (HTTP 400), mantendo a frase.
- **Idempotente:** repetir um lote já guardado (os mesmos `id` com as mesmas escolhas) responde
  `{"ok": true}`, também depois de a revisão sair de `waiting`. O app reenvia quando a resposta se
  perde.

## Pelo terminal

```bash
hangar-server connectors --registry "$REG" review <id> <ticket_id>
hangar-server connectors --registry "$REG" review-asset <id> <ticket_id> <review_id> <asset> > imagem.png
hangar-server connectors --registry "$REG" review-decide <id> <ticket_id> '{"review_id": "...", "decisions": [...]}'
```

Os três só funcionam com `--registry`: o servidor não tem rota de revisão por conector. As rotas
do app são por sessão.
