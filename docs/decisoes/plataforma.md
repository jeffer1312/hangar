# Plataforma — nomes, pareamento, planos, ditado

Decisões medidas, com data e número. O `CLAUDE.md` carrega a regra;
a medição que a sustenta mora aqui. Conteúdo movido sem alteração.

## Revisão de código

neste repositório GitHub, usar revisão local e as verificações
  do projeto. A instalação local do CodeRabbit pertence a outros repositórios.

## Comentário explica o PORQUÊ, e é curto. A história medida mora AQUI, não no código.

Este
  arquivo é longo de propósito: é o lugar onde decisão medida, com data e número, sobrevive e é
  relida. O código não é. Lá vale a regra, não a arqueologia dela: se o comentário repete o que o
  nome da função já diz, sai; se a explicação ficou maior que o trecho que ela explica, o excedente
  vira linha de commit ou entrada em `docs/decisoes/`. **Medição, versão de CLI e data envelhecem** — num
  comentário elas viram afirmação falsa que ninguém revisa, enquanto aqui e na mensagem de commit
  estão datadas por construção. Isso não é licença pra código mudo: o porquê não-óbvio continua
  obrigatório, em uma ou duas linhas.

## O nome antigo (`claude-pocket`) só existe em ponte de compatibilidade

(rename de 25/08/2026).
  O código conhece **um** nome: as pastas de dados são `<config>/.hangar-*`, o cofre do sync é
  `~/.hangar/`, os comandos são `hangar-send`/`hangar-engine`/`hangar-codex`/`hangar-conta` e
  `hangar-panel-*`, a instância do quickshell é `hangar`, e o logger é `hangar.*`. Escreva com o
  nome novo; nunca leia o antigo em código novo. O que resta do velho é, todo ele, migração:
  - `backend/app/migracao_sidecars.py`, chamado na **subida** do backend (`main.py`), renomeia
    `.claude-pocket-*` → `.hangar-*` em todo perfil `~/.claude*` e deixa **link** no caminho antigo.
    O link é o que impede a máquina de se partir no meio da atualização: hook, extensão do Pi e o
    publicador de statusline do Kimi (`~/.kimi-code/statusline.js`, que nem mora neste repo) podem
    estar vivos e desatualizados, escrevendo no nome velho — e caem na pasta nova. Startup, e não
    installer, porque atualizar é `git pull` + reiniciar o serviço; rodar `install-*.sh` não é
    garantido. Ele **nunca funde** duas pastas: destino já existente para naquele item, com aviso.
  - `.json` SOLTOS (`apelidos`, `conn`, `models`, `runner`, `opencode`) leem os dois caminhos, novo
    primeiro (`migracao_sidecars.caminho_de_leitura`), porque no Windows link de ARQUIVO exige
    privilégio — para pasta há junção (`mklink /J`), para arquivo não há equivalente. **Escrita
    sempre no nome novo.**
  - Anexo e diário de orquestração **saíram do projeto e do config dir da conta** (31/08/2026) e
    moram no cofre: `~/.hangar/uploads/<projeto>/<sessão>/` (`uploads._base()`) e `~/.hangar/orq/`
    (`orq.raiz_padrao()`). Os dois estavam no lugar errado pelo mesmo motivo — um lugar que
    pertence a *outra coisa*. O anexo nascia dentro do repositório trabalhado, e o `.gitignore` que
    o esconde é o DESTE projeto: em qualquer outro repositório ele aparecia como
    untracked (32 pastas dessas na máquina, uma no próprio `~`). O diário morava em
    `~/.claude/orq-retros`, que é o config dir de UMA conta, enquanto o contrato de um trabalho põe
    papéis em contas diferentes de propósito — e um executor Pi/Kimi/Codex não tem `~/.claude`.
    **Nada foi migrado**, por decisão do usuário: o que ficou no lugar antigo continua no disco,
    vira 404 no histórico do celular e some do painel de orquestração. A trava do endpoint `/file`
    não muda nada disso — ela exige que o caminho apareça no transcript, e aceita absoluto.
  - **Marcador de bloco gerenciado** (rc do shell, `~/.tmux.conf`, `keybinds.lua`, o bloco
    "Sessões-irmãs" do `~/.claude/CLAUDE.md`) virou `hangar`, e cada installer **arranca o bloco do
    marcador antigo antes** de escrever o novo — sem isso o arquivo do usuário fica com os dois, um
    deles ensinando o comando velho e que nenhum installer atualiza mais.
  - `cp-send` e companhia continuam existindo como **symlink permanente** pro mesmo script (rc
    antigo e sessão Claude já aberta chamam o nome velho); as variáveis `CP_*` e o cookie
    `cp_token` **não mudam** — quebrariam o `.env` de instalação alheia. A documentação ensina só o
    nome novo.

## Ditado: a transcrição não é o problema, o que vem depois é

(`app/transcribe.py` +
  `app/narrar.py:limpar_ditado`). Duas etapas, dois modelos: a Whisper (`whisper-large-v3-turbo`)
  ouve, e um LLM limpa. Tudo aqui foi **medido em 14/08/2026** — 5 ditados reais × 3 execuções ×
  4 modelos —, e as três coisas que mudaram valem como regra, não como preferência:
  - **O modelo da limpeza importa mais do que parece, e o critério não é tamanho — é obediência.**
    O `llama-3.3-70b-versatile`, o padrão até aqui, inventava pasta em caminho ditado
    ("backend barra app barra narrar ponto py" → `backend/barra/app/barra/narrar.py`, 3/3) e mantinha
    **as duas versões** quando a pessoa se corrigia falando. Padrão agora é `openai/gpt-oss-120b`
    (Groq, ~1,2s). O melhor dos quatro foi o `deepseek-v4-flash`, mas ele **raciocina**: 6,4s de
    mediana e **3 de 15 chamadas estourando o timeout de 8s** da limpeza — o ditado voltava cru. Com
    `reasoning_effort: "none"` ele cai pra 1,8s e acerta tudo. Daí o campo `llm_reasoning_effort`, que
    é **opcional de propósito**: vazio = a chave some do payload, porque mandá-la a um provedor que
    não a conhece é um 400 que derruba a limpeza inteira.
  - **Regra de prompt só funciona com exemplo de entrada e saída.** A regra "aplique as correções que
    a pessoa falou" era a razão de ser da limpeza e falhava 0/3 em dois modelos: eles *pontuavam* a
    correção ("A primeira é o custo do carretel. Não, desculpa. A primeira vai ser…") em vez de apagar
    a versão errada. Trocar o verbo por **APAGUE** e colar um par entrada/saída levou a 3/3. Mesmo
    padrão na regra 4 (`barra` → `/`, `traço traço` → `--`): sem o par, o `gpt-oss-120b` deixava a
    frase literal 3/3. Toda regra nova aqui **nasce com exemplo**, e com um contra-exemplo quando ela
    pode generalizar demais ("o ponto principal", "a barra de rolagem" não podem virar pontuação).
  - **Vocabulário vai pra Whisper, não pro LLM.** O `prompt` da API é enviesamento de decodificação,
    e é onde `hangar-send` para de sair "CP send". Consertar depois é impossível por construção: a
    limpeza tem ordem explícita de **preservar** nome próprio como veio, então o que a Whisper errou
    chega errado no fim. `VOCAB_BASE` (termos do app, valem pra todo mundo) + `ditado_vocabulario`
    (o que é de uma pessoa só), truncados em `_VOCAB_MAX` porque a API corta em ~224 tokens **calada**.
    `language=pt` fixo pelo mesmo motivo: sem ele, frase curta cheia de jargão inglês voltava em inglês.
  - Cuidado de cota: o prompt novo tem ~940 tokens por chamada (era ~400). No plano gratuito da Groq
    (8000 tokens/minuto) isso não incomoda um ditado por vez, mas **estoura em teste automatizado** —
    um 429 lá é cota, não qualidade; separe os dois antes de culpar o modelo.
  - **Três estilos, escolhidos na pill ao lado do microfone** (`ESTILOS_DITADO`, `ditado_estilo`,
    `components/DitadoEstiloPopover.svelte`): `limpar` (só tira hesitação e pontua), `prosa`
    (reorganiza e corta repetição — o padrão) e `briefing` (vira documento com seções). A pill fica na
    barra do composer, ao lado do microfone, e abre o MESMO popover do esforço — não um modal: é
    decisão do tamanho de escolher o esforço, e cobrir a tela pra isso é desproporcional. Existem
    porque a mesma limpeza não serve pros dois usos: ditar "abre o narrar.py" e ditar um pedido de
    dois minutos. Quem lê o estilo é o backend, então o atalho Ctrl+Espaço já grava no estilo
    escolhido sem saber que ele existe. **`briefing` é rebaixado pra `prosa` abaixo de
    `_MIN_PALAVRAS_BRIEFING`** — sem isso ele punha um `**Objetivo**` em cima de uma linha só.
  - **A trava de honestidade mudou de forma porque a antiga proibia o que o usuário pediu.** Contar
    palavra nova crua (o guarda antigo) rejeitava qualquer estruturação: `Objetivo:`, `-`, e até
    escrever "tô" como "estou" contavam como conteúdo inventado — 8 "palavras novas" num ditado
    real, com 100% do conteúdo preservado. Agora são duas medidas, e as duas foram calibradas
    contra os mesmos casos: **cobertura** (quanto do conteúdo da pessoa sobreviveu; pega o modelo
    que resumiu ou respondeu) e **`_conteudo_novo`** (palavra de conteúdo que ela não falou). Medido: defeito 4 palavras novas, limpeza honesta 0, prosa real 1 → teto 2. **Cobertura
    sozinha não separa** (defeito 79%, prosa legítima 75%), e é por isso que as duas coexistem.
  - **O `briefing` NÃO paga a trava de invenção** (`_Travas.cobra_invencao`), e isso é decisão do
    usuário, não descuido: "no briefing minhas palavras vão mudar; se eu estiver em prosa, aí
    beleza, não mudar minhas palavras". `limpar` e `prosa` não reescrevem — um pontua, o outro
    reordena —, então ali palavra nova é palavra que a pessoa não disse. O briefing reescreve por
    definição, e cobrar dele é recusar o serviço pedido: medido, um briefing bom com 98% de
    cobertura foi rejeitado por 4 "invenções" que eram conjugação. Ele segue protegido pelo teto de
    tamanho, pelo piso de cobertura e pela recusa de saída vazia.
  - **Comparação é por RADICAL** (`_radical`), não pela palavra inteira. `clicava`/`clico`/`clicar`
    caem no mesmo balde. Sem isso a trava punia conjugação — a mesma classe de erro que
    `_CONTRACOES` resolveu pra `tô`/`estou` e que voltou por outra porta. **Mas a vogal final só cai
    com prova de verbo no próprio texto** (`_raizes_de_verbo`, alimentado pelos DOIS textos): cortá-la
    sempre juntava `posto`/`posta` e `conta`/`conto` no mesmo radical — o par que o comentário do piso
    usava como exemplo do que não podia acontecer —, e aí trocar "a conta do cliente" por "o conto do
    cliente" passava com 0 palavra nova e 100% de cobertura, calado, justo em `limpar` e `prosa`.
    Sufixo de verbo (`ava`, `ando`, `ar`, …) e derivação (`mente`, `dade`, …) cortam sempre; plural
    (`s`) também. Contra-exemplo travado em `test_troca_de_genero_ainda_e_palavra_nova`.
  - `_CONTRACOES` iguala fala reduzida à forma escrita (`tô`→`estou`, `pra`→`para`) **antes** de
    qualquer comparação. Sem isso a limpeza melhora o texto e é punida por isso.
  - **Raciocínio piora e não é questão de calibragem.** Testado com os dois ditados reais: com
    `reasoning_effort` ligado, 4 de 9 execuções estouraram 25s e a única prosa que voltou levou
    14,9s, contra 2,3–3,2s desligado. Num teste anterior o modelo pensando ainda comeu o "não o
    redis" de "usa o postgres não o redis", lendo negação como autocorreção. Pensar sobre um texto
    vira interpretar o texto, e aqui interpretar é o defeito.
  - **Quem manda no estilo é a PILL, não a config** (`?estilo=` no `/transcribe` →
    `narrar._estilo_efetivo(cru, pedido)`). O estilo mora no servidor, mas o app o lê **uma vez por
    carga de página** (`lib/ditadoEstilo.svelte.ts`): uma troca feita noutra aba ou noutro aparelho
    nunca chegava na tela aberta, e em 21/08/2026 a pill dizia "Só limpar" enquanto o servidor
    guardava `briefing` — o ditado voltou estruturado sem ninguém ter pedido. O front manda junto o
    rótulo que a pessoa **leu antes de falar**, e ele vence; ausente ou desconhecido, a config
    decide como sempre. Duas amarras: o front só manda quando o store já leu o servidor
    (`ditadoEstilo.pronto` — mandar o padrão chutado seria o app sobrescrevendo a escolha dela com
    palpite), e o popover **revalida** ao abrir, pra a lista parar de exibir valor de horas atrás.
  - **O teto de tempo é rede contra pendurar, e ele mora nas DUAS pontas.** O do navegador era
    120s (`lib/api.ts`) enquanto o backend podia gastar 120s de Whisper **mais** a limpeza: a
    requisição era abortada com o trabalho em curso e a pessoa perdia o ditado inteiro por causa do
    relógio do cliente. Hoje: 300s no cliente, e 60/90/120s por estilo no servidor (subidos em
    21/08/2026 — o `muse-spark-1.2-contributor-free` do OpenCode Zen levou 16,4s pra limpar UMA
    frase, contra ~1,2s do `gpt-oss-120b` na Groq). Quem estoura ainda **não perde o áudio**: o
    `Composer` guarda o `File` da tentativa que falhou e oferece "Transcrever de novo" ao lado do
    erro — o áudio já existe, mandar a pessoa repetir dois minutos de fala é que era o defeito.
  - **O provedor da limpeza é trocável pela tela, e não só a Groq** (Configurações → Servidor →
    Avançado: Endpoint / Chave / Modelo / Raciocínio do LLM → `llm_*` em
    `~/.claude/runtime-config.json`). `_provedor()` só lê `llm_api_key` quando há `llm_base_url`
    próprio; endpoint vazio = Groq com a `groq_api_key`. E o **briefing tem provedor próprio**
    (`llm_briefing_*`, `_provedor("briefing")`), porque os dois usos não pedem o mesmo modelo:
    limpar e prosa querem rapidez — a pessoa está olhando o campo esperando o texto —, o briefing
    quer quem estrutura melhor e pode demorar. Medido aqui: Groq/`gpt-oss-120b` 2,1s no limpar,
    OpenCode Zen/`muse-spark-1.2-contributor-free` 9,0s no briefing. O perfil sai do **estilo**,
    nunca de um flag à parte, e endpoint de briefing vazio cai no provedor de sempre. Provedor
    lento muda o que as travas veem:
    medido no muse-spark, um ditado com autocorreção longa cai pra 62% de cobertura e o estilo
    `limpar` (piso 0,80) devolve o cru com aviso — o modelo apagou a versão corrigida, que é a
    regra funcionando, mas o piso de `limpar` não foi calibrado nele.
  - **Provedor que falha cai na assinatura Claude da própria máquina** (`narrar._via_claude`), e o
    caminho é o `claude -p`, não o SDK — o `claude-agent-sdk` roda esse mesmo binário por baixo,
    seria dependência nova pra chegar no mesmo `subprocess`. O gatilho é falha do PROVEDOR (HTTP,
    rede, JSON inválido, payload sem texto); **sem chave configurada continua 503**, porque config
    ausente é pra corrigir na tela, não pra mascarar gastando cota. Falhando os dois, quem sobe é o
    erro ORIGINAL do provedor — "o plano B também falhou" é ruído em cima do que a pessoa conserta.
    O que motivou: em 08/09/2026 o OpenCode Zen respondeu **500 nos dois** modelos que uma chave
    `contributor` alcança (`muse-spark-1.3-contributor` e `1.2-contributor`; o `1.2-…-free` virou
    401 "not supported", e glm/deepseek dão 400 nessa chave), e cada ditado voltava cru.
    Três números medidos no mesmo dia, com o system prompt real do estilo `limpar`:
    **sonnet 3,6–4,5s contra haiku 27,8–39,2s** (consistente em 3 rodadas, não é primeira chamada —
    daí o sonnet fixo); **`--tools ""` leva a entrada de ~18.900 tokens pra 466**, porque o caro são
    as definições das ferramentas, não o prompt; e o par `--setting-sources ""` + `--strict-mcp-config`
    evita carregar settings/hooks/skills e subir servidor MCP pra limpar uma frase. O prompt vai por
    **stdin**: ditado de dois minutos passa do limite de argv e apareceria inteiro no `ps`.
    Duas coisas que o fallback NÃO muda, medidas: o texto dele passa pelas mesmas travas (um resumo
    volta como cru, igual ao do provedor), e a `_cobertura` continua rejeitando o ditado curto cheio
    de `barra`/`traço traço` — 0,727 contra o piso 0,80 de `limpar`, idêntico pela Groq.

## Grupo: o protocolo é do HOOK, a saída é de UMA esteira, e o anti-loop é do backend


  (`app/pair_texto.py` + `hooks/pair_hook.py` + `api._avisar_saida` + `registry._varrer_pares_mortos`,
  02/09/2026). Decisões que fecham furos medidos na análise daquele dia:
  - **Protocolo reinjetado no `SessionStart`** (`startup|resume|clear|compact`) a partir do sidecar
    `.hangar-pair/<nome>.json`. O prompt do `--pair` sumia no `/clear` e na compactação; o badge
    ficava e o modelo esquecia os pares. O `pair_dir` vai por argv porque é o do BACKEND — sessão em
    `--conta` tem `CLAUDE_CONFIG_DIR` próprio, e o sidecar não mora lá. Por isso os textos moram em
    `pair_texto.py`, stdlib-only (mesma regra do `engines.py`).
  - **Protocolo completo só pro recém-chegado** (`snap[m] is None`); veterano recebe "fulano entrou";
    peers e tarefa iguais = nada. Adicionar o 5º membro disparava 5 prompts de 1,5KB, 4 redundantes.
  - **Tarefa diferente da existente é 409** sem `--substituir-tarefa` — cada `--pair` de um árbitro
    sobrescrevia a de todos, calado.
  - **Toda saída avisa quem ficou pela mesma esteira**: unpair, kill (não avisava ninguém — os pares
    mandavam recado pra nome morto ou pra sessão nova que o reusasse) e morte fora do app. Remoto vai
    por `/unpair-remote` no unpair e no kill; na varredura só loga (rede dentro do `list()` não).
  - **Varredura de morto fora do app roda no fim de `list()`, e três coisas a seguram:** contador
    DE CLASSE (há 4 instâncias de `SessionRegistry` — api, sse×2, prune — e todas chamam `list()`);
    ausência confirmada por **tempo** (`_PAIR_AUSENCIA_MIN_S`), não por número de polls, porque
    `kill()` e `rename()` chamam `list()` numa janela em que o nome está ausente de propósito; e lista
    vazia = tmux fora = não varre, senão dissolvia todo grupo da máquina. Aviso pela fila durável
    (nunca send-keys ali) + drain por callback (`apos_saida_por_morte`), porque a fila só drena em
    transição de hook e o peer já ocioso nunca receberia. O dict de classe (`_pair_ausencias`) é
    limpo com `pop(n, None)`, nunca `del` — as 4 instâncias varrem concorrentemente e outra thread
    pode já ter tirado a mesma chave.
  - **`--group` recusa `[grupo:`/`[de:` reencaminhado e limita 5/min por gid** (429) — só no que
    passa pelo backend: peer com `inbox_socket_of` é devolvido em `pulados` e vai por `SendMessage`
    (o script sai 3 listando quem falta); o socket do Claude Code está fora do alcance do backend.
    `--group --tmux` força o antigo.
  - Contrato do grupo dissolvido vai pra `~/.hangar/pair-arquivo/`, não pro `unlink`.
  - **Teste que chega em `SessionRegistry.list()` ou num `pair.leave()` de último membro isola
    `pair.settings.projects_dir`, zera `SessionRegistry._pair_ausencias`, anula
    `registry.apos_saida_por_morte` e faz patch de `pair._arquivo_dir`** — sem as quatro, 2 vezes
    neste plano uma suíte verde mexeu de verdade no `~/.claude/.hangar-pair`/`~/.hangar/pair-arquivo`
    do desenvolvedor.

## Plan progress

(`app/planprog.py` + `registry._decorate_plan` + `PlanBar`/`PlanPanel.svelte`):
  the source of truth is the plan's own `.md` under `docs/superpowers/plans/` — no separate state
  file, `parse_plan` re-reads it and re-counts `- [x] **Step …**` on every discovery. Fenced blocks
  (` ``` `/`~~~`) are stripped **before** the regex runs, **preserving byte offsets** (chars → spaces,
  `\n` kept) — plans show example steps inside code fences, and without the strip a freshly-written
  plan is born "3/47 done" (measured on this very plan: 53 matched vs 48 real). The decoration runs
  **inside** the git `to_thread` (`registry.py:851`), never in the coroutine — same precedent as the
  2026-07-23 incident. `_list_sig` (`sse.py`) carries `plan_name` alongside `plan_done`/`plan_total`:
  switching from plan A to plan B that happens to also read `9/17` wouldn't re-emit the list and the
  chip would stick on the wrong plan — the same bug class as `engine`. `plan_tasks` (one `(done,total)`
  per Task) rides the payload too, because the segmented bar can't be derived from
  `task_idx`/`task_total` alone — it would lie whenever an earlier Task still had a pending step.
  `_plans_dir` climbs up to 6 levels looking for `docs/superpowers/plans/` but **stops at the first
  `.git`** — without that, a worktree with no plans of its own would climb into the main checkout and
  show someone else's plan ("no bar" is a limitation, "wrong bar" is a bug). Executing a superpowers
  plan: mark `- [ ]` → `- [x]` at the end of each Step — that's what feeds this feature.

## Compartilhado por sessão, não por conexão

(`app/difusor.py`, `stats.Accumulator.
  compartilhado`, `registry._atualizar_git`, 04/09/2026). Três coisas que cada SSE refazia
  sozinho: o monitor de estado (desktop + celular = 2× `has-session` + `capture-pane` a cada
  0,75s), o acumulador de estatísticas (relia o transcript inteiro por conexão — 109–204ms num
  de 21 MiB, contra 0,08ms no acumulador já quente) e o git da listagem (`git status` + `diff`
  em série por sessão ANTES de publicar o estado; um repositório lento segurava o card de todas).
  O `Difusor` é genérico: uma fonte por chave, cada ouvinte recebe o último evento ao entrar
  (o monitor só emite em mudança) e cópia dos seguintes; a fonte morre com o último ouvinte. A
  chave do monitor leva o transcript, e o `__reset__` do `/clear` recria o `state_task`: o
  monitor fecha sobre o sid da conexão que o criou, e sem isso quem ficasse herdaria a closure
  de uma conexão já morta. O git agora sai da lista com o último número bom por cwd e atualiza
  em segundo plano (single-flight por cwd; resultado `None` não apaga o anterior — erro nunca
  vira "repositório limpo"). Custo: o primeiro poll depois de subir o backend sai sem badge de git.
  **Medido e NÃO mexido: o re-render da prévia a cada 33ms.** No app desktop (Electron, esta
  máquina), uma resposta de 9,8k chars em streaming = 797 atualizações da prévia, 2 long tasks
  (51 e 61ms), quadro p95 de 33ms, 5 quadros acima de 50ms em 38s. Não justifica trocar o
  parse inteiro por parse do último bloco (Markdown novo muda a leitura do anterior). No celular
  não foi medido — a sonda é `PerformanceObserver('longtask')` + `requestAnimationFrame` na
  página do chat, e teria que rodar no iPhone.
