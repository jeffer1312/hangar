# `openai-codex/gpt-5.6-luna` — papéis: REVISOR e EXECUTOR

Primeira ficha: 15/08/2026 (revisor, 13 Tasks). Segunda medição: 22–24/08/2026 (paridade), nos
**dois** papéis — que corrigiu o número mais importante da ficha.

## Números — a janela mudou, e o custo é o assunto principal

- **Janela: 1M DECLARADO em `~/.pi/agent/models.json`** (`providers.openai-codex.modelOverrides`,
  era 550000) — declaração nossa, não promessa do servidor. **Medido 23/08/2026: o provedor aceitou
  até 645k sem cortar.** O "272k / teto 240k" da ficha de 15/08 era de outra configuração e **não
  vale mais**; sem o override, a régua velha volta a valer.
- **Consome 10–16× o modelo barato do time** (medido pelo estimado da statusline: $3,83 numa Task e
  $6,46 em duas rodadas, contra $0,13–0,40 do spark por Task inteira). A conta é **assinatura** —
  o que isso gasta de verdade é a **cota** da conta Codex e contexto, não fatura; o número em $ é
  só o proxy medido da proporção.
- **"Commite cedo, antes da prova visual" vale −190k por Task no mesmo modelo** (645k → 457k,
  T8 → T10, única diferença a régua). Vai no topo do kick-off dele.
- Estimativa: **uma sessão por Task, sem margem para rodada longa de correção** — e linha de custo
  própria quando ele está na rotação.
- **Cuidado com o id:** o mesmo `gpt-5.6-luna` existe no `openrouter` (pago por token) — só o
  `provider/id` completo distingue.

## Modo de falha característico: o turno morre DEPOIS de escrever, ANTES de enviar

**5 ocorrências em 22–24/08/2026** (1 parecer + 4 vereditos, todos lidos do disco pelo árbitro).
Nele, "escreve primeiro, avisa depois" não é boa prática — **é o que segura o trabalho**. Kick-off
dele diz: parecer/reporte em arquivo SEMPRE; canal falhou, não reenvie — o árbitro lê o disco.

## Como executor (1ª medição, 22–24/08/2026)

- **Acerta tudo que o contrato ESCREVE, sem a memória das Tasks anteriores:** hooks, i18n sem chave
  nova (45 reusadas), nada de dummy, move sem quebrar caller, TDD com mutação em worktree. 1072
  linhas na maior Task do plano sem nenhuma armadilha que já tinha custado rodada.
- **Erra onde régua nenhuma alcança** (campo numérico que clampa a cada tecla) — modelo novo erra
  fora do contrato; conferência mais dura exatamente ali.
- Consertou de primeira o teste-que-não-exercita que o modelo anterior errara duas vezes (render
  real + mutação matando) e **declara o que NÃO fez** em vez de inventar.

## Como revisor (2ª medição — confirma a 1ª)

- **Reproduz antes de afirmar** e acha o bloqueador fino que gate nenhum pega (cabeçalho duplicado
  com XMLs de antes/depois; teste falso derrubado por duas mutações). **Recusa relato como prova.**
- A régua *"harness fecha corrida determinística; não fecha fronteira externa"* é dele, e se pagou
  na primeira aplicação. Também dele: *"memoizar apaga o sintoma e deixa a armadilha"*.
- **Assume erro próprio na cara** (confirmado 2ª execução): "esta rodada é culpa da minha receita",
  com medição junto.
- Falha conhecida (confirmada): **a receita fecha o caso nomeado, não a família** — o mesmo defeito
  voltou por outra porta em rodadas seguidas; quem corta é o árbitro, apertando o critério de
  família por escrito (fechou em 1 rodada quando aplicado).

## O que o kick-off precisa dizer por causa dele

- Escrever em arquivo antes de qualquer envio (o turno morre depois de escrever).
- Commitar cedo, antes da prova visual; contar com uma sessão só.
- Reportar `ctx` junto de cada entrega; na receita, inventário completo, não exemplos.
