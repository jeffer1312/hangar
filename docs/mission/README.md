# Tickets

Painel do app nativo que acompanha cada ticket de um conector (`work_tracking`) pelas etapas do
trabalho do agente. O botão de painel da barra do topo, ou `Ctrl+Alt+M`, abre a página.

O que mora onde:

- O ticket (título, status, descrição) é do sistema de origem, lido pelo conector. As ações acionam
  o fluxo do plugin, que continua responsável pelas mudanças no sistema de origem.
- Etapas, vínculo com a sessão, aprovações, evidências e histórico são do Hangar, no crate
  `hangar-mission`, em `~/.hangar/mission/` do servidor (`mission.json` e a pasta `evidence/`).

## Etapas

Colunas fixas nas pontas, Entrada e Concluído; no meio, as etapas do servidor. As padrão seguem o
caminho de uma PM no plugin Promedico:

| Etapa | `id` | Portão | Indicada pelo status |
|---|---|---|---|
| Planejamento | `planejamento` | | |
| Execução | `execucao` | | |
| Revisão | `revisao` | | Revisão de Código |
| Validação | `validacao` | sim | |
| Entrega | `entrega` | | |

O botão Etapas abre um formulário com a lista à esquerda e os campos da etapa selecionada à
direita. É possível adicionar, renomear, reordenar e remover etapas, exigir aprovação, associar
ações do conector e informar os status de origem, um por linha. Renomear e reordenar preservam o
`id`. Cancelar descarta a edição inteira. Etapas removidas deixam de valer nos tickets; a dedução
por plano ou revisão exige o ID correspondente, sem escolher uma coluna pela posição.

A busca, a origem e a escolha de repositório ou pasta ficam à esquerda do board. Iniciar e
Executar etapa usam essa escolha e os padrões de criação do servidor. Várias ações associadas à
etapa abrem uma escolha; cancelar não cria sessão nem envia mensagem. O detalhe tem alça de
largura, ajuste pelo teclado e opção de ampliar. A aba Conversa reutiliza o chat, as perguntas e a
aprovação do plano da sessão vinculada. Somente a conversa selecionada abre um stream; trocar de
ticket conserva o rascunho e encerra o acompanhamento anterior.

Cada etapa também pode guardar `prompt` opcional (até 4.000 bytes) e `actions`, um mapa do ID do
conector para os IDs das ações escolhidas. Campos ausentes em um pedido antigo preservam a
configuração atual; para limpar, envie `prompt: ""` e `actions: {}` explicitamente. A identidade
`id` continua igual ao renomear ou reordenar. O registro em disco aceita a versão 1 e grava a versão 2
na próxima mudança, sob a mesma trava e troca atômica; um servidor antigo recusa essa versão em vez
de descartar os campos novos. Tickets, histórico e evidências são preservados.

A coluna de cada ticket, nesta ordem:

1. Concluído, se o status de origem é da categoria concluída ou se o dono marcou a entrega.
2. A etapa informada pela sessão (tool `mission`) ou escolhida pelo dono (Mover para).
3. A etapa cujo `statuses` contém o status de origem.
4. Com PR ou MR na branch do ticket: aberto, a etapa `revisao` (sem ela, a terceira); mesclado, a
   seguinte. Fechado sem merge não conta.
5. Com sessão viva, a dedução pelo plano dela: sem plano, a primeira etapa; plano em andamento, a
   segunda; plano concluído, a terceira.
6. Sem nada disso, Entrada.

## Merge request do ticket

O painel usa a API das Revisões (`/api/reviews`), com o `gh` ou o `glab` logado na máquina do
servidor. O PR ou MR do ticket é o da branch da sessão dele (`/api/reviews/branch`) e, sem esse, o
aberto da máquina cuja branch de origem começa pela chave (`PM-19816`, `PM-19816-ajuste`) ou que tem
a sessão nela. O cartão mostra o número, o pipeline e as threads abertas; o detalhe mostra também o
estado e os conflitos e abre a revisão na página de Revisões, cujo voltar devolve ao painel. Sem login
no CLI do host, o detalhe diz isso no lugar do MR.

## Sessão do ticket

O Iniciar usa a pasta e as opções escolhidas à esquerda, num modal sobre o próprio board. A sessão
é associada ao ticket antes da primeira ação. Executar etapa envia a ação do conector com o prompt
opcional; havendo várias ações, a pessoa escolhe uma. A conversa, as perguntas e o plano ficam no
detalhe do ticket, com os mesmos controles da conversa principal. Arrastar ou Mover para apenas
registra a etapa, sem executar prompt nem abrir sessão.

Sem vínculo, o painel acha a sessão viva cuja branch é a chave do ticket ou cujo nome começa por ela
(`PM-19816`, `PM-19816-Coordenadora`), como o plugin Promedico identifica a PM. Associar a sessão
detectada grava o vínculo e passa a valer a tool. Uma sessão fica ligada a um ticket só. A associação
manual avisa a sessão pelo painel Tickets e explica a tool `mission`.

## Portão e evidências

A sessão ligada informa a etapa e anexa evidências pela tool `mission` do MCP `hangar`:

```json
{"etapa": "validacao", "nota": "pronto para conferir",
 "evidencias": [{"caminho": "/caminho/absoluto/tela.png", "legenda": "botão de imprimir"}]}
```

Sem argumentos, a tool devolve o ticket ligado e as etapas. Evidência é imagem, vídeo, PDF ou texto
(`png`, `jpg`, `gif`, `webp`, `mp4`, `webm`, `mov`, `pdf`, `txt`, `md`, `log`), até 512 MiB; o
servidor copia o arquivo, então o original pode sair. Um lote com um item recusado não guarda nenhum.
O dono também anexa pelo painel, até 100 MiB por arquivo.

Etapa com portão espera o dono: a sessão entra nela, mas não passa adiante (`mission_gate_pending`)
até a aprovação, e voltar para antes de um portão o fecha de novo. No painel, Aprovar leva o ticket à
etapa seguinte; Pedir ajustes volta uma etapa, desfaz as aprovações daí em diante e, depois de gravado,
o servidor manda o motivo à sessão ligada. Os dois levam a etapa que a tela mostra, que vale quando o
ticket ainda não tem etapa gravada (deduzida pelo status ou pelo plano).

As evidências ficam no Hangar e não vão para o sistema de origem: o fluxo do plugin Promedico não
sobe registros de execução nem evidências na PM.

## API

Rotas só do dono, nunca repassadas ao Python:

| Método | Rota | Corpo ou resultado |
|---|---|---|
| GET | `/api/mission` | `{stages, items}` |
| PUT | `/api/mission/stages` | `{"stages": [{"id", "name", "gate", "statuses"}]}` |
| PUT | `/api/mission/items/{conector}/{ticket}` | `{"key", "title", "url", "session"}` liga a sessão |
| PATCH | `/api/mission/items/{conector}/{ticket}` | `{"action": "stage", "stage"}`, `{"action": "approve", "stage"?}`, `{"action": "changes", "note", "stage"?}`, `delivered`, `reset` |
| DELETE | `/api/mission/items/{conector}/{ticket}` | Esquece o registro e as evidências |
| POST | `/api/mission/items/{conector}/{ticket}/evidence?name=&caption=` | Bytes do arquivo |
| DELETE | `/api/mission/items/{conector}/{ticket}/evidence/{id}` | Tira uma evidência |
| GET | `/api/mission/evidence/{id}` | O arquivo, com faixas (vídeo) |
| POST | `/api/mission/report` | `{"session", "stage", "note", "evidence": [{"path", "caption"}]}`, o que a tool chama |
| POST | `/api/mission/actions` | Solicita a ação de uma etapa; devolve o recibo da tentativa |
| GET | `/api/mission/actions/{tentativa}` | Sessão e resultado guardados |
| GET | `/api/mission/actions?connector=&ticket_id=` | Última tentativa (`attempt`) e última solicitação de execução (`execution`) |

O ID do ticket vai codificado como um componente do caminho.

## Despacho de ações

O catálogo vem de [`work_actions/v1`](../connectors/work-actions-v1.md). O servidor relê a etapa,
o ticket e o catálogo antes de qualquer efeito. A escolha leva a impressão que foi mostrada:

```json
{
  "attempt": "0123456789abcdef0123456789abcdef",
  "connector": "help",
  "ticket_id": "PM-123",
  "stage": "planejamento",
  "action": "start-pm",
  "fingerprint": "impressão devolvida pelo catálogo",
  "open": {"cwd": "/workspace/projeto", "provider": "claude"}
}
```

`attempt` é uma identidade aleatória de 32 caracteres hexadecimais minúsculos. Repetir o mesmo
pedido devolve o mesmo recibo; usar essa identidade com outro pedido é `ticket_action_conflict`.
O trabalho continua mesmo se o cliente desconectar e tem prazo de 90 segundos. Estados:
`preparing`, `opening`, `sending`, `sent`, `queued`, `recorded`, `failed` e `uncertain`.
`sent` e `queued` confirmam somente a aceitação da mensagem, sem declarar a skill concluída.
`recorded` registra uma etapa sem mensagem ou um movimento para a etapa em que o ticket já está.

Arrastar e Mover para enviam `execute: false`: registram a escolha do dono, mesmo que a etapa tenha
ações e prompt. Não leem o catálogo de ações nem abrem sessão. Executar usa `execute: true`
(padrão quando omitido). O movimento manual não apaga uma execução anterior incerta: seu recibo
continua separado e precisa ser conferido antes de repetir um comando.

O campo `open` conserva as opções do lançador único e os padrões do servidor. Nome, primeira
mensagem, criadora e preparação de branch não vêm desse campo: o dispatcher escolhe o nome,
associa o ticket e só então envia; a skill continua responsável pelo fluxo do projeto. Sessão já
vinculada é reutilizada, conferindo vida e conversa durante a escrita. A troca do nome mantém
o vínculo. Uma ação não toma a sessão de outro ticket.

O comando da ação e o prompt opcional formam uma mensagem normal, que pode entrar na fila.
Sem `work_actions`, o `start_prompt` continua disponível no primeiro início. Sem ação nem prompt,
o servidor registra a etapa sem abrir sessão. Uma escolha ausente ou antiga é recusada;
etapas com portão mantêm suas exigências de aprovação. Relato novo do agente não é regredido
pela confirmação do envio.

`repeat: true` executa explicitamente de novo na mesma etapa. Resultado incerto nunca é
reenviado sozinho: confira a conversa antes de um novo pedido com `confirm_retry: true`.
Uma abertura incompleta pode recuperar sua sessão guardada, desde que a identidade continue
válida e ela não tenha sido associada a outro ticket. O recibo fica em `ticket-actions/` no
diretório de dados do Hangar. Se a gravação falhar após um efeito, o processo conserva o resultado
incerto para consulta; após reiniciar, uma tentativa durável ainda em andamento também é incerta.
