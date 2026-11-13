# Capacidade `work_actions/v1`

Ações do fluxo de trabalho que o conector oferece para uma sessão do Hangar. O fornecedor escolhe
os rótulos e os comandos; o Hangar apresenta as escolhas e entrega a mensagem à sessão vinculada.
`work_tracking/v1` continua só leitura. Declarar `"work_actions": 1` no manifesto e no `initialize`
para oferecer esta capacidade.
Ela requer também `work_tracking/v1`, usado para preencher os dados do ticket.

## `work_actions.list`

Pedido: `{"ticket_id": "<id opaco>"}`. O ID segue os mesmos limites de `work_tracking.get`.

Resposta:

```json
{
  "actions": [
    {
      "id": "start-pm",
      "label": "Iniciar PM e planejar",
      "description": "Preparar o trabalho e iniciar as perguntas do planejamento.",
      "prompt_template": "/pmedico:iniciar-pm {key}",
      "stage": "planejamento"
    }
  ]
}
```

- Até 64 ações por resposta, com IDs únicos no catálogo. `id` segue `[a-z][a-z0-9-]{0,63}`.
- `label` é obrigatório, até 200 caracteres; `description` é opcional, até 1.000 caracteres.
- `prompt_template` é obrigatório, até 4.096 caracteres. Marcadores: `{key}`, `{id}`, `{title}` e
  `{url}`; a expansão é a mesma de `start_prompt`, em uma passada. Texto do ticket não é relido como
  marcador. NUL e texto obrigatório vazio são recusados.
- O prompt expandido tem teto de 64 KiB; exceder é `protocol_error`, sem envio parcial.
- `stage` é uma sugestão opcional de etapa. A configuração do board pode associar ações por ID a
  outras etapas sem renomear nem reordenar o catálogo do conector.
- Campos extras são descartados na resposta validada. Resultado malformado é `protocol_error`.

## Leitura pelo dono

`GET /api/connectors/{id}/actions?ticket_id=<id>` devolve o catálogo validado, com o prompt já
preenchido a partir do ticket e uma impressão de cada ação para conferir a escolha no despacho.
Conector sem a capacidade é `unsupported_capability`; seu `start_prompt` continua disponível para
o início normal de um ticket.

## Execução

O catálogo não executa comandos de shell nem transições diretamente. A ação é uma mensagem para
a sessão do ticket: por exemplo, uma skill do plugin Promedico. As perguntas e as validações do
plugin continuam no fluxo da sessão. O prompt opcional da etapa é acrescentado à mensagem da ação.
Uma ação é enviada diretamente; mais de uma abre a escolha. Cancelar não envia nada.

Comando de barra adiado/recusado não é execução bem-sucedida. O board distingue entrega confirmada,
fila e resultado incerto; a tentativa identificada não é reenviada automaticamente em incerteza.
