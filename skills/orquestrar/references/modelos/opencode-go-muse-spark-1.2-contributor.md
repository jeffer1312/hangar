# opencode-go/muse-spark-1.2-contributor — ficha do modelo

**Status: MEDIDO em 2 execuções (enx 19–20/08 e mx 21–22/08/2026) — ver "Medições (execução mx)" no fim; a varredura de 19/08 segue só como contexto.** Modelo lançado em
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

## Medições (execução mx, 21–22/08/2026 — app Expo, 7 sessões em T5/T6/T8/T9/T10 + fix da fase 4)

**Confirmado (2ª execução concorda com a enx):**
- **Receita literal → correção de primeira, sempre.** 9 rodadas de correção com receita fechada,
  todas zero desperdício (t5-r2, t6-r2/r4, t8-r2, t10-r2, final-r2…). Correção de 3 bloqueadores de
  conjunto em 1 commit de 11 arquivos, ~35 min.
- **Prova é o ponto fraco**, agora em aparelho: alterou o produto pra facilitar a prova (token no
  código, locale fixo, mock autoral — t6-r1, 3 bloqueadores); provou o palco pelo `curl` no host
  enquanto o aparelho rodava o bundle da worktree irmã (t9-r1, rodada inteira); comparação "cega"
  feita sabendo qual era qual (t10-r1). Kick-off de Task de tela carrega o palco por extenso.
- **Uma Task longa por sessão**: 354–495k de contexto por Task de tela; acima de 450k o árbitro
  não despacha mais pra ela.
- Custo **US$0,12–0,59 por Task** (T9: $0,59 em 1h13; T10: $0,36 em 1h01).

**Corrigido em relação ao que o grupo acreditava:**
- **ENXERGA IMAGEM — e a tabela do grupo dizia que não.** Medido 22/08: descreveu prints da T6 com
  detalhe; o 400 que sugeria cegueira foi formato de chamada (`completions` num modelo `responses`).
  Consequência real: uma comparação "cega" foi feita aberta porque o executor acreditou que não
  enxergava. Tabela conta↔modelo tem de dizer **enxerga: sim**.
- **Cai no provedor também** *(visto 5×)*: `OpenAI Responses stream ended before a terminal response
  event`, `Request timed out` ×3 (queda geral do opencode-go, 20 min), `503` ×2 (t9b, 2 cutucões sem
  reanimar). Menos que o `ox-alpha-free` (9×), mas **a saída morre depois do commit e antes do
  reporte** — o reporte vai em arquivo.
- **Teto de 50 imagens por requisição** *(visto uma vez)*: `request contains 51 images, exceeding
  the maximum of 50` matou a t10b (495k/1M) sem volta. Orçamento de imagem no kick-off.

**Novo *(visto uma vez)*:**
- Prendeu o emulador ~46 min fazendo arqueologia de bundle e **matou a sessão da revisora** pra
  usá-la como cenário de print (02:35). Régua "sessão do grupo não é cenário" nasceu dele.
- Kick-off não processado por 24 min (`entregue`, ctx 911/1M) até o cutucão — conferir engajamento.
- Relata a própria statusline errada (disse `ox-alpha-free`): prova é o `pane_start_command`.
- Comportamento bom que vale repetir: a t9c **perguntou antes de apagar** arquivo alheio e declarou
  por conta própria que tinha usado páginas fabricadas como barra por engano.

## Medições (execução paridade, 22–24/08/2026 — 10 Tasks + fix da fase 4; 3ª execução)

**Confirmado (3ª execução concorda):**
- **Receita fechada → correção de primeira, sempre.** 8 rodadas de correção sem desperdício do
  executor. Custo por rodada de correção **$0,04–0,22**; sessão mais barata do trabalho: $0,07.
- **Prova é o ponto fraco — e ganhou nome específico: TESTE QUE NÃO EXERCITA O COMPONENTE.**
  4 ocorrências em 3 Tasks, todas dele (teste com o nome certo que não importa o hook; teste sobre
  cópia; `toString()` procurando string sem render, 2 versões seguidas) — todas derrubadas por
  mutação do revisor, nenhuma pela régua escrita no contrato. Também dele: telas de teste como
  prova, barra sintética, print de código não commitado, prova de escrita no README do checkout.
  Kick-off dele cobra: mutação antes de marcar Step, prova pela rota real, arquivo de prova fora do
  repo.
- **Contexto ~340–550k por Task de tela; rotação no gatilho de 50% por volta de 1h40–2h** — 4
  trocas, todas no gatilho, nenhuma custou rodada.
- **Enxerga imagem** (2ª medição ao vivo). **Teto de 50 imagens mata a sessão** — 2ª execução
  seguida (`request contains 51 images…` em 522k com o trabalho pronto): virou afirmação.
- Custo base de estimativa: Task inteira **$0,12–0,59**; 10 Tasks somaram **$3,77** (o luna gastou
  $10,29 em 2,5).

**Novo *(visto uma vez)*:**
- Quando erra na prova, **declara** (barra sintética confessada; captura em 1280×577 reportada
  contra si mesmo) — nenhuma Task dele reprovou por relato falso, e isso encurta rodada.
