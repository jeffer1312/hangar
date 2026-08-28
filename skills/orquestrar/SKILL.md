---
name: orquestrar
description: |
  Use quando o usuario pedir para tocar um trabalho grande com revisao independente e pouca interacao dele depois do planejamento - "executa esse plano sem eu ficar em cima", "monta o time e toca", "quero revisao independente por commit", "portao entre as Tasks", "uma sessao pra planejar e outra pra executar", "abre uma sessao pra revisar" - ou quando um plano grande/arriscado vai virar MR ou push. Use TAMBEM quando um kick-off mandar voce invocar esta skill e disser seu papel, e quando ja existe um trabalho desses em andamento e voce precisa saber o que fazer agora. Serve a trabalho de um repositorio ou de varios - o que a define e o trabalho quebrado em Tasks, o portao entre elas e o revisor independente, nao a quantidade de checkouts; para varios repos ela combina as interfaces antes e abre uma sessao por repo. Nao exige plano em formato nenhum - vale com plano do superpowers, plano de outro metodo, plano que o usuario escreveu a mao, ou nenhum plano (ela escreve um plano de orquestracao curto apontando pro material dele). NAO use para - tarefa pequena que uma sessao so resolve, revisao avulsa de um diff (subagent de review direto).
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
| 3. Execução | executor + revisor, **sessões separadas** | só o executor | todas as Tasks com `APROVA` |
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

**Mais de um repositório não é impedimento.** O trabalho vive num plano com Tasks e num contrato
com papéis; nada disso está preso a um checkout. Uma Task pode tocar outro repo, e a sessão daquele
papel nasce lá — quem diz o repo de cada uma é o contrato, na linha do cabeçalho (`Repo: <um> (+
<outro> a partir da T13)`). O que continua valendo é o **um escritor por árvore**: dois executores
no mesmo checkout ao mesmo tempo, não; em checkouts diferentes, sim.

Quando o trabalho atravessa repositórios, três coisas mudam de peso:

**As interfaces se combinam ANTES de abrir as sessões.** O que atravessa a fronteira — rota,
payload, evento, tipo — vira linha do contrato, numa seção `## Interfaces combinadas`, antes de
qualquer um escrever código. Sem isso dois repos entregam pontas que não encaixam, e o defeito só
aparece na integração, quando as duas Tasks já passaram pelo portão.

**Subagente lê, sessão escreve.** Subagente serve pra explorar o outro repo, rastrear um fluxo,
achar o caller. Editar fora do cwd da sessão exige uma sessão de verdade naquele repo — não porque
o subagente não conseguiria, mas porque trabalho que ninguém vê no terminal não dá pra acompanhar
nem interromper.

**Sessão em outra máquina (`servidor::sessao`) não entra em grupo.** Pareamento cross-server não
existe; ela recebe recado 1:1 e o que ficar combinado tem de ser registrado no contrato local, à
mão, senão some.

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
(`planejamento.md`) cobra de **qualquer** método. É um caso do princípio geral — skill invocada
roda inteira (ver "Travas que valem para todos os papéis").

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

## A SKILL DE DOMÍNIO, quando existe, é quem manda no trabalho

Não confunda com o método, que é a seção acima. **Método** é quem planeja e quem executa
(`superpowers`, `mattpocock`). **Skill de domínio** é a que descreve *o trabalho em si*, passo a
passo, porque alguém já fez aquilo dezenas de vezes: portar uma tela, criar um módulo, montar uma
migração. Ela não planeja nem executa nada — ela diz o que tem de acontecer, e em que ordem.

**Quando existe uma, o plano não a repete: ele a instancia.** Cada Task cita o passo da skill que
executa, na mesma ordem dela, e o executor relê a skill antes de começar. Isso não é invenção
nossa: uma skill de domínio real deste repositório já traz, no meio dela, a descrição de como deve
ser instanciada por um plano — ela foi escrita prevendo isto.

O contrato ganha uma linha, escrita no lançamento, e ela é obrigatória mesmo quando a resposta é
"nenhuma":

```markdown
Skill de domínio: portar-tela    # passos 1–9; em conflito com o plano, vale a skill
Skill de domínio: nenhuma
```

**Duas conferências, antes da Task 1** (o portão de saída da fase 1 as cobra, em
`references/planejamento.md`):

1. **Alguma Task faz o que a skill já faz por dentro?** Se faz, essa Task **não existe** — o que
   existe é a exigência da evidência daquele passo dentro da Task que o contém. Medido em
   25/08/2026: o passo que gera o contrato de backend virou uma Task separada no fim de um plano de
   19; duas telas passaram pelo portão sem ele, o backend não pôde começar em paralelo, e o portão
   nunca cobrou o passo, porque "já existe uma Task para isso". Quem viu foi o usuário, na décima
   Task.
2. **Sobrou passo da skill sem dono?** Passo que nenhuma Task cita é passo que ninguém vai rodar.
   E olhe também o que a skill **não** tem: uma skill de porte de tela pode terminar no último passo
   sem nunca mandar testar a tela inteira — trabalho montado só com os passos dela nasce sem
   verificação de conjunto, e isso é buraco do plano, não da skill.

**A skill de domínio não se altera para caber neste trabalho.** Outras pessoas usam. O que se ajusta
é o plano e o contrato; a skill se lê como está. Passo que não se aplica é decisão do usuário, nunca
do árbitro — é o mesmo caso de "skill invocada roda inteira", nas travas abaixo.

## Kick-off — a mensagem aponta, não copia

Sessão nova nasce com contexto zero, mas com o **mesmo `~/.claude`**: esta skill já está lá,
pelo nome. O kick-off é um endereço, não um manual.

```
Invoque a skill orquestrar e leia a página do seu papel.
Papel: <executor único | revisor | revisão da branch>.
Método: <superpowers | mattpocock | nenhum — o plano é o do usuário>.
Skill de domínio: <nome | nenhuma>.
Repo/branch: <caminho> / <branch>.   HEAD esperado: <hash>.
Regras do grupo: <caminho do regras-<gid>.md>.
A Task da vez, recortada: <caminho do task-<N>.md>.
Intocáveis: <paths, um a um — não "os do contrato">.
Lições que valem nesta Task: <coladas aqui, 3 ou 4, não o caminho do arquivo>.
Sua vez agora: <Task N | esperar o primeiro hash>.
Ao terminar, reporte para <sessao-do-arbitro> e PARE.

Leia SÓ esses dois arquivos além da skill. O plano inteiro, o registro e o arquivo de lições
NÃO são seus.
```

As **lições vão coladas, não como caminho** — é a única coisa do kick-off que se copia em vez de
apontar, e por um motivo: o arquivo inteiro não serve a esta Task, e quem sabe quais servem é o
árbitro. Mandar o caminho faria a sessão ler tudo, que é exatamente o custo que a separação dos
três arquivos existe para evitar.

A última linha é uma **instrução**, não um comentário: sem ela a sessão vai atrás do plano
completo e do registro por conta própria — foi exatamente o que aconteceu no trabalho de
14/08/2026 e custou 110k de contexto antes do primeiro commit.

`HEAD esperado` e a lista literal de intocáveis existem porque a sessão nova, sem eles,
deriva os dois do `git status`/`git log` e pode achar um HEAD que ninguém explicou.

O mesmo texto, reenviado, recoloca de pé uma sessão que deu `/clear`: ele não carrega
estado, carrega caminhos. Nenhuma linha dele diz "a Task 2 já passou" — isso é do contrato,
onde continua verdadeiro amanhã.

## Três arquivos, cada um com um leitor: o registro, as regras e as lições

**Só o árbitro escreve nos três.** Uma sessão que registra a própria autorização legitima o
próprio desvio, e o árbitro só descobre relendo o arquivo.

| Arquivo | Contém | Quem lê |
|---|---|---|
| `~/.claude/orq-retros/<data>-<gid>/registro.md` — **o registro** | o diário da execução: progresso Task→hash→veredito, o que cada rodada quebrou, sessões queimadas, decisões com data | **só o árbitro** |
| `regras-<gid>.md` — **as regras** | o combinado do trabalho, que quase não muda: quem é quem, intocáveis, gates, método, branch, barras, o que a revisão cobre, contas | executor e revisor, **inteiro** |
| `~/.claude/orq-retros/<data>-<gid>/licoes.md` — **as lições** | as réguas que a execução vai fixando, uma por bloco, com a data e a prova | **ninguém lê inteiro** — o árbitro cola no kick-off só as que servem àquela Task |

Existe um quarto, que nenhuma sessão lê: o `eventos.jsonl`, uma linha por acontecimento, que
alimenta as telas do app e a retrospectiva. Ele é do árbitro e está descrito em
`references/arbitro.md` — por isso aquela página fala em **quatro** arquivos e esta, em três.

> **O registro e as lições moram no diretório durável do trabalho, que nada gerencia.**
> `<config>/.hangar-pair/` é do backend: ele apaga o `grupo-<gid>.md` junto com o grupo (medido
> 22/08/2026, quando um executor matou a última sessão viva e o diário de 10h sumiu). As **regras**
> continuam lá — é o caminho que o app mostra ao time.

A fronteira entre os três é o **tipo** do conteúdo, não o assunto:

- **já aconteceu → registro** (a Task 4 foi reprovada quatro vezes);
- **é o combinado deste trabalho → regras** (o executor é a sessão X, `api.py` é intocável);
- **é uma régua que nasceu no meio e vale daqui pra frente → lições** (o `adb logcat` pendura, use
  o `-d`).

**As regras quase não mudam depois do lançamento; as lições crescem o trabalho inteiro.** É essa
separação que resolve o problema real: régua nova é o produto normal de uma execução — toda rodada
que reprova produz uma —, e enfiar todas elas no arquivo que toda sessão lê inteiro fazia esse
arquivo dobrar de tamanho até alguém ter de jogar coisa fora.

**Lição não se joga fora, e não tem teto.** Ela é escrita uma vez, com data e prova, e fica. O que
tem teto é **quanto disso vai num kick-off**: o árbitro escolhe as que valem para aquela Task e
cola no texto. Uma lição de Task de tela não vai no kick-off de uma Task de backend, e não é por
ela estar velha — é por ser de outro assunto.

Por que isto existe, medido em duas datas. Em 14/08/2026 o registro chegou a 54 KB (~14k tokens)
porque toda Task aprovada acrescentava um parágrafo e nada saía; somado ao plano inteiro (~30k), um
revisor recém-aberto para a Task 10 gastou **110k de contexto antes de receber o primeiro commit**.
Em 15/08/2026, num trabalho de 13 Tasks, o arquivo que o time lê chegou a **316 linhas / 18 KB**
porque o árbitro escrevia uma régua a cada achado, o dia todo. O remédio da época era um teto de
200 linhas com compactação antes de cada kick-off — e ele **falhou de um jeito específico**: numa
execução de 28/08/2026 uma régua foi apagada por ser rara e o caso dela reapareceu **uma hora
depois**. Jogar régua boa fora para caber é o defeito, não a solução.

**O registro tem teto (500 linhas) e no teto ele ARQUIVA** — move o bloco mais antigo **inteiro**
para um arquivo irmão no mesmo diretório durável (`registro-tasks-1-N.md`) e deixa um ponteiro no
lugar. Nunca resume: ele é a matéria-prima da fase 5, e resumir ali destrói o que ela vai destilar.
Medido em 23/08/2026: 308 linhas arquivadas, nada perdido, e a retrospectiva leu os dois.

Primeira linha do arquivo de regras, pra sessão amnésica se reancorar sozinha:

```markdown
> Sessões deste grupo: invoquem a skill `orquestrar` e leiam a página do seu papel.
> Branch: <branch> · Repo: <caminho>
> Método: <superpowers | mattpocock | nenhum> · Skill de domínio: <nome | nenhuma>
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

**Quem é do grupo sai do contrato, nunca de `hangar-send --list`.** Sessão viva no mesmo
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
- **Skill invocada dentro de uma Task roda INTEIRA.** Metade ausente na máquina, passo que não se
  aplica, passo que falhou → **pare antes do commit**, e nunca improvise um equivalente nem entregue
  o que faltou como "pendência". **Dispensar passo de skill é do usuário, não do árbitro** — ele só
  cumpre dispensa já dada (no plano, no contrato, ou regra permanente dele) e leva o resto pra
  decisão. Detalhe em `references/executor.md`.
- **Régua se escreve como PRINCÍPIO; o caso medido entra como prova.** Regra que nomeia um caso —
  uma skill, uma ferramenta, um arquivo, uma data — deixa o resto do espaço sem regra, e o resto do
  espaço costuma ser exatamente o cenário da próxima vez. Vale para tudo que esta skill produz:
  receita do revisor, régua nova do árbitro, patch da fase 5. Antes de escrever qualquer uma,
  pergunte: **"e quando não for esse caso?"** Resposta não coberta → o que você escreveu é a
  instância, não a regra: enuncie a condição e mova o caso pra prova. Isso **não** afrouxa a
  exigência de medição — o princípio vem na frente **e** o caso medido vem junto, com data e número;
  as duas coisas, sempre. Detalhe em `references/revisor.md` (receita) e
  `references/retrospectiva.md` (patch).
- **Ferramenta de fora — skill, subagente, comando — passa por TRÊS perguntas, e são sempre as
  três:** (1) **existe com esse nome?** Pode ter virado comando em vez de skill, mudado de nome, ou
  não estar instalada nesta conta (plugin é por diretório de configuração, e uma sessão em conta
  secundária vê outra lista). (2) **Serve ao FLUXO?** Ferramenta que monta o diff a partir de
  mudanças **não commitadas** ou de um PR não serve a um portão que revisa **commit já feito** em
  branch local: o diff chega vazio e a saída sai bonita e oca. (3) **Serve aos ARQUIVOS desta
  Task?** Revisor por linguagem costuma montar o próprio diff com filtro de extensão; filtro que
  não pega os arquivos tocados devolve "nada a apontar" sobre código que ele **não leu**, e ausência
  vira falsa evidência — o conserto é passar os caminhos explicitamente no pedido. Reprovou em
  alguma: registre **por que não serve**, numa linha, e isso vale tanto quanto a lista do que usar.
  E **silêncio de ferramenta só conta se você souber o que ela leu.**
- **Sem `--amend`/rebase/squash** em commit já commitado. Correção é commit novo.
- **Escreva primeiro, avise depois — sempre nessa ordem.** Parecer, reporte e receita nascem como
  **arquivo** no diretório durável do trabalho **antes** de qualquer envio, e a mensagem carrega o
  **caminho**, nunca o conteúdo. Não é formatação: é o que faz o trabalho sobreviver ao canal —
  medido em 17–18/08/2026, quatro modos de falha diferentes no mesmo canal em 48h, e nas quatro nada
  se perdeu porque o arquivo já existia. É também o que torna impossível a mensagem mutilada: texto
  que vai como caminho não tem crase para o shell comer.
- **A escada de transporte, em ordem, e o degrau seguinte só depois de o anterior falhar:**
  **olhe o pane do destinatário** (overlay/menu aberto recusa digitação, e é o que o backend reporta
  como "sessão indisponível") → `SendMessage` → `hangar-send --tmux <sessao>` → `tmux send-keys` no
  pane. `hangar-send <sessao>` **recusa** falar com sessão Claude desta máquina (rc=3, "o caminho nativo
  alcança os dois lados") e manda usar `SendMessage`; com o `ListAgents` **vazio** — acontece — o
  nativo não tem endereço, e sobra o `--tmux`. Sessão Pi ou Codex não sofre disso: só o par
  Claude→Claude. Recusa **de quem recebe** não se contorna por outro transporte; recusa **da
  ferramenta**, sim — e o degrau usado vai no reporte, porque canal quebrado que ninguém registra é
  o mesmo susto duas vezes.
- **`hangar-send` recebe a mensagem como argumento, não por stdin.** Texto longo vai por heredoc
  de aspas simples **dentro** de uma substituição:

  ```bash
  hangar-send <sessao> "$(cat <<'EOF'
  ...texto livre, com crase e $ intactos...
  EOF
  )"
  ```

  Aspas duplas cruas fazem o shell comer crase e `$`, e receita mutilada é pior que receita
  nenhuma. Heredoc solto (`hangar-send <sessao> <<'EOF'`) devolve erro de uso — a mensagem não sai.
- **Escolher o time é OFERTA, não obrigação — e a oferta se faz UMA vez.** Montar a tabela
  conta↔modelo por papel trava quem tem uma conta só: não há o que escolher, e a pessoa fica parada
  numa pergunta sem resposta possível (reclamação de um usuário de fora, 2026). O planejador
  pergunta uma vez — *"quer escolher o time, ou seguimos no padrão?"* — e **qualquer resposta
  destrava o trabalho**:
  - **quer escolher** → a receita inteira de `references/planejamento.md` ("O time é saída do
    planejamento"): inventário levantado, candidatas propostas, ele decide.
  - **não quer, ou não respondeu** → **padrão, na conta que já está em uso**: executor em Opus
    esforço `medium`; revisor, árbitro e revisão final em Opus esforço `high`. A tabela nasce
    preenchida assim, com a data e a palavra `padrão` na linha, e o trabalho começa.

  **Isto não é uma sessão escolhendo conta.** A conta continua sendo a que ele já está usando — o
  padrão só preenche modelo e esforço dentro dela. Sair da conta em uso, ou entrar em conta que
  **cobra por token**, continua proibido sem ele mandar: é a fatura dele, e nenhum padrão
  automático pode chegar lá. Padrão escrito na tabela é decisão dele por omissão, e ele troca
  quando quiser, pelo modal ou pedindo.
- **MODELO É DECISÃO DO USUÁRIO. Ninguém escolhe modelo fora do padrão acima.** A política de contas da máquina fica em
  **`~/.claude/orquestracao-contas.md`** — quais contas existem, quais são assinatura (troca livre
  dentro da conta), quais são travadas num modelo e quais são proibidas por cobrarem por token. O
  árbitro **lê esse arquivo antes de montar time** e copia pro contrato só o que aquele trabalho vai
  usar. Arquivo ausente ou desatualizado: **levante o inventário e pergunte ao usuário** (a receita
  de levantamento está dentro do próprio arquivo), escreva a resposta lá com a data, e siga. O
  contrato traz a tabela conta↔modelo por papel (`## Quem é quem` do `regras-<gid>.md`,
  cabeçalho fixo `| papel | sessão | provider | conta | modelo | esforço |` — ou de 7 colunas com
  `vez`, quando o papel reveza entre contas por Task (`references/arbitro.md`, "Abrir uma sessão")
  — lida e gravada pelo modal Orquestração do app, então célula com prosa não é lida); ela é fechada. Modelo fora dela não se usa **nem pra teste**, nem porque "é mais barato",
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
