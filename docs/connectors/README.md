# Conectores do Hangar

Um conector é um programa que o Hangar executa para ler trabalho de um sistema externo (Help, Jira,
Linear). O host é Rust (`crates/hangar-connectors`) e conversa com o conector por JSON-RPC no
stdin/stdout. A linguagem do conector é livre: o Hangar não instala interpretador para ele.

O host não conhece fornecedor. Ele conhece o manifesto, o transporte e o contrato de cada
**capacidade**. A primeira é `work_tracking/v1`: listar e detalhar tickets, só leitura
([contrato](work-tracking-v1.md)). Um conector não ganha acesso às funções internas do Hangar, à
interface nem à abertura de sessões.

- [Contrato `work_tracking/v1`](work-tracking-v1.md)
- [Testar um conector](testing.md), inclusive em outra máquina

## Manifesto

```json
{
  "manifest_version": 1,
  "id": "help-test",
  "name": "Help de teste",
  "version": "0.1.0",
  "protocol_version": 1,
  "capabilities": { "work_tracking": 1 },
  "command": ["./bin/hangar-help-test-connector"],
  "start_prompt": "/pmedico:iniciar-pm {key}"
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

Exemplos: o conector de teste do Help (binário Rust, `crates/hangar-connectors/src/bin/help_test_connector/`)
e um conector mínimo em JavaScript ([`examples/connectors/`](../../examples/connectors/)), com
`"command": ["node", "./work_tracking.mjs"]`.

## Transporte

- JSON-RPC 2.0, uma mensagem UTF-8 por linha. O stdout é só do protocolo; log vai ao stderr (o host
  o descarta).
- O host abre a vida com `initialize` (`{"protocol_version": 1, "host": {"name": "hangar"}}`). O
  conector responde `{"protocol_version": 1, "id": "<id do manifesto>", "capabilities": {...}}`;
  versão, `id` ou capacidades diferentes do manifesto recusam o conector.
- Métodos desta versão: `initialize`, `work_tracking.list`, `work_tracking.get` e `shutdown`. Um
  pedido por vez; responda cada um com o mesmo `id`.
- Erro: `{"code": <número>, "message": "...", "data": {"code": "<código>"}}`. Códigos:
  `invalid_params`, `unsupported_capability`, `unsupported_version`, `invalid_cursor`,
  `auth_required`, `permission_denied`, `not_found`, `unavailable`, `timeout`, `protocol_error` e
  `payload_too_large`.
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

No app nativo, o botão de painel da barra do topo (ou `Ctrl+Alt+M`) abre os tickets do conector em
três colunas: A fazer, Em andamento e Concluído. Concluído vem do status do fornecedor; um ticket com
sessão ligada e viva vai para Em andamento. O card mostra o estado da sessão e o progresso do plano
dela. No detalhe, Iniciar sessão leva à tela de nova conversa com o `start_prompt` no campo; a sessão
que nascer com essa mensagem fica ligada ao ticket.

## Administração

Rotas exclusivas do dono (convidado e token errado são recusados antes de ler o corpo):

| Método | Rota | Resultado |
|---|---|---|
| GET | `/api/connectors` | Cadastrados e `running`, sem iniciar processos |
| POST | `/api/connectors` | `{"manifest": "<caminho absoluto no servidor>"}` |
| PATCH | `/api/connectors/{id}` | `{"enabled": false}` desliga e encerra |
| DELETE | `/api/connectors/{id}` | Para e remove |
| GET | `/api/connectors/{id}/tickets` | `query`, `assignee`, `limit` (padrão 20), `cursor` |
| GET | `/api/connectors/{id}/tickets/{ticket_id}` | Detalhe, com `start_prompt` já preenchido quando o manifesto o tem; o ID vai codificado como um componente |
| PUT | `/api/connectors/{id}/links/{ticket_id}` | `{"session": "<nome>"}` liga o ticket à sessão aberta para ele |
| DELETE | `/api/connectors/{id}/links/{ticket_id}` | Desfaz o vínculo |

Os vínculos ficam no catálogo, no campo `links` de cada conector (ticket → nome da sessão), e saem
com ele. São do dono, não do fornecedor: nada vai ao sistema de origem.

Erros saem como `{"detail": {"code": "...", "msg": "..."}}`. Falta de login no fornecedor
(`auth_required`, `permission_denied`) volta 424, nunca 401, para não parecer senha do Hangar errada.

O mesmo pelo terminal, falando com o servidor desta máquina:

```bash
hangar-server connectors add /caminho/do/manifesto.json
hangar-server connectors list
hangar-server connectors tickets help-test --assignee me --limit 20
hangar-server connectors tickets help-test --cursor '<next_cursor>'
hangar-server connectors ticket help-test PM-123
hangar-server connectors disable help-test
hangar-server connectors remove help-test
```

`--registry <arquivo>` antes do comando usa um host isolado, com catálogo próprio, sem servidor
(ver [testing.md](testing.md)). Saída em JSON; erro em JSON no stderr com código de saída diferente
de zero (1 recusa, 2 uso, 3 servidor fora do ar).

O catálogo do servidor fica em `~/.hangar/connectors.json`, privado e gravado de forma atômica.
