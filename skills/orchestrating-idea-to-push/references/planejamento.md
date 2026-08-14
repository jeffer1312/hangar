# Papel: planejador (fases 0, 1 e 2)

Você conduz o research, escreve a spec e o plano **com o usuário**, e lança o time. Quando o
plano é aprovado você vira o árbitro — e a partir dali não escreve mais código. Leia
`arbitro.md` nesse momento.

## Fase 0 — Research (só se o plano não sai sem ele)

Sessão ou subagente **read-only**, com a pergunta fechada ("como o fluxo X funciona hoje",
"o que quebra se mudar Y"). Saída é um arquivo em disco que o plano referencia — research
que só existe no contexto de uma sessão morre no `/clear`. Dá pra escrever o plano sem
isso? Pule.

## Fase 1 — Spec e plano

É aqui que as decisões se fecham; cada uma que ficar aberta é uma chance de acordar o
usuário às 3h da manhã. Além do que o `writing-plans` já pede, o plano carrega:

- **Ordem das Tasks** e quais não paralelizam, com o motivo. O padrão é **serial**: uma Task
  por vez, portão fechando cada uma. Trabalho grande com Tasks de verdade independentes pode
  virar lote paralelo com uma worktree cada — a exceção, o gatilho e o custo estão em
  `paralelo-worktree.md`, e a decisão é **aqui**, com o usuário, nunca do árbitro depois.
- **Intocáveis**: paths com mudança paralela na árvore, listados um a um.
- **Verificação por Task**: o comando exato e o que conta como passou.
- **Steps escritos como `- [ ] **Step N: …**`** — é o formato que o contador de progresso reconhece
  (`_STEP_RE` em `backend/app/planprog.py`; `### Task N:` para os cabeçalhos). Numerar de outro
  jeito (`Passo A`, `Etapa 1`) faz a Task inteira contar **zero** e a barra que o usuário acompanha
  no celular ficar parada com o trabalho andando. Receita partilhada por várias Tasks: escreva-a
  como texto explicativo e **repita os Steps dentro de cada Task** — o executor lê uma Task por vez
  e não pode depender de ter lido a anterior. Confira antes de aprovar:
  `uv run python -c "from app.planprog import parse_plan; p=parse_plan('<caminho>', require_started=False); print(p.total, [t.total for t in p.tasks])"`
- **Barra** das Tasks que mexem em pixel: contra o que o resultado vai ser comparado — ver abaixo.
- **O que a revisão precisa cobrir** — ver abaixo. Isso entra **antes da Task 1**.
- **Decisões em aberto**: o que ainda não foi decidido e quem decide. Lista vazia é a meta.
- **Teto**: quanto de custo/cota o usuário aceita gastar sozinho, e o que faz parar.
- **O time**, com motor e conta de cada papel.

### O rigor da revisão entra no contrato antes da Task 1

Escreva no plano o que a revisão tem que quebrar: fluxo completo na UI ou no comando real,
callers irmãos do símbolo alterado, concorrência (resposta atrasada, duplo clique, troca de
alvo, unmount), estado final em disco/storage/URL, e quais skills de revisão usar por tipo
de Task.

Task visual entra com **a lista dos estados** que precisam de screenshot (as duas larguras,
overlay, tela cheia, o que mais a Task afetar). É essa lista que o revisor cobra depois —
estado que ninguém listou é estado que ninguém olha.

### Task visual entra também com uma BARRA

Uma linha por Task que mexe em pixel: **contra o que o resultado vai ser comparado**. Não é
adjetivo ("bonito", "acabado", "com a cara do app") — é uma coisa que existe e que dá pra
abrir. Três testes, todos obrigatórios:

- **Nomeada**: uma tela específica, não uma categoria. `EnginesSheet` sim; "as outras folhas
  do app" não.
- **Buscável**: quem julga consegue **ver** — caminho absoluto de um print, ou uma tela do
  próprio app que dá pra abrir e capturar. Referência que só existe na cabeça de alguém não
  serve.
- **Comparável**: as duas imagens cabem lado a lado, no mesmo estado e na mesma largura.

Na prática a barra quase sempre é **uma tela irmã deste app** — é a comparação mais dura que
existe, porque é exatamente onde o desalinhamento aparece. Referência de fora (print de outro
produto que o usuário salvou) vale igual, desde que ele entregue o arquivo aqui na fase 1.

Sem barra escrita, o portão visual continua sendo o autor perguntando ao próprio pixel se
está bom — e "está bom?" devolve "está bom". Task que não desenha nada não precisa de barra.

#### Você PROPÕE a barra; ele escolhe

Não pergunte *"qual é a barra?"*. Quem usa o app sabe o que quer ver, não necessariamente qual
referência funciona como barra — e barra malformada (categoria em vez de tela, coisa que quem
julga não consegue abrir, estado que não casa) é pior que barra nenhuma, porque parece que o
portão existe.

Chegue com **duas ou três candidatas prontas**, cada uma já passada pelos três testes, e uma
frase dizendo por que aquela é dura:

```
Task 3 mexe na folha de Configurações. Barra — escolha uma:

a) `EnginesSheet.svelte` no desktop, modal centrado, 1440px — é a folha mais acabada
   do app e usa o mesmo par `wide`/`centered`; se a nova não empatar com ela, dá pra ver.
b) `Git.svelte`, mesma largura — mesmo material de vidro, mas com abas; boa se você quer
   cobrar a navegação por aba junto.
c) Um print que você salvar de outro produto — me manda o caminho e eu uso esse.
d) Sem barra nesta Task.
```

**"Sem barra" é opção legítima e fica na lista.** Escolhida, ela entra no plano como
`Barra: nenhuma — decisão do usuário, <data>`, e o portão visual daquela Task volta a ser o
protocolo normal do `executor.md` (abrir, clicar, capturar, olhar), sem a comparação cega.
Escolha registrada não é buraco; buraco é o campo em branco que ninguém decidiu.

Se as três candidatas te parecerem fracas, diga isso e proponha outras — barra que você mesmo
não defenderia não deve ir pra lista só pra ter três itens.

Sem isso as primeiras Tasks passam por um portão que ainda não existe, e o preço é uma
auditoria retroativa que reabre Task já aprovada — mais cara que ter escrito três linhas.

### O time é saída do planejamento — mas **você PROPÕE, ele escolhe**

Quem escreve e quem revisa se decide **aqui**, porque o research e o brainstorming acabaram
de mostrar de que este trabalho é feito. Decidir no lançamento é decidir sem esse dado.

**"Se decide aqui" quer dizer que a PERGUNTA se faz aqui — não que você responde.** Modelo, motor,
harness e conta são do usuário, sempre, e ler a política de contas da máquina **não** te autoriza a
preencher: aquele arquivo diz o que **pode** ser usado, nunca o que **vai** ser usado neste
trabalho. Preencher a tabela sozinho é indistinguível, no registro, de uma decisão que ele tomou.

Mesmo formato da barra (seção abaixo): chegue com o trabalho caracterizado e as combinações que a
máquina realmente consegue abrir, e faça **uma pergunta**. Não pergunte "qual modelo?" — ele pode
não saber o que a máquina oferece; e não liste "as contas permitidas" como se fossem opções.

Levante o inventário de verdade antes de perguntar, porque **`engines.json` não é o universo**:

```bash
claude-engine                     # motores p/ sessão Claude com --engine
pi --list-models | awk 'NR>1{print $1}' | sort -u   # providers do Pi (rode do SHELL do usuário)
ls -d ~/.claude ~/.claude-*       # contas Claude
```

Duas armadilhas medidas em 13/08/2026, as duas capazes de te fazer propor coisa que não existe:

- **Harness ≠ motor.** Uma conta pode ser inalcançável por `--engine` e perfeitamente alcançável por
  `--provider pi` ou pelo CLI próprio. Listar só os motores esconde metade das opções reais.
- **Rode a listagem do shell do usuário.** Provider cuja credencial é variável de ambiente (fish
  universal, `set -Ux`) **não aparece** num bash não-interativo, e você conclui que não existe. Use
  `fish -l -c '...'` (ou o shell dele) antes de afirmar que uma conta não está configurada.

E confira **colisão de id** antes de propor: duas contas diferentes podem oferecer os mesmos nomes de
modelo, e só o `provider/id` completo distingue — propor a errada é a fatura de outra pessoa.

Não existe elenco padrão. Modelo citado em qualquer exemplo é exemplo — nunca default.
Olhe as Tasks e responda:

| Pergunta sobre o trabalho | O que ela decide |
|---|---|
| Cada Task é volume mecânico, raciocínio sutil ou julgamento visual? | quem escreve — pode ser **um escritor por Task** |
| O erro típico dela aparece em quê: teste, tela, carga, estado em disco? | o que o revisor precisa **conseguir fazer** (ver print, rodar harness, ler concorrência) |
| Tem Task visual? O executor escolhido enxerga imagem? | se não enxerga, o protocolo de visão do `executor.md` (`see`) é obrigatório e entra no contrato — não é motivo pra descartar o motor |
| Quem revisa pensa diferente de quem escreveu? | família do revisor — **não negociável** |
| Quanto custa e em qual conta? | os motores, e o teto |

Regras fixas:

- **Quem planejou não executa.** Vira árbitro, read-only no código pro resto do trabalho.
- **Revisor de família diferente do executor.**
- **Revisão final** em sessão nova que não participou de nada.
- Um escritor por árvore vale mesmo com vários escritores no elenco: o portão serializa as
  Tasks, então eles nunca escrevem ao mesmo tempo.

O plano registra a tabela com **cinco** colunas — papel, sessão, agente/motor, **qual conta
gasta**, e **como a sessão é aberta** (o comando literal). Motor de provedor consome a conta
dele: o usuário aprova isso aqui.

A coluna "como abrir" existe porque *"a revisão final é numa sessão do <agente> X"* é uma
frase que envelhece mal: meses depois, na hora de abrir, vira decisão improvisada entre
conta padrão, motor, gateway e subagente — e as quatro dão resultados diferentes. Escreva o
comando no dia em que o usuário definir o papel:

```markdown
| Papel | Sessão | Agente/motor | Conta | Como abrir |
|---|---|---|---|---|
| revisão final | <trab>-final | <agente>, conta padrão | assinatura | `cp-send --new <trab>-final <cwd>` (SEM --engine) |
```

**A revisão final entra na tabela como item próprio, com o gatilho junto:** *"dispara quando
todas as Tasks de código estiverem aprovadas"*. Nunca "depois da Task N" — Task manual
(subir asset, registrar domínio, mexer em conta) não é Task de código, e amarrar o portão
final a ela faz o gatilho não disparar nunca. A receita de abertura está em `arbitro.md`.

### Antes de aprovar

Passe o plano por um olhar adversarial (subagente de arquitetura + explorador): cada
arquivo/símbolo citado existe? A ordem se sustenta? O que quebra? Plano que cita símbolo
inexistente vira round perdido na execução.

**Este subagente roda com o modelo da definição dele — não force nada.** A política de contas da
máquina governa as **sessões do time**, que ainda nem foram decididas nesta altura; ela não governa
um subagente que você despacha durante o planejamento. Passar `model:` aqui "pra respeitar a tabela"
é aplicar uma regra fora do escopo dela, e ainda troca o modelo que o autor do agente escolheu.
Erro medido em 13/08/2026: o planejador tentou fixar `model: opus` num pass adversarial citando a
tabela de contas, numa fase em que o usuário não tinha definido time nenhum.

Ofereça o pass — não o rode sozinho, e não o pule por achar que já conferiu tudo. Ele pega o que
você não tem como ver: na mesma data, achou 20 problemas num plano revisado, sendo 5 que faziam
Task fechar em vermelho — inclusive três arquivos citados que **não existiam** e a regex do próprio
guard do plano, que passaria a casar o mapa errado depois da Task que a consertava.

## Fase 2 — Lançamento (o único "pode ir" do usuário)

### Pré-voo, antes de criar sessão

```bash
git status --short          # árvore suja → os paths viram intocáveis, listados um a um
git branch --show-current   # branch certa
cp-send --list              # QUEM mais está vivo neste cwd
tmux display -p '#{session_name}'   # ... e QUAL DESSES É VOCÊ
```

Outra sessão escrevendo neste checkout trava a largada — resolva com ela, não com o usuário.
Não resolveu → aí sim é decisão dele.

**A quarta linha não é enfeite: você aparece na própria lista.** Sem ela, a sessão `working` no seu
cwd parece um segundo escritor, e você gasta uma rodada mandando recado — para você mesmo, que
volta como `[de: <você>]`. Medido em 13/08/2026.

**Branch: `main`/`master` não é decisão sua.** Se a árvore está na branch principal e o plano tem
mais de um commit, **pergunte antes de criar sessão** — criar ou trocar branch por iniciativa
própria é proibido, e largar um time de doze commits na `main` é pior. Chegue com a proposta pronta
(branch nova a partir do HEAD atual, com nome) e a alternativa (commitar direto), e deixe ele
escolher. Se ele pedir `pull` antes, **reconfira os números do plano depois**: um pull que traz
centenas de linhas move as linhas que o plano cita e pode mudar as contagens que a fase 1 mediu.

**Baseline verde antes da Task 1.** Rode cada comando de verificação que o plano define, **uma
vez, na base** — ainda antes de criar sessão. Só isso pega dois modos de falha que custam uma
round inteira cada:

- suíte já vermelha no HEAD de partida → a primeira REPROVA culpa o executor por quebra
  herdada, e ninguém mais separa o que é dele do que já estava lá;
- comando que não roda nesta máquina → DEVOLVIDO na primeira revisão ("as verificações não
  rodam"), descoberto por quem não tem como consertar.

Registre o resultado no contrato: `baseline: <comando> → <verde, N testes>, <data>`. Vermelho →
decisão do usuário **antes** de largar: consertar antes, ou registrar como falha conhecida que
a revisão ignora. Nunca largar calado com a base quebrada.

### Criar, na ordem

```bash
cp-send --new <trab>-writer /caminho/do/repo --engine <motor do plano>
cp-send --new <trab>-review /caminho/do/repo --engine <outro motor>
cp-send --pair <sessao> "<trab>: <onde está o contrato>"   # uma chamada por sessão
```

**NUNCA ponha o papel na string do `--pair`.** Ela é um campo do **GRUPO**, não da sessão: o
sidecar de cada membro guarda a MESMA `task`, e cada `--pair` novo **sobrescreve a de todos** e
dispara um aviso pro grupo inteiro com aquele texto. Pareando o executor com `"papel: executor"` e
depois o revisor com `"papel: revisor independente"`, o executor recebe um aviso dizendo que ele é o
revisor — e assume, porque a mensagem chegou pela infraestrutura, com cara de autoridade.

Aconteceu de verdade em 13/08/2026: a sessão do executor anunciou *"a segunda mensagem corrigiu meu
papel: agora sou i18n-review"* e foi ler o contrato como revisora. Custou uma correção de papel nas
duas sessões e uma reescrita dos três sidecars.

A string do `--pair` é **neutra e aponta pro contrato**, nunca afirma papel:

```bash
cp-send --pair <sessao> "<trab> — o papel de cada sessão está no contrato grupo-<gid>.md"
```

Papel se declara **no kick-off e na tabela do contrato**, e o contrato diz explicitamente que, se um
aviso de grupo contradisser a tabela, vale a tabela. Se você já errou isto, conserte o estado, não
só o texto: os sidecars ficam em `<config>/.claude-pocket-pair/<sessao>.json`, campo `task`, e dá
pra reescrever direto (tmp+rename) sem disparar broadcast novo.

Ordem obrigatória: `--new` → `--pair` → ler o `gid` no próprio sidecar → **escrever o
contrato** → só então os kick-offs. Endereço apontando pra arquivo que ainda não existe é
uma sessão parada perguntando.

Motor inexistente devolve `400` e a sessão não nasce. Ver os motores: `claude-engine`.

### O contrato nasce de esqueleto, não de memória

O conteúdo do contrato está descrito em prosa em três arquivos; reconstruir de cabeça é como
campo esquecido aparece — no meio da execução, como lacuna que ninguém decidiu (já custou um
revisor revisando sem nenhum dos subagentes instalados, porque a seção de ferramental ficou em
branco). Copie e preencha; campo que não se aplica leva `n/a`, **nunca some** — campo apagado é
invisível pra quem lê depois:

````markdown
> Sessões deste grupo: invoquem a skill `orchestrating-idea-to-push`.
> Plano: <caminho>. Branch: <branch>. HEAD de partida: <hash>.

## Quem é quem
| Papel | Sessão | Agente/motor | Conta | Como abrir |
|---|---|---|---|---|
| árbitro | <esta sessão> | ... | ... | (já aberta) |
| executor | ... | ... | ... | `<comando literal>` |
| revisor | ... | ... | ... | `<comando literal>` |
| revisão final | ... | ... | ... | `<comando literal>` — dispara quando TODAS as Tasks de código estiverem aprovadas |

Aviso de grupo contradizendo esta tabela: vale a tabela.

## O que o plano possui (aponte, não copie)
Ordem das Tasks, Steps, verificação por Task, intocáveis, barras da fase 1: <plano, seção>.
Baseline: <comando> → <resultado>, <data>.

## Ferramental de revisão (por tipo de Task)
| Tipo de Task | Subagentes/skills a despachar | Não usar (motivo em uma linha) |
|---|---|---|

## O que a revisão precisa cobrir
<do plano da fase 1: fluxo completo, callers irmãos, concorrência, estado final, visual>

## Teto
<custo/cota que o usuário aceita, e o que faz parar>

## Barras decididas DEPOIS da aprovação do plano
Task N — Barra: <tela, estado, largura> | nenhuma — decisão do usuário, <data>

## Progresso
| Task | Hash | Veredito | Quem corrigiu (se round de correção) |
|---|---|---|---|

## Decisões supervenientes
<data> — <decisão, de quem, motivo em uma linha>
````

### A sessão nova prova modelo e effort ao vivo

`cp-send --new --engine` **não** configura effort, e pedir "max" no primeiro prompt não
funciona. Antes de liberar a primeira Task, exija da sessão nova a prova ao vivo (o que a
statusline dela mostra, ou o retorno do comando de troca) — repetir o que o kick-off pediu
não é prova. Sem isso ela trabalha horas no effort errado afirmando que está no certo.

### Recado: nativo ou cp-send

Sessão no `ListAgents` e você tem `SendMessage` → `SendMessage`. Senão `cp-send <sessao>`.
`--new`, `--pair` e `--group` são sempre `cp-send`. Mensagem longa vai por heredoc de aspas
simples.
