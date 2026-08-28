# Kimi K3 (`--provider kimi --model k3`) — papel: EXECUTOR

Primeira ficha: 24–25/08/2026, seis Tasks de um trabalho de 33 (setup, DDL + seeds, componente de
kit, duas telas de CRUD, e uma Task de backend .NET).

## Números

- **Task de CRUD com molde:** 1h29, contexto 295k. A **mesma tela**, outro nome, em Opus 5
  `medium`: **15 min**, contexto 266k. As três primeiras Tasks de porte fecharam em **283k / 244k /
  227k** de contexto.
- **Cota, e foi ela que decidiu o time:** **36–37% da cota semanal da assinatura em UM dia**, com 13
  Tasks por vir e a janela recém-resetada. Decisão do usuário em 25/08: executor passa a Opus 5
  `medium` da Task 6 em diante, com o Kimi como **fallback** se a cota Claude apertar.
- **Janela:** a entrada de assinatura por API key tem janela ampla e coube; a entrada de 256k **não
  caberia** — três das quatro sessões de porte teriam compactado no meio da Task.

## Qualidade: o que decidiu contra ele não foi qualidade

- Na Task de backend (rota nova sobre entidade externa, 44 testes novos) **passou de primeira**, e o
  achado central foi dele: um campo de "possui laudo" calculado a partir da tabela do irmão faria
  todo registro externo com laudo voltar dizendo que não tem — omissão que nenhum portão pegaria.
- Declarou pendência em vez de disfarçar: quando um teste de fumaça não pôde rodar, entregou **a
  tabela do que o teste cobriria contra o que existe**, rota a rota, e nomeou o ponto mais fraco da
  própria Task.
- Nas quatro Tasks de porte parou e reportou defeito do recorte em vez de improvisar, igual ao Opus.

## O que o kick-off precisa dizer por causa dele

- **Endereço:** o caminho nativo de sessão-a-sessão do Claude Code **não alcança sessão Kimi**. O
  kick-off dela leva o nome do hangar, não o nativo. Medido em 25/08/2026: o reporte voltou 404 e o
  executor achou o árbitro pelo remetente.
- **Esforço:** a apresentação do login por API key da assinatura mostra o esforço junto do alias;
  prove no pane como em qualquer outra sessão.

## Onde ele é bom

Task mecânica e Task de backend bem especificada, com cota folgada. **O que não vale gastar nele:**
Task de porte visual longa, onde o relógio é 4–6× o de um modelo mais caro e a cota some num dia.
