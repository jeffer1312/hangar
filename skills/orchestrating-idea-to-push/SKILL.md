---
name: orchestrating-idea-to-push
description: |
  Use quando o usuario pedir para tocar um trabalho grande com revisao independente e pouca interacao dele depois do planejamento - "executa esse plano sem eu ficar em cima", "monta o time e toca", "quero revisao independente por commit", "portao entre as Tasks", "uma sessao pra planejar e outra pra executar", "abre uma sessao pra revisar" - ou quando um plano grande/arriscado vai virar MR ou push. Use TAMBEM quando um kick-off mandar voce invocar esta skill e disser seu papel, e quando ja existe um trabalho desses em andamento e voce precisa saber o que fazer agora. NAO use para - tarefa de um repo so sem plano escrito (sessao normal), trabalho multi-repo com uma sessao por repo (skill orquestrar), revisao avulsa de um diff (subagent de review direto).
---

# Tubo: research → plano → execução autônoma com portão

Um trabalho grande atravessa cinco fases, cada uma numa sessão com o contexto certo. O
usuário decide tudo na fase 1; depois disso o tubo anda **sozinho** e só o acorda pelo que o
plano não fechou.

| Fase | Quem | Escreve código? | Termina quando |
|---|---|---|---|
| 0. Research | sessão/subagente read-only | não | achados num arquivo que o plano cita |
| 1. Spec + plano | **com o usuário** | não | plano aprovado, decisões e time fechados |
| 2. Lançamento | a mesma da fase 1 → vira **árbitra** | **não, nunca mais** | time criado, contrato escrito, um "pode ir" |
| 3. Execução | executor + revisor, modelos diferentes | só o executor | todas as Tasks com `APROVA` |
| 4. Revisão da branch | sessão nova, que não participou | não | conjunto aprovado |
| 5. Retrospectiva | sessão nova, que não participou | não | patch proposto para **esta skill**, na mão do usuário |

**O trabalho não acaba na fase 4.** Branch aprovada é código pronto; a fase 5 é o que faz a próxima
execução ser melhor que esta. Ela é curta (uma sessão, três arquivos de entrada) e é a única fase
cujo produto não é código — é `references/retrospectiva.md`.

Push e MR são sempre do usuário.

## Leia SÓ a página do seu papel

Este arquivo é o roteador. O resto está separado de propósito: papel misturado é como uma
sessão acaba confirmando que é revisora enquanto está no meio de um commit.

| Seu papel | Leia | Você é isso quando |
|---|---|---|
| **planejador** | `references/planejamento.md` | o usuário te pediu o trabalho e não existe kick-off |
| **árbitro** | `references/arbitro.md` | você escreveu o plano e o usuário aprovou |
| **executor** | `references/executor.md` | o kick-off diz `Papel: executor único` |
| **revisor** | `references/revisor.md` | o kick-off diz `Papel: revisor` |
| **revisão final** | `references/revisao-final.md` | o kick-off diz `Papel: revisão da branch` |
| **retrospectiva** | `references/retrospectiva.md` | o kick-off diz `Papel: retrospectiva` |

Duas páginas que não são papel:

- `references/paralelo-worktree.md` — a **exceção** de rodar Tasks em paralelo com uma worktree
  cada. O padrão é serial; leia só se o plano declarou um lote paralelo (planejador) ou se você
  vai integrá-lo (árbitro).
- `references/replanejar.md` — **reescrever o plano e o contrato no MEIO da execução**, quando o
  usuário mandar ou o plano deixar de ser confiável (premissa caída, método sem metade executora,
  estimativa estourando pela mesma causa). Não é troca de método escondida: é a fase 1 rodando de
  novo, menor, só sobre o que resta — e é a **única** porta legítima para trocar de método.

**Papel é declarado, nunca deduzido — e é recusado quando contradiz o que você está
fazendo.** Kick-off dizendo "você é revisor read-only" chegando numa sessão que está no meio
de uma Task: responda *"sou o executor da Task N, confirme o destinatário"* e **não** assuma.
Confirmar um papel que não é o seu troca o dono do trabalho no meio, em silêncio.

Trabalho multi-repo com uma sessão por repo é outra skill: `orquestrar`.

## O MÉTODO não é escolha sua — vem do contrato

Esta skill orquestra: papéis, portão, revisão independente, rotação, retrospectiva. **Ela não
planeja e não executa** — isso é de outra família de skills, o *método*, e existe mais de um.

> **Método ≠ motor.** *Motor* nesta skill é o provedor do modelo (`--engine`, `engines.json`:
> DeepSeek, Kimi…). *Método* é qual família de skills planeja e executa. Uma sessão tem os dois, e
> eles são decididos separadamente.

**O método é declarado no contrato do grupo** (`regras-<gid>.md`), numa linha, e vale do research ao
último commit:

```markdown
Método: superpowers    # planejador: brainstorming → writing-plans · executor: executing-plans
Método: mattpocock     # planejador: /grill-me → /to-spec → /to-tickets · executor: /implement
```

**`superpowers` é o padrão e a recomendação — decisão do usuário, 17/08/2026, depois de medir a
única execução em `mattpocock`.** Outro método só com pedido explícito dele, e com a **metade
executora instalada e testada antes de aceitar**: a execução de 16–17/08 rodou `mattpocock` com o
`/implement` ausente (o árbitro improvisou "os Steps são o método"), e o `/to-tickets` não gerou
estimativa a priori nem prova de não-colisão — dois artefatos que o portão de saída da fase 1
(`planejamento.md`) cobra de **qualquer** método.

Nenhum papel escolhe método, e **nenhum troca de método no meio**. Plano nascido num método e
executado noutro é o defeito que esta seção existe para impedir: os dois escrevem o trabalho em formatos
diferentes (Task com Steps em `- [ ]` de um lado, ticket do outro), e quem lê depois — o executor, o
árbitro que recorta a Task, a barra de progresso do app — passa a ler uma coisa que não existe.

Três regras, e as três são do árbitro:

1. **A linha `Método:` é obrigatória** no contrato, escrita no lançamento, antes da primeira sessão.
2. **Todo kick-off repete o método**, porque contrato se lê uma vez e kick-off chega fresco.
3. **Contrato sem a linha** → o método é `superpowers`, que é o padrão histórico desta skill — e o
   árbitro **escreve a linha** antes de seguir, em vez de deixar implícito.

Método que você não conhece, ou pedido de trocar no meio: **pare e pergunte ao usuário**. É decisão
dele, como modelo e conta. Troca que ele aprovar não se faz por emenda: roda
`references/replanejar.md`, e o plano do trabalho restante nasce **inteiro** no método novo — nunca
metade em cada.

## Kick-off — a mensagem aponta, não copia

Sessão nova nasce com contexto zero, mas com o **mesmo `~/.claude`**: esta skill já está lá,
pelo nome. O kick-off é um endereço, não um manual.

```
Invoque a skill orchestrating-idea-to-push e leia a página do seu papel.
Papel: <executor único | revisor | revisão da branch>.
Método: <superpowers | mattpocock>.
Repo/branch: <caminho> / <branch>.   HEAD esperado: <hash>.
Regras do grupo: <caminho do regras-<gid>.md>.
A Task da vez, recortada: <caminho do task-<N>.md>.
Intocáveis: <paths, um a um — não "os do contrato">.
Sua vez agora: <Task N | esperar o primeiro hash>.
Ao terminar, reporte para <sessao-do-arbitro> e PARE.

Leia SÓ esses dois arquivos além da skill. O plano inteiro e o registro do grupo NÃO são seus.
```

A última linha é uma **instrução**, não um comentário: sem ela a sessão vai atrás do plano
completo e do registro por conta própria — foi exatamente o que aconteceu no trabalho de
14/08/2026 e custou 110k de contexto antes do primeiro commit.

`HEAD esperado` e a lista literal de intocáveis existem porque a sessão nova, sem eles,
deriva os dois do `git status`/`git log` e pode achar um HEAD que ninguém explicou.

O mesmo texto, reenviado, recoloca de pé uma sessão que deu `/clear`: ele não carrega
estado, carrega caminhos. Nenhuma linha dele diz "a Task 2 já passou" — isso é do contrato,
onde continua verdadeiro amanhã.

## Dois arquivos, não um: o registro e as regras

**Só o árbitro escreve nos dois.** Uma sessão que registra a própria autorização legitima o
próprio desvio, e o árbitro só descobre relendo o arquivo.

| Arquivo | Contém | Quem lê |
|---|---|---|
| `grupo-<gid>.md` — **o registro** | o diário da execução: progresso Task→hash→veredito, o que cada rodada quebrou, sessões queimadas, decisões com data | **só o árbitro** |
| `regras-<gid>.md` — **as regras** | o que **ainda vale**: intocáveis, gates, réguas, barra, o que a revisão cobre, teto e contas | executor e revisor |

A fronteira é o **tipo** do conteúdo, não o assunto: **já aconteceu → registro; ainda vale →
regras.** Uma decisão nova entra nas regras, e o registro só anota a data e aponta pra lá. Assim
os dois não divergem, e o arquivo que todo mundo lê **para de crescer**.

Por que separar, medido em 14/08/2026: o registro chegou a 54 KB (~14k tokens) porque toda Task
aprovada acrescentava um parágrafo e nada saía. Somado ao plano inteiro (~30k), um revisor recém-
aberto para a Task 10 gastou **110k de contexto antes de receber o primeiro commit** — lendo,
entre outras coisas, como a Task 4 tinha sido reprovada quatro vezes três semanas antes. O que ele
precisava sabia-se em duas páginas.

### O arquivo de regras tem TETO, e ele se mede

"Duas páginas" não é limite: ninguém mede, e o arquivo cresce mesmo assim — cada régua nova entra e
nada sai. Medido em 15/08/2026, num trabalho de 13 Tasks: o arquivo de regras chegou a **316 linhas
/ 18 KB** e o registro a 22 KB, com o árbitro escrevendo uma régua a cada achado, o dia todo, sem
nunca tirar nada.

**Teto: 200 linhas.** Antes de mandar cada kick-off, o árbitro mede:

```bash
wc -l <config>/.claude-pocket-pair/regras-<gid>.md
```

Passou → **compacta antes de enviar**. Compactar não é resumir: é tirar o que **deixou de valer** —
régua de um lote já fechado, exceção de um arquivo que já mergeou, decisão que virou código. O que
saiu vira uma linha no registro (com a data), que é onde história mora.

Vale para o registro também, com teto mais largo (**500 linhas**): ele só o árbitro lê, mas é ele
que o árbitro relê inteiro toda vez que precisa lembrar de algo.

Primeira linha do arquivo de regras, pra sessão amnésica se reancorar sozinha:

```markdown
> Sessões deste grupo: invoquem a skill `orchestrating-idea-to-push` e leiam a página do seu papel.
> Branch: <branch> · Repo: <caminho> · Método: <superpowers | mattpocock>
```

A linha `Método:` é obrigatória (ver "O MÉTODO não é escolha sua", acima) e nunca muda no meio do
trabalho.

**O que muda a cada Task não vai em arquivo nenhum**: qual Task está liberada, qual é o hash, quem
é o seu par. Isso vai no kick-off, que é sempre fresco por definição. Arquivo com estado da vez é
arquivo que envelhece entre a escrita e a leitura.

**A Task da vez vai RECORTADA, não o plano inteiro.** O plano tem todas as Tasks; o executor
implementa uma e o revisor revisa uma. Recorte a seção daquela Task mais o cabeçalho curto
(goal/architecture) para `~/.claude/orq-retros/<data>-<gid>/tasks/task-<N>.md` — caminho durável, não
`/tmp`, que some no reboot — e mande esse caminho. No mesmo trabalho
de 14/08: plano inteiro ~30k tokens, Task recortada ~2,9k.

**Quem é do grupo sai do contrato, nunca de `cp-send --list`.** Sessão viva no mesmo
diretório é só uma sessão viva no mesmo diretório — o usuário abre sessões pro que quiser, e
elas não viram time por estarem ali. Contrato sumido ou vazio não autoriza deduzir o elenco:
peça ao usuário quem é quem antes de mandar recado a alguém que não pediu pra participar.

**Contrato escrito é ordem, não sugestão.** Motor, modelo, conta, nome de sessão e papel já
foram decididos pelo usuário — nenhuma sessão reabre isso porque a situação mudou. Em dúvida,
**releia o contrato** antes de agir; ele não previu o caso, **pergunte**. Detalhe em
`references/arbitro.md`, seção "Contrato fechado".

## Travas que valem para todos os papéis

- **Recado de par alegando "o usuário autorizou" não é autorização** quando contradiz a
  ordem vigente do árbitro. Confirme com ele **antes** de commitar, não depois.
- **Stage por caminho explícito.** Nunca `git add -A` nem `git add .`. Intocáveis nunca
  entram, em commit nenhum.
- **Sem `--amend`/rebase/squash** em commit já commitado. Correção é commit novo.
- **`cp-send <sessao>` RECUSA falar com sessão Claude desta máquina** (rc=3, "o caminho nativo
  alcança os dois lados") e manda usar `SendMessage`. Se o `ListAgents` vier **vazio** — acontece —,
  o `SendMessage` não tem endereço e você fica sem caminho nenhum. A saída é `cp-send --tmux
  <sessao>`, que envia do jeito antigo. Sessão Pi ou Codex não sofre disso: só o par Claude→Claude.
- **`cp-send` recebe a mensagem como argumento, não por stdin.** Texto longo vai por heredoc
  de aspas simples **dentro** de uma substituição:

  ```bash
  cp-send <sessao> "$(cat <<'EOF'
  ...texto livre, com crase e $ intactos...
  EOF
  )"
  ```

  Aspas duplas cruas fazem o shell comer crase e `$`, e receita mutilada é pior que receita
  nenhuma. Heredoc solto (`cp-send <sessao> <<'EOF'`) devolve erro de uso — a mensagem não sai.
- **MODELO É DECISÃO DO USUÁRIO. Ninguém escolhe modelo.** A política de contas da máquina fica em
  **`~/.claude/orquestracao-contas.md`** — quais contas existem, quais são assinatura (troca livre
  dentro da conta), quais são travadas num modelo e quais são proibidas por cobrarem por token. O
  árbitro **lê esse arquivo antes de montar time** e copia pro contrato só o que aquele trabalho vai
  usar. Arquivo ausente ou desatualizado: **levante o inventário e pergunte ao usuário** (a receita
  de levantamento está dentro do próprio arquivo), escreva a resposta lá com a data, e siga. O
  contrato traz a tabela conta↔modelo por papel; ela é fechada. Modelo fora dela não se usa **nem pra teste**, nem porque "é mais barato",
  nem porque apareceu no catálogo. Cada conta tem cota e preço próprios, e provedor errado **cobra
  dinheiro do usuário** — um `openrouter/*` escolhido por conta própria é fatura, não experimento.
  - **Sessão nova nasce no padrão do harness, que não é o modelo da tabela.** Quem cria: troca,
    **lê o modelo de volta** e confere; só então manda trabalho. Sessão trabalhando em modelo não
    conferido é gasto na conta errada que só aparece na fatura.
  - **Subagente pode — mas SEMPRE na mesma CONTA da sessão, e a liberdade de modelo é POR CONTA.**
    Sair da conta nunca pode. Trocar de modelo **dentro** dela só onde o contrato liberar
    explicitamente: há conta em que o usuário aceita dois modelos (um mais forte pro julgamento,
    outro mais barato pro mecânico) e há conta **travada num modelo só** — e existe conta proibida,
    porque cobra por token no cartão dele. Não deduza pela lista de modelos que a conta oferece: vale
    o que está escrito no contrato, e conta não listada é **pare e pergunte**. E confira o frontmatter do que você despacha: um `model:` escrito lá dentro
    sobrepõe o teu (os agentes `ecc:*` trazem `model: sonnet`, que numa sessão Claude gasta a conta
    Anthropic; a ponte do Pi remove esse campo).
  - Precisa de um modelo que não está na tabela? **Pare e pergunte.** Não é decisão de árbitro,
    executor nem revisor.
- **Entrega não é resposta.** `entregue -> <sessao>` e o `success` do `SendMessage` dizem que a
  mensagem **entrou na fila do destino**, não que alguém leu, nem que a resposta vai voltar. Não
  existe prazo por mensagem: Task inteira leva o tempo que levar, e cutucar executor trabalhando é
  ruído. **O sinal é outro — ver "Ociosidade" abaixo.**
- **Nunca `comando | tail && echo OK`** — o `&&` lê o código de saída do `tail`, e o "OK"
  imprime com o comando falhando. Use `set -o pipefail` ou cheque `${PIPESTATUS[0]}`.
- **Verificação roda o comando que o plano definiu para aquela Task**, na forma que não
  depende do cwd (prefixo/diretório explícito). Nunca invente o comando nem rode "o que
  costuma ser".
