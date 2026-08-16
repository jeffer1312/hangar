# Claude Opus 5 — papéis: ÁRBITRO, 2º REVISOR, REVISÃO DA BRANCH

Primeira ficha: 15/08/2026, trabalho de 13 Tasks. Como árbitro (uma sessão, o trabalho todo) e como
revisor (três sessões, rotacionadas por contexto e por cota).

## Números

- **Janela 1M**, teto de trabalho 500k. É a folga que faz dele o modelo da **revisão da branch**,
  que lê o conjunto de uma vez.
- **Consumo como revisor:** Task de módulo ~130k na primeira rodada, ~60k nas seguintes.
- **Como revisor de Task de tela, 1M:** 234k na primeira rodada, **+136k** na segunda, **+80k** na
  terceira — a leitura inicial não se repaga (16/08/2026, uma sessão só). Comparação cega refeita
  por ele saiu da conta: a skill agora manda **não** refazer o protocolo do executor, e as 6 rodadas
  em que isso foi feito renderam 6 divergências e zero bloqueadores.
- **Como revisor de encerramento, 1M:** três sessões cobriram 8 pareceres em 4 Tasks (16/08/2026) —
  272k → 336k → 359k numa, 265k noutra depois de 3 pareceres, 240k na terceira depois de 2.
  **Parecer de delta: 60–90k. Revisão de conjunto de 5 commits com prova ao vivo em 4 larguras:
  ~240k.** Nenhuma compactou no meio do julgamento.
- **Custo: é o caro do time.** Um dia de revisão numa conta estourou a cota de 5h e marcou US$ 23,77.
  Compare com US$ 0,05–0,15 por Task do revisor de outra família. **Não gaste Opus em Task de texto
  ou de teste.**

## Enxerga imagem: sim, nativamente

Lê o print direto, sem ferramenta externa. É o que o torna o revisor certo para **Task de tela**:
a passada final sobre o print e a barra sai sem custo de tradução, e ele enxerga o conjunto de
estados de uma vez. **Refazer o protocolo cego do executor não é mais trabalho do revisor**
(`revisor.md`, "Você NÃO refaz o protocolo cego") — foi essa mudança que tirou a comparação cega da
conta dele.

## Onde ele é bom

- **Acha o bloqueador sutil que os outros não veem**, e é o padrão, não o caso isolado: erro que
  nunca dispara porque compara texto de mensagem em vez de status; regra global de CSS comendo o
  componente em sete elementos; teste que passa porque fabrica um formato que a API nunca produz.
- **Prova por caminho independente** em vez de aceitar o relato — monkeypatch em runtime, mutação do
  identificador para ver o teste falhar, medição do DOM contra o vizinho real.
- **Levanta callers por conta própria** quando o kick-off pede, e discorda do inventário do executor
  com evidência.
- **Prova por mutação sem ser mandado:** copiou o front pra fora do repo, tirou o guard e viu o
  teste novo cair sozinho (`PASS(6) FAIL(1)`); tirou o guard irmão e nada caiu — logo aquele ponto
  não tinha teste. É a técnica que separou teste que prova o cenário de teste decorativo, e virou
  régua (`revisor.md`).
- **Carimba a instância do nó pra provar ciclo de vida** (`dataset.<marca>` antes da ação, conferir
  depois) — a única técnica que distingue "reapareceu" de "nunca saiu" (16/08/2026).
- **Corrige a própria evidência e retira por escrito** conclusão que não se sustenta, sem ser
  perguntado — inclusive *"o executor está CERTO e eu estava ERRADO"*, e inclusive retirando a
  própria receita antes de o executor construir em cima dela. Quatro vezes em 15–16/08/2026: é
  padrão, não acaso.
- Como árbitro: aguenta o trabalho inteiro numa sessão só (13 Tasks, ~9h) sem rotacionar.

## Como ele falha

- **Trunca a própria conferência.** O árbitro reportou "6 arquivos" num commit de 8 porque usou
  `git show --stat | head -8`. A conferência relato×repo é a única coisa que só ele faz — e ali
  `head` é proibido.
- **Escreve régua demais.** Doze réguas num dia, sem tirar nenhuma: o arquivo que todo mundo lê foi
  de 2 páginas a 316 linhas. Daí o teto medido (`SKILL.md`, "O arquivo de regras tem TETO").
- **Não enxerga a espiral de dentro.** Nove rodadas na mesma família de defeito, cada uma
  justificada isoladamente, e quem cortou foi o usuário de fora. É o motivo da linha de desperdício
  e do gatilho de "duas rodadas fechando só o caso nomeado".
- **Vai ao navegador antes de ler as expressões lado a lado.** Numa revisão de branch gastou duas
  medições caçando o defeito pelo caminho errado; as três expressões da mesma derivada, termo a
  termo, apontavam o lugar em minutos (16/08/2026).
- **Escreve o número de passos da receita sem aplicar a mudança** — "2 linhas" que eram 3, porque
  contou os pontos que já conhecia em vez de rodar a verificação (16/08/2026).
- **Prescreve limite numérico contra sintoma de layout** com a medição da causa já na própria mão
  (16/08/2026, custou 2 commits), e **generaliza "o elemento sai do DOM" para "o componente
  desmontou"** ao receitar cleanup de ciclo de vida (corrigido pelo executor). As duas viraram a
  seção "A sua receita é hipótese sua" do `revisor.md`.

## O que o kick-off precisa dizer por causa dele

- Nunca truncar comando que lista arquivo (`--stat`, `--name-only`, `status`).
- Medir o arquivo de regras antes de cada kick-off, e compactar no teto.
- Reportar `ctx` junto de cada entrega, como todo mundo.
