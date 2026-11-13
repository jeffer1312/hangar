# Mission Control

Painel do app nativo que acompanha cada ticket de um conector (`work_tracking`) pelas etapas do
trabalho do agente. O botão de painel da barra do topo, ou `Ctrl+Alt+M`, abre a página.

O que mora onde:

- O ticket (título, status, descrição) é do sistema de origem, lido pelo conector. Nada é escrito lá.
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

O botão Etapas troca a lista. Uma etapa por linha, `Nome | portão | Status, Status`. O nome igual ao
de uma etapa existente mantém o `id` dela; etapa que sai deixa de valer nos tickets, que voltam a ser
deduzidos.

A coluna de cada ticket, nesta ordem:

1. Concluído, se o status de origem é da categoria concluída ou se o dono marcou a entrega.
2. A etapa informada pela sessão (tool `mission`) ou escolhida pelo dono (Mover para).
3. A etapa cujo `statuses` contém o status de origem.
4. Com sessão viva, a dedução pelo plano dela: sem plano, a primeira etapa; plano em andamento, a
   segunda; plano concluído, a terceira.
5. Sem nada disso, Entrada.

## Sessão do ticket

O Iniciar abre a tela de nova conversa com o `start_prompt` do conector no campo. A primeira sessão
criada depois dele, cuja mensagem começa por esse texto, fica ligada ao ticket. Todo vínculo novo faz o
servidor mandar à sessão, pela rota de entrada dela, um aviso `[painel: Mission Control]` explicando a
tool `mission`.

Sem vínculo, o painel acha a sessão viva cuja branch é a chave do ticket ou cujo nome começa por ela
(`PM-19816`, `PM-19816-Coordenadora`), como o plugin Promedico identifica a PM. "Ligar ao ticket"
grava o vínculo e passa a valer a tool. Uma sessão fica ligada a um ticket só.

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

O ID do ticket vai codificado como um componente do caminho.
