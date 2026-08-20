# opencode-go/muse-spark-1.2-contributor — ficha do modelo

**Status: HIPÓTESE (varredura de 19/08/2026, ainda sem execução medida).** Modelo lançado em
05/08/2026 (Meta, família Muse Spark); a comunidade tem duas semanas de dados. Tudo abaixo vem do
fabricante e de reviews públicos, não de rodada nossa — a primeira execução real
(2026-08-19, enxugada/C5/permissão no hangar) deve substituir esta seção por medição.

## O que se sabe (fontes públicas, 19/08/2026)

- **Janela 1.0M de contexto, 131.1K de saída** (catálogo do Pi nesta máquina). Tools: sim.
- **Tier "contributor": prompts e completions viram dado de treino da Meta** — é o preço do
  desconto (12–21× mais barato que o tier normal). **Avisar o usuário antes de toda execução
  que use este tier** — o código do repo vai nos prompts. **NUNCA usar em código de
  cliente/Promédico.**
- **Teto de 60 requisições/minuto** no tier contributor (o tier normal tem 3.000) — executor
  único serial não deve sentir; fan-out de subagentes pode esbarrar.
- Posição em ranking público de coding: mediano (#22/135, ~62.8/100 numa agregação de 08/2026).
  Não é um modelo de topo: **plano conservador** — receita literal, régua numérica, Step curto.

## Hipóteses de trabalho (até a primeira medição)

- **Tratar como executor que aplica receita literal** (mesma postura da ficha do
  deepseek-v4-flash): investir no detalhe do Step; critério visual vira número.
- ~~**Assumir que NÃO enxerga imagem** até prova em contrário → protocolo de visão do
  `executor.md` (`see <caminho>`) obrigatório nas Tasks de tela.~~
  **DERRUBADA em 19/08/2026 pelo usuário: o muse-spark TEM visão própria.** Ele lê o print com
  `Read` no caminho absoluto, direto. Chamar `see` aqui é um turno inteiro embrulhando uma
  capacidade que o modelo já tem — e foi o que aconteceu na rodada 1 da Task 1 do trabalho `enx`:
  o executor delegou a leitura do print, recebeu "nenhum menu", não acreditou na resposta e fechou
  a Task com prova de DOM sobre um menu invisível. Com visão própria a régua fica direta: **abra o
  print você mesmo e olhe** antes de declarar tela pronta.
- Thinking levels reais: conferir com `/cp-think` na sessão viva (o Pi trunca pedido acima do
  teto do modelo sem erro — precedente k3/glm).

## Medições (execução enx, 19–20/08/2026 — 6 Tasks, 5 sessões; padrões vistos 1× a confirmar)

- **Rodadas até APROVA por tipo:** contrato/backend largo = **1** (T3: 18 arquivos em 31 min;
  T4 medição: 28 min); tela/estado = **3** (T1, T2, T5, T6 — todas). O que custou as rodadas
  extras não foi código: foi PROVA e receita com lacuna.
- **Segue receita literal: SIM, até demais** (visto 2×). Lacuna na receita ele preenche com a
  escolha errada em vez de perguntar ("levante a lista ao vivo" sem QUANDO → sonda no mount).
  Receita fechada ele aplica rápido: correção de 1 bloqueador em **16 min** (10+/4−); noutra,
  aplicou a correção de método do revisor incluindo a asserção negativa (7 linhas de código,
  138 de teste).
- **Contexto: ~300–450k por Task** (janela 1M; thinking clampa em xhigh). **Uma Task longa por
  sessão**: 5 sessões em 6 Tasks, 4 trocas, todas por contexto, **nenhuma custou rodada** — a do
  meio de correção fechou em 16 min porque receita e parecer moram em arquivo.
- **Custo:** ~US$0,12–0,20/Task (T1 medida: $0,12 em 37 min). Velocidade: 16–59 min por entrega.
- **Ponto fraco em padrão: PROVA visual/viva** (visto 3×) — descartou a leitura visual certa e
  provou por DOM um menu invisível (T1 r1); HTML estático como se fosse componente montado
  (T6 r1); mock onde o mundo real desmentia (T5 r3). Kick-off de Task visual pra ele carrega o
  palco por extenso: build → preview → confirmar bundle → capturar.
- **Não se auto-reporta ao cruzar o portão de contexto** (visto 1×: 552k/1M com ordem escrita de
  avisar em 500k — quem viu foi o revisor). Entre o REPROVA e o reporte ninguém mais olha: o
  árbitro confere a statusline dele após cada REPROVA despachado.
- **Mediu gates com o disco, não com o commit** (visto 1×: arquivo de teste fora do commit,
  números reportados do disco — pego pelo relato×repo do árbitro). Cobrar no kick-off: gates em
  série DEPOIS do commit, e `git status` limpo no reporte.
- Enxerga imagem: **SIM, visão própria** (usuário, 19/08/2026). Não usar `see` com ele.
- Obedece portão quando avisado (recusou abrir parecer acima do portão) e handoff limpo
  (2 aposentadorias sem rastro pendente).
