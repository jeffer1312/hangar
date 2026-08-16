# Claude Opus 5 — papéis: ÁRBITRO, 2º REVISOR, REVISÃO DA BRANCH

Primeira ficha: 15/08/2026, trabalho de 13 Tasks. Como árbitro (uma sessão, o trabalho todo) e como
revisor (três sessões, rotacionadas por contexto e por cota).

## Números

- **Janela 1M**, teto de trabalho 500k. É a folga que faz dele o modelo da **revisão da branch**,
  que lê o conjunto de uma vez.
- **Consumo como revisor:** Task de módulo ~130k na primeira rodada, ~60k nas seguintes.
  **Task de tela: ~160k**, com as duas comparações cegas refeitas.
- **Custo: é o caro do time.** Um dia de revisão numa conta estourou a cota de 5h e marcou US$ 23,77.
  Compare com US$ 0,05–0,15 por Task do revisor de outra família. **Não gaste Opus em Task de texto
  ou de teste.**

## Enxerga imagem: sim, nativamente

Lê o print direto, sem ferramenta externa. É o que o torna o revisor certo para **Task de tela** —
e o único que refaz comparação cega sem custo de tradução.

## Onde ele é bom

- **Acha o bloqueador sutil que os outros não veem**, e é o padrão, não o caso isolado: erro que
  nunca dispara porque compara texto de mensagem em vez de status; regra global de CSS comendo o
  componente em sete elementos; teste que passa porque fabrica um formato que a API nunca produz.
- **Prova por caminho independente** em vez de aceitar o relato — monkeypatch em runtime, mutação do
  identificador para ver o teste falhar, medição do DOM contra o vizinho real.
- **Levanta callers por conta própria** quando o kick-off pede, e discorda do inventário do executor
  com evidência.
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

## O que o kick-off precisa dizer por causa dele

- Nunca truncar comando que lista arquivo (`--stat`, `--name-only`, `status`).
- Medir o arquivo de regras antes de cada kick-off, e compactar no teto.
- Reportar `ctx` junto de cada entrega, como todo mundo.
