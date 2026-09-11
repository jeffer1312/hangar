# Harnesses — Claude, Codex, Pi, omp, Kimi

Decisões medidas, com data e número. O `CLAUDE.md` carrega a regra;
a medição que a sustenta mora aqui. Conteúdo movido sem alteração.

## Ponte de skills (`app/skill_bridge.py`): o omp descobre sozinho as skills dos outros CLIs (providers `claude`/`claude-plugins`/`agents`); Pi e Kimi leem as pastas da própria config.

Sem a ponte, cada um mantinha uma fazenda de symlinks à mão apontando pro
  cache VERSIONADO dos plugins (`plugins/cache/ecc/ecc/2.2.0/skills/...`): bump de versão =
  dezenas de links pendurados, calados (03/09/2026: 3 fazendas manuais, 99/119/157 links, todas
  com podres). A ponte varre as fontes (`~/.claude/skills`, `skills/` do repo, cache — só a
  versão MAIS NOVA de cada plugin —, marketplaces, `~/.agents/skills`), dedup por nome na ordem
  de precedência, e materializa symlinks nas pontes: pi → `~/.pi/agent/skills-bridge`, kimi →
  `~/.kimi-code/skills-bridge`. Harness novo = uma linha em `TARGETS`;
  o omp fica fora de propósito (descobre nativo), e o Codex tem reconciliador próprio desde
  06/09/2026 (abaixo). Regras duras: stdlib-only (o installer chama
  com o python3 do sistema, regra do `engines.py`); **só mexe em symlink cujo alvo está numa
  fonte conhecida** — arquivo real do usuário ou link à mão pra fora das fontes
  nunca é tocado; config alheia (settings.json do pi, config.toml do kimi) é só CONFERIDA, com
  aviso quando a ponte não está na lista — nunca editada. Roda na subida do backend e no
  `install-claude-wrapper.sh` (precedente `migracao_sidecars`: atualizar é `git pull` + restart,
  installer não é garantido). Standalone: `python3 backend/app/skill_bridge.py [--dry-run]`.
  **Ela é a ÚNICA dona das pastas de ponte** (04/09/2026): o `scripts/install-skills-bridge.sh`
  — que o hook `SessionStart` do Claude chama, e que o `claude-hooks-adapter` roda também dentro
  do Pi — tinha uma poda própria, só de plugins, e apagava a cada largada do Pi os 67 links de
  skills pessoais/marketplace que a ponte criava (o Pi abria listando cada uma como "skill path
  does not exist", e o backend as recriava no restart seguinte: 67 criados, todo dia). Hoje esse
  script cuida da persona do Pi/Kimi e dos pacotes do Pi, e chama a ponte no fim. Ele não escreve
  mais no Codex: nem hooks, nem persona, nem symlinks de skills.

## Integração nativa do Codex

(`app/codex_integracao.py`, `codex_importador.py`,
  `codex_compat.py`, `codex_arquivos.py`, 06/09/2026): o Hangar usa o importador oficial
  `externalAgentConfig/detect` + `import` e espera a notificação `import/completed` com o mesmo
  `importId`. Plugins e marketplaces usam os comandos nativos do CLI; nenhum turno de agente é
  aberto para sincronizar. Dois gatilhos, e só: a abertura de uma sessão Codex (o lançador chama
  `POST /api/harness/codex/integracao/sessao` e espera até 20s) e o botão **Reconciliar agora**.
  Sem laço e sem rodada na subida — decisão do usuário em 06/09/2026, no lugar da varredura das
  pastas do Claude a cada 30s que veio no PR: **Codex converte, backend decide quando, lançador só
  avisa.** A abertura é um cache por conteúdo (`precisa_reconciliar`): a assinatura das pastas do
  Claude fica no `estado.json`; igual à última, marketplace dentro das 6h e última rodada sem
  falha = o Codex nem é chamado (medido: 0,08s contra 1,0–1,3s da rodada vazia do PR). Falha só é
  refeita 5 min depois, na abertura seguinte. A sincronização opcional do Codex Desktop é
  independente e não é necessária. A documentação de arquitetura, migração e limites está em
  [`docs/codex-integration.md`](docs/codex-integration.md).
  O registro e os backups ficam em `~/.hangar/codex-integracao/<identidade>/`, separados por
  `CODEX_HOME`; o lock em `CODEX_HOME/.hangar-integracao.lock` serializa os escritores do Hangar
  mesmo quando seus valores de `HOME` diferem. `GET` do painel é só
  leitura, `POST` inicia ou acompanha a operação existente (202). O painel consulta enquanto a
  operação executa e descarta respostas ao trocar servidor/desmontar. **Nunca gravar confiança
  para autoaprovar hooks**: normalizar RTK/`SessionEnd` pode invalidar aprovação, então o painel
  e a TUI avisam. Instruções globais usam bloco gerenciado no `AGENTS.md`; fallbacks `CLAUDE.md`
  e `CLAUDE.MD` são acrescentados à config sem substituir os já existentes.
  `settings.env` entra pelo item nativo `CONFIG` em HOME temporário; somente
  `shell_environment_policy.set` é mesclado por variável e registrado no manifesto. As políticas
  de herança/filtros e as demais preferências do Codex permanecem intactas. Fonte inválida ou
  conversão incompleta nunca significa remoção. Valores de tokens de ferramentas são locais e
  não devem aparecer no painel, nos logs públicos ou no Git.
  A suíte desliga apenas os gatilhos automáticos com `CP_CODEX_SYNC_ENABLED=0`; testes do serviço
  usam diretórios temporários. Turnos reais do CLI 0.153.4 responderam exatamente `OK`, rc=0,
  zero eventos de ferramentas, em Linux (6,08s) e Windows (6,82s), em 06/09/2026. Usaram
  `HOME`/`CODEX_HOME` temporários com apenas `auth.json` copiado com autorização; cópias e
  diretórios foram removidos e a limpeza confirmada. Windows usou CLI puro, e o Desktop do
  usuário não foi alterado nem exercitado. Essa prova de resposta não valida execução dos
  plugins/hooks importados: os turnos não usaram ferramentas.
  **O que a revisão do PR #2 mudou, medido em 06/09/2026 com a importação real (CLI 0.153.4) sobre
  uma cópia do layout desta máquina** — 9 plugins habilitados, 18 entradas no `hooks.json`,
  `AGENTS.md` como link pro `CLAUDE.md`, 379 links de skills:
  - **A conversão dos hooks do usuário é do Codex, não do Hangar.** O importador descarta o que
    não conhece (`MessageDisplay` e `Notification` sumiram sozinhos) e copia cada script pra
    `~/.codex/hooks/`. O Hangar só faz o que ele não faz: `codex_compat` (rtk, `SessionEnd` ≤ 3s,
    bloco do `AGENTS.md`), a ponte de skills pessoais e os plugins.
  - **Os hooks do PRÓPRIO app não atravessam pelo importador** (`sem_hooks_do_app`): cada harness
    recebe o `state_hook` pelo instalador dele — `codex_hook_installer.py` no Codex, irmão do do
    Kimi —, e `adapters/codex/adapter.py` lê esse marcador como segunda fonte de "turno fechou",
    então ele precisa existir mesmo com a integração desligada. Sem o filtro, `askq_capture`,
    `preview_hook`, `pair_hook`, `nav_hook` e `subagent_hook` (que só entendem o stdin do Claude)
    iam junto. O instalador só ACRESCENTA: reescrever o comando muda o hook, e hook alterado é
    hook não aprovado no Codex.
  - **A primeira rodada adota o que o instalador antigo escreveu** (`_migrar_ponte_antiga`): o
    espelho `~/.codex/.hangar-hooks.json` é o registro exato do que `install-skills-bridge.sh`
    gravava, então ele diz o que sai, sem chute. Sem isso a máquina ficava com cada hook em
    dobro — 18 entradas viraram 37 na primeira rodada (a antiga em `~/.claude/hooks/` e a cópia
    nova em `~/.codex/hooks/`), `sync-skills.sh &` e `state_hook` 2× por evento. Depois: 17 (11
    do usuário + 5 de estado + rtk), segunda rodada em 1,0s sem reescrever nada. O instalador da
    subida já rodou quando a migração tira a entrada antiga, por isso ela reinstala na hora.
  - **O rtk embrulhado reusa o interpretador e o wrapper já gravados** (`wrapper_instalado`):
    `sys.executable` + o checkout de quem reconciliou reescreviam o comando a cada backend
    subindo de outra árvore (medido: worktree `.worktrees/pr2` no `hooks.json`), e cada
    reescrita invalida a aprovação.
  - **`AGENTS.md` como link pro `CLAUDE.md` vira arquivo com o bloco** — decisão do usuário: o
    Codex lê o `CLAUDE.md` pela instrução, e o `CLAUDE.md` nunca fica cristalizado numa cópia.
    Custo: as instruções globais deixam de estar no contexto desde o primeiro token.
    **Superada em 07/09/2026** pelo `AGENTS.override.md` (ver "Instruções nativas" neste arquivo):
    o bloco sai e o `CLAUDE.md` entra inteiro, por link, no primeiro request.
  - **O gatilho de sessão nasce ligado, com interruptor na tela e sob o kill-switch**
    (`sincronizacao_ligada`): `codex_sync` no `runtime-config` (card do Codex em Harnesses) +
    `automations_enabled()` + `CP_CODEX_SYNC_ENABLED` (desligamento duro, o da suíte). O botão
    "Reconciliar agora" não passa por nenhum dos três.
  - **Quem reconcilia é o backend; o lançador da TUI pede, espera até 20s e abre** (o PR fazia o
    lançador reconciliar sozinho, esperando o lock sem prazo — com uma instalação de plugins de
    65–103s o pane ficava minutos parado, e um teto que cancelasse a rodada nunca a deixaria
    terminar). Um executor só, e a instalação longa termina no backend.
  - **`~/.agents/skills` não é do Codex** (`codex_skills._duplicata_nativa`): é fonte do Pi, do
    Kimi e do omp, e o Codex a lê sozinho. A dedupe do PR apagava dali qualquer cópia idêntica à
    fonte do Claude, com ou sem plugin nativo envolvido — nesta máquina são 11 skills pessoais que
    existem nos dois lugares, e sumiriam dos outros três harnesses, caladas. Regra: skill que já
    está em `~/.agents/skills` não ganha link na ponte (o Codex já a vê); a dedupe só roda com
    plugin nativo confirmado e só retira o que o manifesto diz que o Hangar mesmo pôs lá; cópia
    pessoal fica, com aviso. Medido na cópia fiel desta máquina: a ponte vai de 375 links pra
    42 (só o que não vem de plugin nem de `~/.agents/skills`), 333 nativas, os 11 de
    `~/.agents/skills` intactos e sem link, zero avisos.
  - **Erro fora dos três tipos esperados deixava o estado preso em "executando"**: `hooks/list`
    num formato inesperado dava `AttributeError`, escapava do `except`, e o botão ficava cinza e o
    lançador esperava 20s a cada sessão até reiniciar o backend. Hoje qualquer exceção vira
    "erro" (detalhe só no log) e o formato do `hooks/list` é conferido antes de percorrer.
  - **Um `.md` que o Codex não reconhece não derruba a etapa** (medido no CLI 0.153.4: o
    detector aceita qualquer `.md` em `commands/`, inclusive sem frontmatter e em subpasta, mas um
    `README.md` em `agents/` fica de fora). O PR abortava hooks, env, MCPs e agentes inteiros quando
    a contagem não batia, toda rodada. Hoje o arquivo não reconhecido entra num aviso, é ignorado,
    e o artefato que já existia com aquele nome não é podado.
  - **Mensagem pra tela é código + parâmetros** (`app/codex_msgs.py`, `CATALOGO`; o front traduz
    por `harness_codex_m_<codigo>`). Uma `Mensagem` É uma `str` — log, lançador e testes seguem
    lendo o texto —, e `status()` a serializa em `{codigo, params, texto}`; código que o app não
    conhece cai no `texto`. Armadilha medida: `copy.deepcopy` numa `str` com `__new__` próprio
    reconstrói pelo VALOR (`KeyError: 'Concluído'`), daí o `__reduce__`/`__deepcopy__`.
  - `AbortSignal.any` só existe do Safari 17.4 em diante (`credenciais.ts:comTeto`); sem o
    fallback, um iPhone mais velho derrubava toda chamada de credenciais/harness.
  - O card mostra `skills: N na ponte, M nativas` do manifesto, no lugar do item "ponte de skills"
    que o PR tirou — sem isso, com a sincronização desligada ninguém via as skills paradas.
  - `settings.env` vai inteiro pro `shell_environment_policy.set` — as 16 variáveis desta
    máquina, 6 delas tokens (Grafana, Jira, Jenkins, Outline, ElevenLabs), também no
    `estado.json` do manifesto (0600). É o comportamento do importador nativo; o que o Hangar
    acrescenta é fazê-lo sozinho, daí o interruptor.

## Loop runner

(`app/loop.py` + `components/LoopSheet.svelte`): loop autônomo por sessão —
  goal → sessão trabalha → idle dispara tick (`_on_hook_transition`, dentro do `_work`, só com
  `sent == 0`) → roda `check_cmd` (exit 0 = `done`) ou procura `LOOP_DONE` (→ `done_claimed`,
  que SÓ fecha com confirmação humana via `/loop/resolve`) → senão re-prompta com a cauda do erro.
  Sidecar em `.hangar-loop/<nome>.json` (sobrevive `/clear`); guardrails: max_iters,
  branch≠main, kill-switch `automations_enabled`, anti-estagnação (mesma cauda 2×). Loop ativo
  **suprime o chain** da sessão. Campos `loop_status/loop_iter/loop_max` fluem no `/api/sessions`
  e no `sig` do SSE (badge 🔁 nas 2 views). Spec/decisões: docs/superpowers/specs/2026-07-22-*.md.

## Model engines

(`app/engines.py` + `app/engine_probe.py` + `components/settings/MotorForm.svelte`,
  aberto de dentro do card da chave em `ContasSettings.svelte`, tela "Contas e modelos" — a tela
  Motores foi fundida nela em 05/09/2026, pela spec de config por assunto):
  a session can run on a non-Anthropic provider — only env vars change inside that session's process,
  `~/.claude` (skills, hooks, transcript) stays the SAME. Single source of truth at
  `~/.claude/engines.json` (0600). Four invariants: (1) `engines.py` is **stdlib-only** — an
  `app.config` import there would pull in pydantic and break `scripts/hangar-engine`, which the shell
  calls with the system `python3`; (2) it's `ANTHROPIC_AUTH_TOKEN`, **never** `ANTHROPIC_API_KEY`
  (that one writes `customApiKeyResponses` into the global `~/.claude.json`); (3) the env is applied
  by `hangar-engine --exec <engine> -- claude …` (`os.execvpe` inside the pane) and **never** via
  `tmux -e`, because the key would land in `/proc/<pid>/cmdline`, world-readable — tmux doesn't
  inherit the caller's env, so there's no "just export it" path; (4) the context-window var is
  `CLAUDE_CODE_MAX_CONTEXT_TOKENS` — `CLAUDE_CODE_AUTO_COMPACT_WINDOW` measured inert on both
  providers tested, and without the right var Claude Code still compacts at ~167k on a 500k model.
  A live session's engine is read back from `/proc/<pid>/environ` (`CP_ENGINE`), same trick as
  `CLAUDE_CONFIG_DIR` — it's what keeps both resumes (`registry.resume` and the Archive one in
  `api.py`) from silently switching engines mid-conversation; Archive resume, unlike a live resume,
  has no process left to read, so it always re-asks. Models and context window come from
  `GET {base_url}/v1/models` — no static catalog, because the value varies by the user's
  subscription tier. The statusline only hides `💵`/cost-sidecar writes on an engine session — the
  effort chip (`(high✦)`) is untouched, it's not faked.

## `hangar-preview open` com a sessão FORA da tela

(`sse.nav_*`, `sessionsStore` →
  `lib/navPelaLista.ts`, `hangar:nav-open` com `oculto`, 05/09/2026). O pedido do agente era um
  `pop` em memória entregue ao PRIMEIRO stream da sessão que passasse: o celular lendo a mesma
  sessão comia o evento (medido: 7 conexões da VPS contra 2 locais) e o desktop nunca via; e
  reiniciar o backend perdia o pedido. Hoje é um marcador `{url, ts}` por sessão em
  `~/.hangar/nav/pendentes.json`, entregue **uma vez por conexão** nos dois streams — o da sessão
  e o da **lista**, que é o único que o desktop mantém aberto o tempo todo — e apagado quando o
  shell confirma que criou o view (`DELETE /nav`) ou em 10 min. Do lado do shell, o view nasce
  **escondido** (`setVisible(false)`) e já carrega: o agente dirige por CDP na hora, e o
  `NavegadorPane` só reexibe quando o usuário abrir a sessão. View já visível não é tocado pelo
  pedido oculto — ali quem manda é o painel montado. Trocar `main.cjs`/`preload.cjs` exige
  reabrir o app desktop; o front e o backend não. Três limites medidos no teste de ponta a ponta:
  (1) **um view escondido é uma página de 0×0, e isso era pior do que "não dá pra tirar print"**
  (medido 05/09/2026, Electron 43.3.0): `setVisible(false)` zera a viewport da página —
  independente dos bounds, que não a movem —, então `matchMedia("(max-width:600px)")` responde
  **true** e o agente que abria com a sessão fora da tela lia e clicava no layout de **celular**
  do app achando que era o de desktop; e não havia quadro, com `capturePage` **rejeitando**
  `UnknownVizError` (não devolvendo imagem vazia) e `Page.captureScreenshot` pendurando. O que
  desamarra a página do compositor é `Emulation.setDeviceMetricsOverride` (1280×800): a viewport
  volta, a media query volta pro desktop e o `captureScreenshot` responde em ~60ms — o print de
  view escondido passou a existir. Três regras que caíram junto: a emulação **só pode entrar com
  a página carregada** (aplicá-la no `about:blank` de um view recém-criado derruba o processo com
  **SIGSEGV**, reproduzido 3×, e é por isso que `avisarOculto` espera o `did-finish-load`);
  `capturePage` e `captureScreenshot` **não são intercambiáveis** — o primeiro serve o view
  visível, o segundo o escondido, e usar o segundo sem a emulação é o que pendura; e
  `setBackgroundThrottling(false)` **não tem efeito nenhum** aqui (medido: A, B e C da sonda
  saíram todos vazios), o que descarta portar o `acquireAgentWake` do Superset — lá o webview
  fica visível e parqueado, aqui o view é desligado no compositor. Quem sabe do estado é o
  controlador (`definirOculto`), porque é ele que já reaplica emulação depois de navegar;
  (2) a sessão que ganhou navegador fora da tela entra
  direto na aba **Navegador** ao ser aberta (`DesktopSessionContext`, só quando ela nunca
  escolheu aba); (3) **tudo isso é do layout desktop**: com a janela do Electron abaixo de 820px
  (estava com 757px numa tile do Hyprland) o app está no layout de celular — sem sidebar, sem
  stream da lista, sem `NavegadorPane` — e o pedido só marca o store. O `dist` novo ainda passa
  pelo service worker: reload comum serve o bundle velho, é Ctrl+Shift+R.

## Modo de permissão troca COM a sessão trabalhando

(`api._guard_perm`, `permission_mode.py`,
  medido 05/09/2026): BTab é tecla, não texto — com `✻ Ebbing… (6s · thinking)` na tela, um BTab
  levou de bypass pra auto na hora, como no Pi. O guard de "está trabalhando"
  (`terminal._require_drivable`) existe pro `/model`, que é TEXTO e cairia no campo de entrada
  como mensagem; aplicado à permissão ele recusava com 409 o que o terminal aceita. O que resta
  no guard é menu aberto no pane (engoliria a tecla) e painel de terminal aberto. Do lado do
  app: o atalho (Alt+Shift+P, e Shift+Tab com foco no campo — a tecla do terminal) **sonda** o
  ciclo quando não há cache; antes pedia sem `sondar`, recebia `[]` e morria calado até a
  pílula ser aberta uma vez (0 POSTs em 7 dias de log). Ctrl+L foca o campo de qualquer lugar.
  Ciclo: sessão nascida em bypass tem 5 posições (bypass → auto → manual → acceptEdits → plan);
  as outras, 4 — bypass nunca é alcançável de fora, e `dontAsk` não tem volta.

## Modelo de uma sessão Claude Code: a lista NUNCA é constante

(`app/model_picker.py` +
  `terminal_input.list_model_options` / `set_engine_model` + `app/default_model.py` +
  `components/ClaudeModelPopover.svelte` + `components/ClaudeEffortPopover.svelte`). Duas fontes, escolhidas pelo que a sessão é — medido em
  31/07/2026, claude 2.1.220:
  - **Conta Anthropic** → as linhas do próprio picker do `/model`, lidas ao vivo (abre, parseia,
    Esc). A lista `['default','opus','sonnet','haiku']` chumbada no front envelheceu: o picker real
    tem 5 linhas com o **Fable** entre Opus e Sonnet, então o app escondia um modelo e ainda dava a
    Sonnet/Haiku o número de linha errado. `MODEL_NUMBERS` sobrou só como fallback pra linha rolada
    pra fora da viewport. Cache de **1 hora** por config dir porque ler a lista **dirige o
    terminal** e isso deixa rastro: `❯ /model` + `⎿ Kept model as …` (o Esc de saída) ficam no
    scrollback do tmux pra sempre. Não polui o chat do app — entra no jsonl como `type: system`,
    que o `transcript.py` ignora —, mas quem estiver com aquele terminal aberto vê, e cinco
    leituras empilhadas ali já pareceram bug. Esperar o **rodapé** (`Esc to cancel`), não só o
    título, antes de parsear: no instante em que o título aparece as linhas ainda estão sendo
    pintadas e a leitura devolvia 4 modelos, sem o Haiku. E nunca mandar o 2º Enter sem antes
    reler: se o picker já abriu, esse Enter **confirma como default** a linha sob o cursor — num
    caminho que era pra ser só leitura.
  - **Sessão de motor** → o `/v1/models` do provedor (o mesmo `engine_probe` do "Testar e listar
    modelos" de Contas e modelos).
    Ali o picker é inútil: lista os 4 aliases, **todos apontando pro mesmo `ANTHROPIC_MODEL`**
    (`Custom Opus model`, `Custom Fable model`, …) — e `gateway_model_discovery: true` não muda
    isso. A troca vai por `/model <id>`, que aceita id arbitrário.
  Três armadilhas medidas: (1) `/model <id>` grava o id como **default GLOBAL** no
  `settings.json` ("saved as your default for new sessions") — uma sessão nova da conta Anthropic
  nasceria pedindo `kimi-for-coding`; `default_model.restore_quando_aterrissar` repõe o valor
  anterior, e **espera a escrita chegar**, porque o arquivo só muda ~0.8s depois do Enter (repor
  antes é um no-op e o vazamento aterrissa em seguida); (2) a linha `⎿ Set model to …` da troca
  ANTERIOR continua na tela, então a confirmação só vale se **mencionar o id pedido** — sem isso a
  primeira leitura devolvia a resposta da troca passada como se fosse desta; (3) o guard de "posso
  digitar agora?" usa **duas capturas**, não uma: um pane parado não distingue spinner vivo de
  marcador de turno concluído (está na docstring do `state.classify`), e uma captura só recusava,
  com "está trabalhando", uma sessão que tinha acabado de terminar.

## As extensões de FUNCIONAMENTO da experiência Claude no Pi moram aqui

(`scripts/pi/`,
  04/09/2026): `claude-bridge.ts` (agents/commands/skills do `~/.claude` como recursos do Pi),
  `claude-todo.ts` (painel de tarefas), `claude-hooks-adapter.ts` (hooks do `settings.json` nos
  eventos do Pi), `git-checkpoint.ts` (`/rewind`) e `fullscreen-tui.ts` (alternate screen no Pi)
  vieram do repo `pi-claude-bridge`, que ficou só com aparência (caixa da mensagem, título do
  terminal e temas). Motivo: sem elas uma sessão Pi criada pelo app não enxerga skills/agents nem
  roda hooks, e quem instala o Hangar não deveria precisar de um segundo repo pra isso. O
  `install-claude-wrapper.sh` symlinka as sete no Pi e cinco no OMP: neste, `claude-todo` e
  `fullscreen-tui` ficam com o núcleo. O painel de saúde usa a mesma seleção por CLI e não
  oferece fullscreen no OMP. No Pi com fullscreen nativo, a extensão não assume o buffer
  alternativo para evitar dupla posse.
  **Comparação com OMP 18.1.11 (05/09/2026):** em uma HOME descartável, sem chamadas a modelos,
  `getAllTools()` mostrou que `claude-todo` substituía a ferramenta `<builtin:todo>` pela extensão:
  o contrato `action/id/activeForm` tomava o lugar de `op/task/phase`, fases e bloqueios usados
  pelo próprio núcleo. Com a proteção no entrypoint, a origem continua `builtin`. A proteção
  também cobre quem atualiza só com `git pull`, sem rodar o instalador.
  Na mesma prova com o binário real e sockets tmux separados, o nativo usou
  `alternate_on=0, mouse_any_flag=0`; fullscreen externo ativado usou `1,0`. Após a correção,
  mesmo carregado explicitamente com `enabled: true`, permaneceu `0,0`. O OMP depende do
  scrollback normal; colocar a conversa no buffer alternativo não cria um renderizador com
  rolagem (issues can1357/oh-my-pi#10232 e #2040).
  **Migração não edita preferências:** instalador e reparo removem somente symlinks próprios
  dessas duas extensões. Arquivos reais, symlinks para outra fonte e `fullscreen-tui.json`
  ficam intactos. A configuração do OMP não é reescrita.
  **Não estender essa conclusão às demais extensões.** A descoberta nativa do OMP respeita
  registros/escopo de plugins e reduz a necessidade de espelhar skills e comandos, mas não
  importa todos os agents pessoais do Claude, sua normalização de modelos nem o índice de
  memória. Hooks JS/TS nativos não executam automaticamente o protocolo CLI do `settings.json`.
  E checkpoint/rewind nativos reduzem contexto: não restauram arquivos como o shadow Git do
  Hangar. Essas capacidades continuam complementares, não substituídas por nome.
  A seleção das extensões não demonstra compatibilidade completa: o bridge e os checkpoints
  têm as provas específicas abaixo; limitações do adaptador de hooks continuam separadas.
  **Bridge adaptado ao OMP (05/09/2026):** `lib/agent-context.ts` resolve a identidade pelo
  executável e normaliza `PI_CODING_AGENT_DIR`/`CLAUDE_CONFIG_DIR`, incluindo `~`; a fábrica
  guarda esse contexto por instância. **Os helpers compartilhados moram em `scripts/pi/lib/` e
  o instalador (e `harness_saude._ligar_extensoes`) linka a PASTA `extensions/lib`** (medido
  06/09/2026, pi 0.85.0): o loader do Pi resolve import relativo pelo caminho do symlink, não do
  arquivo real — `./agent-context` ao lado de `claude-bridge.ts` dava `Cannot find module` e o
  Pi saía com rc=1 sem ponte nem `/rewind`; e um `.ts` solto em `extensions/` é carregado como
  extensão (`does not export a valid factory function`). Pasta sem `index.ts` o Pi ignora. O omp
  (Bun) resolve pelo realpath e carregava de qualquer jeito — foi por isso que a suíte, que só
  roda o omp, não pegou. **"Estou no omp?" tem UMA resposta**, `getAgentContext().harness`:
  `claude-todo.ts` e `fullscreen-tui.ts` tinham o regex do `execPath` copiado, e uma mudança de
  empacotamento do omp corrigida no `lib/` deixaria as duas religando no omp o que tem que ficar
  desligado. Miudezas fechadas junto (06/09/2026): nome de agente repetido entre fontes no omp
  entra em `skipped` (o caminho lá é plano, o segundo era descartado calado); o desfazer de uma
  ação do plugin sync que falha loga e relança a **causa original** (antes o `finally: raise`
  punha o erro do desfazer no relatório); `_digest` guarda assinatura (mtime, tamanho) por
  arquivo e só relê quando ela muda (o laço de 300 s relia todo byte de todo plugin); e
  `observe_controls` lê as 3 chaves do `omp config get` em paralelo e a releitura só pega
  `disabledExtensions` — de 6 processos em série (~0,75 s cada, na subida do backend) pra 4 em
  dois lotes.
  **A raiz do agente omp tem UMA resposta: `app/omp_dirs.agent_dir()`** (06/09/2026). O omp
  com perfil (`--profile x` ou `OMP_PROFILE=x`) grava TUDO — login, sessões, config, plugins —
  em `~/.omp/profiles/x/agent` (medido no 18.1.10 numa HOME descartável). O plugin sync e o
  contexto vieram com `resolve_omp_directories`, que espelha essa regra; sessões
  (`sessions_root("omp")`), painel de saúde (`_raiz_agente`) e login do ChatGPT (`_omp_db`)
  continuavam em `~/.omp/agent` sem perfil — com `OMP_PROFILE` no ambiente do serviço, o sync
  instalava no perfil e o painel dizia "não instalado". Hoje os três perguntam ao `omp_dirs`,
  que só embrulha o resolvedor do sync (import tardio: `sessions.py` é folha e não pode puxar
  `peers` na importação) e, pra quem só LÊ, perfil inválido vira aviso e raiz sem perfil —
  levantar ali derrubaria a listagem de sessões inteira.
  **Perfil por SESSÃO** (06/09/2026): o perfil de um pane omp viaja como `OMP_PROFILE` no
  ambiente dele — o wrapper (`omp.posix.sh`/`omp.fish`, `hangar_omp_perfil`) lê `--profile x`
  da linha ou a variável já exportada, monta o `--session` na raiz do perfil e passa `-e` pro
  tmux (o pane nasce do servidor, não do shell); o app faz o mesmo pelo `env` do
  `OmpAdapter.spawn_command(perfil=...)`. Do outro lado, `registry._omp_profile_of(pid)` lê a
  variável do processo vivo (mesmo `/proc/<pid>/environ` de `CP_ENGINE`) e passa pra
  `transcript_path`/`localizar_na_raiz`, senão a varredura de `sessions/-/` caía na raiz do
  BACKEND. Entrada: `omp_profile` no `POST /api/sessions` (só com `provider=omp`, nome validado
  pela regra do próprio omp; 400 fora dele), `hangar-send --new … --provider omp --profile x`,
  e o campo "Perfil do omp" da folha de Nova sessão, que só aparece com OMP escolhido.
  Variável e não flag de propósito: `--profile` no cmdline funcionaria pro omp mas o backend
  teria duas fontes pra ler. **O que ainda NÃO olha as pastas de perfil:** o relatório de
  custo (`costs_sources.raiz_omp`) e o Arquivo de conversas mortas (`archive_providers`) leem
  só a raiz do backend — uma sessão omp criada com perfil funciona ao vivo, mas some das duas
  telas depois de fechada. Cobrir isso é varrer `~/.omp/profiles/*/agent/sessions` além da
  raiz, e ainda não foi feito. Quem GRAVA na raiz do omp (`oauth_codex._omp_db`) usa
  `omp_dirs.agent_dir(estrito=True)`: perfil inválido levanta, em vez de cair calado na raiz
  sem perfil com a credencial gravada no lugar errado. No OMP, agents pessoais/extras viram arquivos diretos
  em `<agentDir>/agents/claude-bridge-<nome>.md`, com ferramentas em array YAML: `Glob → glob`,
  `Task/Agent → task`, `WebFetch → read` e prefixo `mcp__` intacto. Agents nativos pessoais
  têm precedência; aliases Claude sem mapeamento explícito herdam o modelo da sessão.
  Isso inclui `fable`. Negações explícitas (`disallowedTools`) são subtraídas da allowlist;
  sem uma allowlist ou com negação não representável, o agent é recusado, não ampliado.
  Nomes `main`/`sub`, reservados pelo núcleo OMP, também são recusados nesse harness.
  Skills/comandos/plugins não são espelhados no OMP, nem oferecidos no menu de fontes.
  No Pi permanecem a conversão de ferramentas e o layout recursivo de agents, prompts e skills.
  `lib/frontmatter.ts` usa `Bun.YAML.parse` no OMP e carrega o parser legado somente no Pi.
  Memória respeita `enabled`, preserva blocos do prompt e não reinsere conteúdo já presente;
  `claude-bridge.json` ilegível é logado e ignorado no `before_agent_start`, nunca lançado.
  O manifesto versão 2 registra conteúdo e caminho relativo de cada arquivo gerado: atualização
  e remoção exigem os bytes originais, e conflitos são preservados e reportados. **O manifesto
  v1 é ADOTADO uma vez** (`adoptLegacy`, 06/09/2026): ele só listava nomes de prompts, e a pasta
  `agents/claude-bridge/` era inteira da ponte — os dois já eram sobrescritos e apagados por ela,
  então adotá-los lendo o disco não tira segurança nenhuma. Tratar v1 como vazio (a primeira
  versão do PR) deixava cada instalação existente com todos os arquivos em `skipped` para
  sempre, e sem volta, porque o v2 vazio já tinha sobrescrito o v1 (nesta máquina: 16 prompts
  e três pastas de agents). Escritas usam arquivo temporário + rename. **Fonte com frontmatter
  inválida pula só ela** (entra em `skipped` com o motivo) e, enquanto houver uma, a ponte cria e
  atualiza mas **não remove nada** — sem ler a fonte não se sabe qual cópia ela geraria, que era
  o risco que o abort da primeira versão evitava ao custo de um `.md` quebrado em qualquer
  marketplace derrubar o sync inteiro. Isso adapta a ponte, não substitui o instalador nativo
  de plugins.
  Prova: `tests/test_claude_bridge_omp.py` roda o OMP real com HOME própria; o driver exige
  descoberta no catálogo de `task`, grava resultado estruturado em `session_start` e encerra
  sem prompt/modelo remoto. `rc=0` sozinho não prova carregamento de extensão.
  **Checkpoints por contexto no OMP (05/09/2026):** `git-checkpoint.ts` consome o mesmo
  `lib/agent-context.ts` e registra `/hangar-rewind`; o Pi mantém `/rewind`. A captura usa
  `before_agent_start`, não `turn_start`, e persiste revisão, worktree canônica, identidade do
  Git do projeto e diretório que contém os objetos. O próprio registro é a âncora anterior ao
  pedido. Retomada/fork conservam essa origem; `getBranch` impede oferecer um ramo descartado.
  **Uma pasta de checkpoints por SESSÃO** (`<agentDir>/checkpoints/<slug do jsonl>`, reusada
  na retomada, 06/09/2026): a primeira versão do PR abria `<slug>-<uuid>` a cada ativação,
  inclusive em cada resume, e cada pasta guarda os objetos da árvore inteira — 113 pastas e
  1,2 GB nesta máquina, sem poda. O uuid existia pra duas instâncias da mesma sessão não
  disputarem o índice; hoje o índice é **por captura** (`index.<pid>.<uuid>`, apagado no fim),
  então objetos e refs (já nomeadas por uuid) convivem num bare repo só. Sufixo `-<uuid>` só
  quando a pasta com esse nome é de OUTRO projeto (`hangar-origin.json` diverge) ou não é um
  bare repo; a pasta v1 do Pi (mesmo slug, sem origem gravada) é adotada e ganha a origem.
  Restaurações continuam com índice temporário próprio, nunca o da sessão de origem.
  **A captura enumera numa chamada só**: `ls-files -t -s --cached --others --deleted
  --exclude-standard` — `H`/`S`/`M` rastreado com modo (`160000` = submódulo, fora), `?` novo,
  `R` rastreado que sumiu do disco. A primeira versão fazia um `lstatSync` síncrono por arquivo
  rastreado mais 8–9 spawns por prompt; num repo de milhares de arquivos isso travava o loop de
  eventos antes de cada mensagem. Árvore igual à da última foto **reaproveita a revisão**
  (`lastTree`/`lastRef` na sessão ativa): todo pedido ganha registro, não commit.
  **Captura lenta ou falha NÃO mata o turno.** A primeira versão chamava `ctx.abort()` no omp
  ao estourar 25 s, em qualquer erro de captura e em `agent_start`/`turn_start` com captura
  pendente — um repo grande cancelava TODO prompt. O que importa (nada tardio entra no turno)
  é o cancelamento da captura, que fica; o abort saiu. Hoje é aviso "este pedido segue sem
  checkpoint" e o turno anda; o `/rewind` só tem um ponto a menos. O Git do projeto só
  enumera arquivos/exclusões; variáveis `GIT_*` herdadas são removidas, hooks/assinatura/fsmonitor
  são desativados e atributos do shadow preservam bytes, inclusive CRLF, sem filtros de conteúdo.
  O modo de código repõe arquivos modificados/apagados, preservando os criados depois. Origem,
  projeto, revisão, ramo, diretório e ociosidade são conferidos antes da escrita; symlinks
  ancestrais ou diretórios posteriores em colisão recusam a operação.
  **O await do OMP tem prazo:** no 18.1.11 o dispatcher libera handlers após 30 s. A captura
  tem limite total de 25 s, incluindo espera na fila, e cancela os processos Git antes desse
  limite nativo (o pedido segue, ver acima). `agent_start`/`turn_start` também invalidam qualquer
  captura restante; ela não pode publicar um checkpoint tardio no turno em execução.
  Prova usa `ExtensionRunner`/`loadExtensionFromFactory` reais, sem chamada a modelo; reproduziu
  a publicação tardia ao expirar o dispatcher e passou após o cancelamento.
  **As provas do omp reusam o addon nativo da máquina** (`tests/omp_runtime._reusar_natives`,
  06/09/2026): o omp extrai `pi_natives` (~344 MB) em `~/.omp/natives/<versão>` da HOME que
  vê, e cada caso tem HOME própria — com as 3 rodadas que o pytest guarda, um `/tmp` em tmpfs
  de 12 GB lotou NO MEIO da suíte e derrubou 756 testes com `No space left on device`, todos
  longe do omp. A HOME de teste ganha um symlink `.omp/natives` (e `.cache/omp/natives`) pro
  real quando ele existe; sem ele (CI limpo) o omp extrai como sempre.
  Registros Pi antigos só são restaurados quando a sessão original, seu `header.cwd` e o
  armazenamento legado previsto demonstram a origem; não se procura um SHA por pastas alheias.
  Código e conversa são etapas separadas: falha da segunda é informada como parcial, não sucesso.
  Regras herdadas do adapter: a allowlist embutida libera só `~/.claude/hooks/` — hook que mora
  noutro lugar entra por `~/.pi/agent/claude-hooks-adapter.json`, e `allowPatterns` ali
  **substitui** a lista, não soma; e os hooks só-Claude do próprio app (`state_hook`, `askq_capture`,
  `preview_hook`, `subagent_hook`) ficam no `skipPatterns` porque o Pi tem extensão própria pra isso.
  **Catálogos e plugins nativos (06/09/2026):** `app/omp_plugin_sync.py` oferece
  `PluginSynchronizer.import_marketplaces` e `reconcile`. A importação percorre todos os
  marketplaces registrados no Claude, sem nomes especiais, e chama o gerenciador nativo do
  OMP. Confere nome/origem no registro após o comando; catálogo homônimo divergente permanece
  intacto. Importar catálogo não instala seus plugins nem migra instalações Git existentes.
  O OMP já oferece `marketplace.autoUpdate=off|notify|auto`, com padrão `notify`; a atualização
  nativa por versão do catálogo ocorre na abertura da sessão e não é duplicada pelo Hangar.
  A reconciliação de Git direto exige origem, revisão e manifesto instalável comprovados,
  preserva escopo, seleção de recursos e preferências, e suspende a gestão após alteração
  manual. Metadados Claude sem SHA tornam somente aquele candidato não verificável.
  Em atualização, prepara apenas a dependência gerenciada antes de chamar o instalador:
  isso evita arestas duplicadas no Bun quando o parser OMP não reconhece `#SHA` em host genérico.
  Uma falha só reverte essa chave se a instalação anterior ainda estiver comprovadamente
  intacta; não remove o plugin antes da atualização nem restaura cópia global antiga.
  O registro próprio fica em `~/.hangar/omp-plugin-sync.json`, com trava portátil compartilhada
  entre passagens e escrita atômica. Operação interrompida não concede autoridade de remoção.
  Todos os vínculos e estados do ledger são validados antes de chamar o CLI ou agir; registro
  malformado não é reparado por inferência e não autoriza remover um plugin. Duas identidades
  Claude para o mesmo pacote tornam o nome ambíguo durante toda a passagem, inclusive diante
  de uma terceira origem; os candidatos independentes continuam. Diagnósticos não publicam
  texto bruto de exceções de parser/I/O, que pode transcrever credenciais da entrada.
  `dry_run=True` é somente leitura de registros/manifestos: nenhum CLI, lock, cache ou ledger
  é escrito, pois até `omp plugin list` pode migrar arquivos. Provas cobrem o CLI real, Git
  Smart HTTP em loopback privado, importação genérica e preservação da instalação nas falhas.
  **Resolução de diretórios:** `resolve_omp_directories` separa configuração, agente e dados
  conforme o OMP. `PI_CODING_AGENT_DIR` não move o armazenamento global. `PI_CONFIG_DIR`,
  precedência de `OMP_PROFILE` sobre `PI_PROFILE` (inclusive vazio), override herdado e XDG
  seguem as regras nativas. A categoria XDG exige caminho existente e agente padrão; perfis
  nomeados exigem o caminho XDG daquele perfil. A resolução é lexical, usa o cwd do filho e
  não expande `~`, não segue symlinks e não cria diretórios. Cada passagem tem sua própria visão;
  mudar o destino não migra o ledger antigo: o vínculo incompatível gera diagnóstico.
  **Passagens periódicas:** `PluginSyncLoop` é criado/encerrado no lifespan do backend, sem
  serviço externo. `CP_OMP_PLUGIN_SYNC_ENABLED` é falso por padrão; intervalo positivo e finito
  em `CP_OMP_PLUGIN_SYNC_INTERVAL` (300 s). Respeita também `automations_enabled()`. A primeira
  passagem começa na subida e a seguinte espera o intervalo após a conclusão da anterior.
  Importação/reconciliação rodam em `asyncio.to_thread`; desligar aguarda o worker em voo,
  não cancela uma Future deixando o processo externo vivo. Uma parada observada fica registrada
  até terminar a passagem, mesmo que o kill-switch seja reabilitado nesse intervalo.
  `GET /api/omp/plugin-sync`, autenticado, expõe estado, horários e relatórios sanitizados.
  Prova com backend real confirmou resposta HTTP enquanto o worker aguardava, e marcador de
  teardown confirmou o encerramento cooperativo. Nenhuma página nova foi introduzida.
  **Contexto CLAUDE.md:** `CP_OMP_CLAUDE_CONTEXT_ENABLED=1` habilita a configuração na subida
  pelo módulo `omp_context`. Reusa o resolvedor nativo de diretórios, vincula APPEND_SYSTEM.md
  ao CLAUDE.md global existente e instala/reusa a regra genérica de leitura do projeto.
  Arquivo/link personalizado em conflito é preservado e informado; arquivo global ausente
  não vira link quebrado. A lista disabledExtensions é mesclada pelo CLI nativo somente com
  `context-file:project:AGENTS.md` e `context-file:user:AGENTS.md`, sem retirar outras escolhas.
  Equivalência exige corpo compatível e frontmatter comprovadamente habilitado/incondicional,
  lido pela biblioteca YAML já usada no backend. Só arquivos diretos .md/.mdc participam, como
  no provider nativo; o nome/ID vem do arquivo. disabledExtensions, ttsr.disabledRules e
  disabledProviders são conferidos sem remover bloqueios pessoais. Precondições de arquivo,
  regra e diretório são repetidas após a trava e chamadas externas; rules/ convertido em
  symlink é recusado antes de publicar a regra, preservando o alvo externo.
  O CLI pode normalizar formatos legados de configuração, como o tema escalar para theme.dark,
  mantendo a preferência efetiva. OMP real confirmou as sentinelas CLAUDE global/de projeto
  e ausência das sentinelas AGENTS no prompt antes de ferramentas; projeto sem CLAUDE não
  recebe conteúdo inventado. Outros harnesses e arquivos AGENTS.md dos projetos não são alterados.

## Pi model + thinking level

(`app/pi_models.py` + `scripts/pi/hangar-state.ts` + `components/PiModelPopover.svelte` + `components/PiEffortPopover.svelte`):
  the third mechanism, next to Claude's TUI picker and Codex's app-server, and it does **not** scrape
  the pane. Measured on pi 0.82.1: `/model` is a fuzzy-**search** list of ~300 entries (footer
  `(1/301)`, 10 rows visible) — not enumerable from the pane and not navigable by counting `Down`;
  and there is no `/thinking` command (it lives inside `/settings` → "Thinking level", a submenu).
  So the Pi extension we already ship publishes a catalog sidecar
  (`<config>/.hangar-pi/models/<jsonl-stem>.json`, same key as the state marker) and registers
  `/cp-model <provider> <id>` + `/cp-think <level>`, which the backend types with `send-keys` and Pi
  applies through `pi.setModel()` / `pi.setThinkingLevel()`. Two invariants: (1) the thinking levels
  are **per model** (glm-5.2 → off/low/medium/high/xhigh; k3 → low/high/max), so they come from the
  session, never from a constant — the static `LEVELS` tuple only rejects garbage before typing;
  (2) Pi **clamps** the level to what the model supports (`agent-session.js:1277`), so the endpoint
  re-reads the sidecar and returns what *stuck*, not what was asked (asking `max` on glm-5.2 lands on
  `xhigh`). Missing sidecar → 409 telling the user to re-run `install-claude-wrapper.sh`, never an
  empty list that reads as "no models".

## `omp` (oh-my-pi) é um FORK do Pi, e é por isso que ele engana

(`adapters/omp/` — `OmpAdapter`
  é subclasse do `PiAdapter`; wrappers `scripts/shell/omp.*`). Mesmo JSONL, mesma API de extensão,
  as MESMAS `scripts/pi/*.ts` — o que muda é pequeno e cada item já custou um bug calado. Medido em
  02-03/09/2026, omp 18.1.4 (embute `pi-coding-agent` 0.84.4):
  - **Binário ELF nativo com argv0 `omp`**, não um fork do processo `pi`. Sem a entrada própria em
    `_EXEC_PROVIDER` o pane cai no default `claude` e é casado com o transcript do **Claude** do
    mesmo cwd — a regressão que o Pi já pagou.
  - **Raiz `~/.omp/agent`**, pela env `PI_CODING_AGENT_DIR` — que é variável do `pi-coding-agent` e
    move os DOIS agentes, então ela nunca é lida como "a pasta do omp" sem se lembrar disso (o Pi
    usa `PI_CODING_AGENT_SESSION_DIR` + `~/.pi/agent`). As extensões vão em
    `~/.omp/agent/extensions/`, que não existe até alguém criar.
  - **Não existe `--session-id`** (`Error: unknown flag`) — o id é do omp (uuidv7). Quem escolhe a
    sessão é o CAMINHO: `--session <arquivo>` (não documentado) cria o transcript exatamente ali.
    Por isso app e wrapper montam `<raiz>/<slug do cwd>/<ts>_<uuid>.jsonl` e exportam
    `CP_PI_SESSION=<uuid>` junto: com o bilhete da extensão como ÚNICO vínculo, um bilhete recusado
    deixa a sessão sem transcript até o próximo `agent_start` — que só chega se alguém conseguir
    mandar prompt, o que o app não consegue numa sessão untracked. Resume é `-r <caminho>` (o id
    interno do omp não é o do nome do arquivo).
    **E o diretório do `--session` não é honrado (omp 18.1.6, medido 04/09/2026):** o transcript
    principal nasce em `sessions/-/<nome>.jsonl` (o `-` é o slug de um cwd vazio) e só os
    subagentes vão pra pasta pedida — enquanto o `getSessionFile()` que a extensão publica no
    bilhete continua devolvendo o caminho pedido, que não existe. A sessão aparecia vazia no app
    com a TUI cheia. `pi_sessions.localizar_na_raiz` procura o mesmo NOME de arquivo em qualquer
    pasta da raiz (`registry.pi_session_file` e `transcript_path`, só pro omp); o Pi honra o
    diretório e continua restrito à pasta do cwd.
  - **Eventos com outro nome:** `agent_end` e `model_changed`, não `agent_settled` nem
    `model_select`. Sem tratá-los, o estado ficava preso em `working`. E a troca de modelo pelo app
    dava falso "o Pi recusou a troca" por um motivo mais fundo, achado na revisão final: o
    `setModel` devolve `true` e o rodapé troca, mas **`ctx.model` continua o modelo velho** e o omp
    não emite evento de modelo nenhum (`model_changed` também não dispara aí; `pi.getModel` não
    existe). Publicar `ctx.model` depois do `setModel` republicava o modelo VELHO com `ts` novo, e
    o backend lê "ts novo com modelo velho" como recusa. Daí o `override` de `publishModels`
    (`scripts/pi/hangar-state.ts`): quem sabe o modelo certo é quem acabou de pedi-lo. Troca feita
    no teclado do omp não tem evento, então o `agent_start` republica quando `ctx.model.id` difere
    do último publicado.
  - **Subagente roda no MESMO processo**, sem `PI_SUBAGENT_DEPTH`, emite `session_start` com o ctx
    dele e grava em `<stem>/<NomeDoAgente>.jsonl` (o Pi: `<stem>/<taskId>/run-N/session.jsonl`).
    Sem o portão, a extensão do subagente reescrevia o bilhete do pane e o histórico da sessão
    virava a conversa do subagente.
  - **`/reload` NÃO recarrega extensão editada nem descobre arquivo novo** (medido 2×): editou a
    extensão, reabra a sessão. Mesmo aviso do Pi, só que ali o `/reload` resolve.
  - **A ferramenta de perguntar chama `ask`** (no Pi é `question`), com shape multi-pergunta e um
    cartão de resumo acima do picker, mais as linhas de descrição de cada opção — quem conta linha
    de tela pra escolher opção precisa contar essas também.
  - **Catálogo de modelos é `omp models --json`** — não há `--list-models`.
  - **Sem chip de cota:** `~/.omp/agent` não tem `auth.json` nem `models.json` (é SQLite:
    `agent.db`, `models.db`), e `cotas._chaves_do_pi` lê exatamente esses dois arquivos, do `~/.pi`.
  - **No Windows a sessão omp depende SÓ do bilhete da extensão.** `_com_env` (`adapters/omp/
    adapter.py`) prefixa `env CP_PI_SESSION=<uuid>` porque o tmux não repassa o ambiente de quem
    chama — e `env` não existe lá, então o ramo `os.name == "nt"` devolve o comando cru. Bilhete
    recusado = sessão sem transcript. O conserto existe e não foi feito por falta de medição
    naquela máquina: `tmux new-session -e` funciona no psmux (é o que `tmux._e_config_dir` já usa),
    então dá pra mandar a variável pelo `-e` em vez do prefixo.
  - **Subagente do omp não aparece no painel de Atividade.** `subagents.py` é Pi-puro por duas
    travas independentes: `_pi_agents_dir` exige que o transcript esteja sob `sessions_root("pi")`
    (o do omp está sob `~/.omp/agent`), e o leitor espera o layout `<stem>/<taskId>/run-<n>/
    session.jsonl` — o omp grava `<stem>/<Nome>.jsonl`, sem `run-N`, então `_pi_run_dir` devolveria
    `None` mesmo com a raiz certa. Lista vazia, não erro.
  - **Executor omp no orquestrar usa o inventário do Pi.** `orq_politica.inventario` chama
    `pi_catalog.listar()` sem argumento (= `pi`), coerente com "mesmo inventário, sem linha
    própria" da política. Consequência a saber: `omp models --json` pode listar menos modelos que
    `pi --list-models`, e a tela do orquestrar oferece a lista do Pi.
  - **`rich-status-line.ts` honra `PI_CODING_AGENT_DIR` também numa sessão Pi.** A raiz sai de
    `process.env.PI_CODING_AGENT_DIR || ~/.{omp,pi}/agent`, e é dela que saem `auth.json` e
    `models.json` (o chip de cota). Quem exporta a variável no shell por causa do omp muda de onde
    o **Pi** lê os dois.

## Before typing into the Pi's composer, ASK — the screen cannot tell a notice from a draft


  (`terminal_input._composer_ocupado_pi` + `pi_inbox.perguntar` + `responderPergunta` in
  `scripts/pi/hangar-state.ts`). Pi prints extension notices (`console.error`) **inside the composer
  band**, with the same ANSI as typed text; measured 22-23/08/2026, `cursor_flag` is 0 either way.
  So the anti-paste guard counted our own `[hangar-state] linha do hangar conectada` as a draft and
  every `/cp-model`/`/cp-think` came back **409 with the composer empty**. Recognizing each phrase by
  regex is whack-a-mole (`/reload` draws a fourth one no regex of ours knows), and the "compare two
  captures — a notice is static, a draft changes" upgrade the code itself proposed **was measured and
  does not hold**: a *parked* draft is static too, and the parked draft is exactly what the guard
  exists for. What answers is the Pi: `ctx.ui.getEditorText()` returns `""` with a notice on the band
  and the exact text with a draft. So the `pi_inbox` line, until then delivery-only, took a second
  verb — `{id, pedir}` out, `{id, resposta}` back. Four rules: questions live in a **separate**
  futures dict from deliveries (a delivery resolves `(ok, erro)` and a question resolves a value);
  `""` is an **answer** and `None` is absence (→ fall back to scraping, so an old extension behaves
  exactly as before); the question does **not** take `linha.lock` (that lock orders *writes*, and a
  read must not queue behind a 3s ACK); and it uses `pi_inbox.linha_de(name, pane_id)`, never the raw
  pane. Note `/reload` drops and re-raises the line — a command fired inside that ~5s window falls to
  plan B and can still 409.

## Contas Codex adicionais têm origem própria

(`app/codex_contas*.py`, 10/09/2026): a padrão usa
  o `CODEX_HOME` atual; cada secundária tem outro `CODEX_HOME` e herda seletivamente a configuração,
  recursos e plugins da padrão. O fluxo legado de login único — "Login do ChatGPT (Codex) é UM login pra três CLIs",
  hoje em `superado.md` — está superado para cadastro Codex: o novo login por conta usa o protocolo nativo do Codex e não
  propaga novas credenciais para Pi/OMP. OAuth/API key são apenas métodos (`auth_method`), nunca a
  chave da conta: a identidade é `credential_id=codex:<home canônica>` e a origem do rollout.
  Autenticação, histórico, sessões, caches e confiança ficam separados; hooks herdados podem ficar
  pendentes e nunca são aprovados automaticamente. Catálogo embutido é materializado dentro da
  secundária: no CLI 0.153.4, symlink do cache ou do catálogo da padrão resulta em zero plugins, e o
  catálogo reservado só instala quando está sob o `CODEX_HOME` atual. Plugins remotos instalados por
  padrão são reconhecidos pela identidade remota, mesmo quando o inventário chama o catálogo de
  `openai-curated` e o plugin usa `openai-curated-remote`. Sessão, Arquivo e retomada preservam a
  conta; não há exclusão, rotação, migração ou troca automática por cota.
  Prova real em Linux (10/09/2026): conta padrão Pro preservada e secundária Plus conectada pelo
  login nativo, com os e-mails omitidos desta documentação pública; 20/20 plugins mantiveram
  a mesma versão/estado,
  hooks aprovados somente após pedido explícito do usuário e turno mínimo em `gpt-5.6-luna`
  respondendo `OK`. O rollout nasceu em `~/.codex-google`, apareceu no Arquivo após fechar e foi
  retomado pela mesma conta. AVD e Windows continuam sem verificação.

## Painel de saúde dos harnesses

(`app/harness_saude.py` + `harness_api.py` +
  `components/settings/HarnessSettings.svelte`, aba "Harnesses" em Configurações → Servidor): uma
  linha por CLI com o que o app instalou nele (hooks do Claude, contas, login do ChatGPT, extensões
  do Pi/omp, ponte de skills, statusline do Kimi) e um botão por item que **reusa o instalador que já
  existe** (`hook_installer.ensure_*`, `skill_bridge.rebuild`, `contas.reconciliar`,
  `oauth_codex.propagar`, o symlink das `scripts/pi/*.ts`). A checagem é só leitura; o texto vai como
  `codigo`+`params` e o front traduz (`harness_<codigo>`). O item **Credenciais** cruza o store de
  cada CLI (auth.json+models.json do Pi, `auth_credentials` do omp, `providers` do Kimi,
  `model_providers`+login do Codex) com o que o app conhece (engines.json + cofre OAuth), no nome
  que AQUELE harness usa (`provedor_embutido_do_pi` pra Pi/omp, o nome do motor pros outros);
  o card Codex tem uma seção própria de integração nativa e não oferece a ponte antiga de skills.
  "Sincronizar" reusa o `agentes_sync` e, no omp, grava a chave no mesmo SQLite do login. O Codex
  continua guardando só o nome da variável, e o resultado diz qual exportar. `instalado` é "binário no PATH OU pasta de
  config existe" porque o backend roda como serviço com PATH curto — só o binário dava "Kimi não
  instalado" com o `~/.kimi-code` cheio.

## Runtime por conta não vira atalho

(`contas._RUNTIME_DA_CONTA`, 04/09/2026): `telemetry/`,
  `feedback/`, `image-cache/`, `.last-update-result.json` (o Claude Code regrava por config dir com
  tmp+rename, que troca o symlink por arquivo real) e `.hangar-models.json` (cache do picker, por
  config dir). Ligados, cada `--prep` achava a "deriva" de novo, gavetava e disparava o toast
  "CONTA" — a gaveta desta máquina chegou a `telemetry.3`. A reconciliação desfaz o atalho antigo
  desses nomes, senão a conta seguia gravando o runtime dela dentro do `~/.claude`.

## Statusline por sidecar, não pelo pane

(`app/statusline.py` + `scripts/omniroute-statusline.js`
  + `scripts/pi/rich-status-line.ts` + `~/.kimi-code/statusline.js`): a linha que o app mostra
  (modelo, contexto, ⚡5h/📅7d, custo)
  **não** sai do transcript — quem a calcula é o agente, e o app só via o texto **já renderizado no
  terminal**, cortado na largura da janela. Medido 2026-07-30 num pane de 99 colunas: o Pi chama
  `truncateToWidth` e a linha morre em `cache…` (somem contexto, cota e custo); o Claude quebra em
  várias linhas, mas quando a quebra cai em cima do par de contexto ele vira `💬 769k/238 770k…`.
  Nos dois casos o painel dizia "medição indisponível" **por causa do tamanho do terminal**.
  Contrato: quem RENDERIZA publica a linha inteira (sem ANSI) em
  `<config>/.hangar-status/<stem>.json` = `{"line", "ts"}` — mesma chave dos outros
  marcadores (o stem do `.jsonl`) — e `statusline.read()` a prefere ao pane, caindo nele quando não
  há sidecar (sessão sem instrumentação **nunca** pode ficar sem linha nenhuma). Três detalhes que
  já custaram bug: (1) o tmp do `tmp+rename` leva o **pid**, porque o script do Claude roda a cada
  render e duas invocações da mesma sessão se sobrepõem (nome fixo → `rename` promovendo bytes
  entrelaçados, o mesmo furo que `hangar_panel_common.py` já corrigiu); (2) `read()` exige **dict** —
  JSON válido do tipo errado (`null`, lista) não levanta `ValueError` e o `.get()` derrubava a
  resolução de estado de TODAS as sessões em `list_with_state`; (3) o publicador do Pi vive na
  extensão porque a linha completa só existe dentro do processo dele — logo, **sessão Pi já aberta
  só passa a publicar depois de `/reload`** (o Pi carrega extensão na largada), enquanto o lado
  Claude vale na hora, por ser script executado a cada render. O publicador do **Kimi Code**
  (`~/.kimi-code/statusline.js`, fora do repo porque o `tui.toml` aponta pra lá) segue o lado
  Claude: script a cada render, sidecar em `~/.claude/.hangar-status/<sessionId>.json` —
  a chave é o `sessionId` do stdin, o mesmo que `session_key()` extrai do `wire.jsonl`. A linha
  dele replica os marcadores do Claude (`🤖 K3 (high✦)`, `📁 dir [branch*]`, `⚡5h`, `📅7d`,
  `🕐 HH:MM ⏱`) com duas diferenças de formato: o contexto vem como par **rotulado e sozinho**
  (`💬 ctx 480k/1M` — o stdin do Kimi não traz in/out do turno, então a regra dos "≥2 pares" do
  parser/`sse._status_sig` tem exceção pro rótulo `ctx`, a mesma do Pi) e **não há 💵** (Kimi é
  assinatura de valor fixo, mesmo motivo do Claude em motor). O ⏱ dele é a idade do
  `wire.jsonl` (birthtime), não duração de API como no Claude.

## O `wire.jsonl` do Kimi não é um transcript bem-comportado

— duas armadilhas medidas em
  14/08/2026, as duas em produção, na mesma sessão:
  - **Nem toda escrita é turno.** O hook grava `idle` no `Stop` e `state.corrige_ocioso_kimi`
    promovia pra `working` sempre que o arquivo fosse mais novo que o marcador (é o que cobre o
    prompt ENFILEIRADO na TUI, que não dispara hook nenhum). Só que o Kimi grava `config.update` —
    o system prompt inteiro, ~90KB — com a sessão parada: turno fechou 08:28, o `config.update` caiu
    08:40 e a sessão ficou "em execução" com o pane no prompt. Agora o mtime é só o **portão barato**
    (um `stat` por poll) e quem decide é `_kimi_turno_aberto`, que lê o **fim** do arquivo até a
    primeira fronteira de turno: `turn.ended`/`turn.cancel` = parada, `turn.prompt`/`turn.steer` =
    andando (levantado sobre todos os wires da máquina: não há outro `turn.*`). O regex é só filtro
    barato — quem decide é o `type` de TOPO da linha, via json, senão uma msg CITANDO
    `"type":"turn.ended"` vira fronteira.
  - **O main fica MUDO quando delega.** Subagente (tool `Agent`/`AgentSwarm`) roda no mesmo
    processo mas escreve no wire DELE (`<sessão>/agents/agent-N/wire.jsonl`); o
    `agents/main/wire.jsonl` não recebe uma linha enquanto isso. E quando um subagente termina, o
    hook `Stop` dispara com o `session_id` da SESSÃO — marcando `idle` no meio do turno do main.
    Foi essa dupla que fez a mesma sessão aparecer "pronta" com o terminal mostrando
    `Running 2 agents`, três vezes. Por isso o mtime não decide nada: quem decide é a fronteira de
    turno do main, e prova de vida (no caminho degradado) é o mtime mais novo entre TODOS os
    `agents/*/wire.jsonl`. Quem for mexer em estado do Kimi: **o wire do main não é a sessão**.
  - **`tool.result` não tem `uuid`** (só `parentUuid` e `toolCallId`), e o parser mandava `id=""`.
    O front deduplica evento **por id** (`Chat.svelte`, `idIndex`), então os 205 resultados de uma
    sessão real disputavam o MESMO slot: cada um apagava o anterior. Dois estragos ao mesmo tempo —
    todo card de ferramenta preso em "Executando…", e o card do **AskUserQuestion reabrindo depois
    de respondido** (o front deriva "respondida" da presença do `tool_result`; quando a ferramenta
    seguinte tomava o slot, a pergunta voltava a parecer pendente). Id agora é `res:<toolCallId>`.
    O teste antigo não pegou porque fabricava um `uuid` que o Kimi nunca manda: **ao escrever teste
    de parser, copie o shape do wire real**, não o que a doc sugere.

## Furar a fila do Kimi (steer)

(`terminal_input.steer_now` + `POST /api/sessions/{name}/steer` +
  o chip `⏳ N na fila · mandar agora` no `Composer`): msg enviada com a sessão trabalhando fica na
  fila da TUI do Kimi ("↑ to edit · ctrl-s to steer immediately"); o `ctrl-s` a injeta no turno em
  curso — vira `turn.steer` no wire, no MESMO turnId, com o `context.append_message` de user de
  sempre (por isso o dedup da fila durável não muda nada). Medido: o ctrl-s promove a fila
  **inteira** de uma vez (duas msgs entraram como um bloco só), e com a sessão parada é no-op. É
  tecla avulsa, não parâmetro do envio: a decisão "essa não espera" vem DEPOIS de já ter mandado. O
  número do chip conta as bolhas translúcidas — eco local (`pending`) **mais** os eventos
  `queued-` da fila durável; só o eco local dava 0 (ele some em ~1s, quando o `queued-` chega) e o
  chip nunca nascia. 409 fora do Kimi.

## Prévia ao vivo: sidecar do agente primeiro, pane depois

(`preview.read_sidecar` +
  `scripts/pi/hangar-state.ts`): mesmo contrato da statusline, agora pro texto **em voo**. A extensão do
  Pi recebe o bloco do assistente token a token (`message_update`) e publica o **último bloco de
  texto** em `<config>/.hangar-preview/<stem>.json` = `{"text", "ts"}`; `PreviewBroker._loop`
  o prefere e só cai no `capture-pane` quando não há sidecar. É o que tira a prévia do Pi da
  adivinhação: todo o `extract_assistant_text` (verbo de ferramenta, caixa do composer, spinner,
  painel de Todos) existe só pra separar prosa de desenho de TUI, e um quadro do spinner em `*`
  ASCII — fora de `SPINNER_GLYPHS` — já fez a prévia engolir a linha de status **e o painel de
  tarefas inteiro** (03/08/2026). Quatro coisas que o desenho decide de propósito: (1) `""` é
  **resposta** ("não há nada em voo"), `None` é ausência (cai no pane) — tratar os dois igual traria
  de volta o bloco já commitado como bolha duplicada; (2) publica o **último** bloco, não a soma —
  mandando a soma, `sse.preview_is_committed` vê o commitado como prefixo da prévia e engole tudo;
  (3) a extensão coalesce em 150ms e `unref()` o timer, porque `message_update` dispara por token e
  um timer pendente não pode segurar o processo do Pi vivo; (4) teto de idade de 10min, pro caso da
  extensão morrer no meio do turno — aí o pane volta a mandar em vez de congelar a última frase.
  Vale o mesmo aviso da statusline: **sessão Pi já aberta só publica depois de `/reload`**. O
  **Claude Code também publica** desde 17/08/2026: `hooks/preview_hook.py` (instalado pelo
  `hook_installer.ensure_preview_hook_installed` no startup) escuta o evento `MessageDisplay`
  (Claude Code ≥ 2.1.152 — deltas INCREMENTAIS do texto em exibição, medido: 5 parágrafos = 6
  eventos com `index` crescente e `final` no último, markdown cru) e grava o mesmo sidecar; o
  `Stop` zera. O acúmulo entre eventos vive no próprio sidecar (`message_id` gravado junto), e
  texto com `agent_id` (subagente) nunca é publicado. Sessão Claude já aberta não relê hooks →
  segue no pane até reiniciar; a raspagem inteira do `extract_assistant_text` vira plano B, não
  código morto. Codex nunca raspou pane (app-server).

## O diálogo de confiança do Claude Code, e as três coisas que ele derrubava

(medido 06/09/2026,
  claude 2.1.263, com o pane real capturado em `tests/fixtures/pane_trust_dialog.txt`). Sintoma no
  Windows: sessão criada pelo app numa pasta nova morria sozinha e o app dizia "sessão não
  encontrada"; o chat de outra ficava em "reconectando" para sempre. São três defeitos em fila, e o
  segundo e o terceiro valem em qualquer sistema:
  - **A chave do pre-trust é o caminho com barra NORMAL no Windows.** No bundle do CLI,
    `function uN(e){let t=B(e); if(L()==="windows") return t.replaceAll("\\","/"); return t}` é quem
    monta a chave de `projects` no `.claude.json`. O `_pretrust_cwd` gravava o `cwd` cru — que vem do
    `fs.py` como `str(Path(...))`, com contrabarra —, então escrevia uma chave que ninguém lê e o
    diálogo aparecia mesmo com o pre-trust rodando. Hoje passa por `registry._chave_trust`. (A outra
    metade dessa armadilha, "escreveu no ARQUIVO errado", já estava fechada em `tmux.claude_json_de`.)
  - **`is_overlay` não via o diálogo porque olhava as 8 últimas linhas de um pane cheio de branco.**
    A caixa ocupa 16 linhas de um pane de 30 e o resto fica vazio; `capture-pane` devolve a altura
    inteira, então a janela de 8 linhas pegava só branco e o gate respondia "tela livre". Com isso o
    `deliverable` liberava, o envio digitava às cegas e o Enter caía em **"No, exit"** — que é a
    opção sob o cursor, porque o CLI desenha esse diálogo com `cancelFirst:!0, focus:"cancel"`. E as
    opções vêm com `hideIndexes:!0`, sem `1.`/`2.`, então `classify` nunca as vê como menu: o
    `is_overlay` é a única defesa. Quem responde "quais são as últimas 8 linhas" agora é o
    `state._rodape`, que descarta as em branco do fim — a mesma correção que o `_pane_tail` do
    `terminal_input` já tinha, e que o `_menu_block` (o gate do picker do Pi) também precisava.
  - **Quarta porta do mesmo defeito: `_composer_regiao`** (medido 08/09/2026, numa máquina Windows,
    parear `pss` com `pmw`). Numa sessão **recém-aberta** o Claude Code desenha o composer no ALTO
    da tela e o resto do pane vem em branco; a distância da régua de baixo até o fim estourava
    `_COMPOSER_FUNDO = 8` e a região era dada como ilegível. Consequência: `_deliver` não conseguia
    provar a entrega do prompt do grupo em NENHUM membro, `pair_session` reverteu o grupo e devolveu
    502, e a tela dizia só "Falhou o pareamento com pss." O log traz a geometria exata —
    `reguas=5,7 fundo=16` e `reguas=11,13 fundo=10`, panes de 23 linhas —, reproduzida com um pane
    fabricado nesses números. Aqui, com a janela cheia, o fundo é 4–7: por isso nunca apareceu no
    Linux. Hoje `_linhas_uteis` apara as brancas do fim antes de medir, e o **diagnóstico usa a mesma
    poda** — medindo o pane cru ele reportaria um fundo que a decisão real não usa.
  - **O `catch` do `PairSheet` jogava fora o motivo.** Todos os erros do `/pair` vêm em envelope
    traduzível (`erro_sessao_nao_encontrada_detalhe`, `erro_pareamento_desfeito`,
    `erro_pareamento_tarefa_existente`) e `lerErro` já os resolve antes de virar `Error` — o
    `catch` sem variável descartava isso e deixava a tela sem nada para consertar. Mesmo defeito no
    `doLeave`, corrigido junto.
  - **`Baixar-Dist` só falava no sucesso.** Cada `return $false` (sem tar/curl, sem git, `frontend/`
    sujo, sha do CI de outro commit, tar quebrado) era mudo, e o `npm ci` de um minuto e meio
    começava sem explicação — quem trocou para baixar o dist do CI não tinha como saber se o
    download nem foi tentado. Agora cada desistência imprime o motivo, nos dois instaladores; no
    `install.ps1` isso só é seguro porque `Nota` é `Write-Host` e não entra no valor de retorno.
  - **`awatch` numa pasta que ainda não existe derruba o SSE em laço.** `projects/<slug>` só nasce
    quando o agente escreve; até lá o `follow()` levantava `FileNotFoundError`, o `pump` mandava o
    erro pro cliente, o EventSource reconectava e caía no mesmo erro. O `TranscriptTailer.follow`
    espera a pasta em vez de estourar (o `mkdir` que o adapter do Codex já fazia era o mesmo
    problema, resolvido só naquele caminho).

## Preferência da barra do Claude Code (07/09/2026)

o card de Harnesses abre **Opções**
(`HarnessOpcoes.svelte`), com rascunho e Salvar no servidor selecionado. `claude_statusline_update`
vem ligado; desligado, o instalador preserva `statusLine`. Linux e Windows chamam a mesma rotina
stdlib Node (`scripts/configure-statusline.cjs`), que lê `runtime-config.json` sem backend,
respeita `CLAUDE_CONFIG_DIR`, faz backup e grava sem BOM. Preferência inválida não vira autorização
para sobrescrever a barra. Salvar só muda a preferência, não o comando atual.

## Marketplace nativo com outro nome (07/09/2026)

o Claude Mem declara `thedotmack` no manifesto
Claude e `claude-mem-local` no do Codex. Nome do plugin + origem confirmada identificam o alias;
`registro.plugins` continua indexado pela fonte Claude, e `id_codex` acompanha o destino real nas
operações e na checagem de skills habilitadas. Não associar só pelo nome e não esquecer o destino
ao desabilitar: isso deixaria o plugin antigo executando. Mais de um alias possível é erro.

## Hook do `security-guidance` no JSON estrito do Codex (07/09/2026, PR #3)

o plugin 2.0.7
provocava dois erros medidos no Codex 0.153.4: `SessionStart` emite anúncio `async` + resposta
com `metrics`, e `Stop` também emite `metrics`. São extensões do Claude, recusadas pelo JSON
estrito do Codex. Só esse plugin recebe `codex-hook-json.py` (instalado em
`<codex>/.hangar-hooks/`); bloqueios, contexto e exit code são preservados, sem autoaprovar os
comandos novos. O PR também COPIAVA `~/.claude/hooks` inteiro pela área de importação e publicava
cópias com manifesto em `~/.codex/hooks` — mesmo bug que o `13ed4251` do mesmo dia já fechava com
symlink (`codex_hooks_arquivos`). Ficou o symlink, decisão do usuário: uma fonte só, sem cópia
pra envelhecer entre reconciliações. A parte de cópia foi retirada na integração do PR.

## Hooks em subpastas (07/09/2026)

`codex_hooks_arquivos` preserva o caminho relativo inteiro,
também ao atualizar cópias no Windows. A verificação anterior exigia o pai imediato `hooks/` e
ignorava `gitnexus/gitnexus-hook.cjs`: os comandos importados existiam, mas o arquivo não.
Captura de `hook/completed` no app-server confirmou falha nos dois hooks do GitNexus; execução
direta mostrou `MODULE_NOT_FOUND`, código 1. Homônimos na raiz não substituem arquivos de
subpastas; caminhos resolvidos fora de `hooks/` continuam excluídos. A restauração do arquivo
mantém o comando aprovado no Codex.

## Clone reduzido de marketplace no Codex (07/09/2026)

o clone completo do Claude Mem levou
44,09 s e trouxe 461 MiB nesta máquina; o atualizador nativo encerra o clone após 30 s. Clone
raso manual levou 12,44 s, mas o CLI não oferece `--depth`. A opção nativa `sparse_paths =
[".agents", "plugin"]`, no marketplace `claude-mem-local`, usa `--filter=blob:none` e checkout
das pastas necessárias: cadastro em 2,42 s, atualização em 3,21 s, mesma origem GitHub.
Essas pastas são específicas desse catálogo; não são padrão para marketplaces alheios.
O importador nativo considera opções de clone diferentes como outra origem, mesmo com a URL
igual, e recusava reimportar o plugin já instalado. O reconciliador agora dispensa a importação
quando nome e origem comprovam a instalação nativa, inclusive com alias; atualização, reparo,
habilitação e desabilitação continuam pela mesma esteira. Plugin ausente continua sendo importado.

## Avisos da reconciliação Codex (10/09/2026)

uma entrada MCP que já contém todos os campos
convertidos da fonte pode ser adotada mesmo com complementos locais (`tools`, timeout, variáveis
extras). O manifesto guarda só os campos da fonte; divergência em qualquer um deles continua
preservada com aviso. Um link pessoal que resolve exatamente para a skill descoberta, como o alias
antigo `~/hangar`, fica intacto e sem aviso, mas não é adotado como gerenciado: equivalência não
transfere propriedade. Falhas do CLI com código não zero, JSON inválido ou `errors` no JSON deixam
um diagnóstico privado em `<codex>/.hangar-diagnosticos/cli-<hash do comando>.log` (0600 no POSIX,
última falha por comando, caudas limitadas a 8 KiB por saída). O log do serviço mostra somente o
código e o caminho; stdout/stderr brutos podem conter tokens e não vão para ele. Em 10/09, o erro
de atualização do Claude Mem não reproduziu na nova tentativa: catálogo local e HEAD remoto
coincidiram e a reconciliação terminou `ok`, sem erros nem marketplaces pendentes. Isso comprova a
recuperação, não a causa da falha anterior, cujo detalhe não foi preservado pelo backend antigo.

## O que segura a abertura de uma sessão Codex é a TUI parada num widget, não a sincronização


(medido 10/09/2026, codex-cli 0.153.4 → 0.154.0). Com o CLI desatualizado a TUI abre com
"✨ Update available … Press enter to continue" ANTES do `thread/start`; sem thread não há
sidecar, e o app fica em "A conversa do Codex ainda não começou" para sempre (150 s medidos, sem
sidecar). O cartão de seletor pré-thread já existia (`menu_codex` + lista), mas só reconhecia o
rodapé "press enter to confirm" dos seletores de hooks/permissões — o do aviso de update é
"continue", e no pane estreito (terminal do celular anexado) a opção 1 quebra em 3 linhas, que
fechavam o bloco com 1 opção. Hoje os dois rodapés valem e continuação indentada sem número cola
na opção anterior. Escolher "Update now" pelo app funciona, mas o Codex SAI depois de atualizar e
o pane morre: a sessão some da lista e precisa ser criada de novo. A reconciliação custa ~20 s
quando roda (fingerprint mudou, 6 h do marketplace, ou 5 min após falha); em 10/09 ela rodou em
toda abertura porque o "auto-upgrade was in flight" do marketplace `ecc` contava como falha
(`fcdd382b`). Sem o aviso de update, a thread abriu em 21 s nesta máquina. Pendente, visto uma vez no
0.154.0: o seletor de hooks passou a desenhar SEM número (`›    Review hooks`), e `menu_codex`
exige `N.` — captura real do widget antes de mexer.

## Instruções nativas (07/09/2026, PR #3)

`codex_instrucoes.py` prepara `AGENTS.override.md`
— nome que o Codex 0.153.4 lê no lugar do `AGENTS.md` da mesma pasta — como link para o
`CLAUDE.md` global (`<codex>/AGENTS.override.md`) e dos projetos registrados no `config.toml`; o
lançador prepara também os escopos raiz→cwd antes de subir o app-server. `CLAUDE.MD` é a segunda
opção. Override pessoal não é sobrescrito. Sem permissão de symlink, usa cópia gerenciada que é
atualizada na próxima preparação. Isso INVERTE a decisão de 06/09 (bloco "leia o CLAUDE.md" no
`AGENTS.md`, custo de as regras não estarem no primeiro token): o bloco antigo sai com backup e o
`CLAUDE.md` inteiro entra no primeiro request — por isso `project_doc_max_bytes` sobe pra pelo menos
1 MiB, crescendo com as fontes conhecidas e preservando limite maior já configurado. Dois custos
aceitos pelo usuário: ~110 KB de contexto por sessão Codex, e um arquivo untracked na raiz de cada
repo com `CLAUDE.md` — o mesmo problema dos anexos de 31/08, mitigado aqui gravando
`AGENTS.override.md` no `.git/info/exclude` do repo (`_excluir_do_git`), que é local e não
versiona. Onde já existe `AGENTS.md` de verdade, ele deixa de ser lido pelo Codex (o override
substitui, não soma). Teste com CLI real captura a primeira requisição em servidor local, sem
modelo: global + projeto acima de 180 KB presentes, AGENTS preteridos ausentes. Projeto novo
aberto pelo IDE/CLI cru precisa ser registrado e reconciliado antes de ganhar prioridade sobre um
AGENTS existente. Sessões já abertas conservam o contexto inicial. Falha na preparação (override
pessoal, `config.toml` ilegível) não impede a TUI de abrir: sai aviso no stderr do pane.

## Perguntas assíncronas do Codex (11/09/2026, CLI 0.154.0)

`request_user_input_async`
chega como `agentMessage` com `delivery: "async"` e `questions`, não como pedido JSON-RPC
`item/tool/requestUserInput`. `async_questions.py` acompanha cada pergunta por thread/item/índice;
o `thread/resume` repõe o histórico antes de reaplicar eventos recebidos durante a leitura.
O backend acompanha também sessões recém-abertas pelo terminal. `pending_questions` alimenta os
avisos das listas e abas sem trocar `working` por espera; o formulário existente recebe a pergunta.
A resposta usa o mesmo `turn/start` da TUI, dirigido à thread original, com `> título\n\nresposta`.
Cada pergunta é independente, inclusive quando uma chamada contém várias. O eco da resposta local
não responde de novo outra pergunta com título igual. Provas com app-server/TUI reais e provedor
local falso confirmaram envio durante o turno, resposta após seu fim e recuperação após reconexão.
Limites nativos medidos: “Pular” só altera a memória da TUI, sem evento/histórico; uma resposta por
outro cliente não fecha o widget já aberto no terminal. O Hangar reconhece respostas do terminal
pelo histórico, mas não inventa confirmação de descarte.

## Aviso de espera do Codex (11/09/2026, CLI 0.154.0)

o popup “Giving this request a little
extra thought” vem de `model/safetyBuffering/updated`, não de uma pergunta ou temporizador local.
`showBufferingUi` alimenta `codex_buffering` no estado compartilhado, exibido como aviso informativo
no web e no app nativo. O turno continua trabalhando; delta não vazio, mensagem completada, fim do
turno ou sinal `false` retiram o aviso. Eventos de outra thread/turno não o alteram. Reabrir o chat
preserva o estado no backend. Limite medido com CLI real e provedor local: um cliente novo não
recupera buffering por `thread/read`/`thread/resume`; reiniciar o backend perde o aviso já emitido.

## - `adapters/codex/` — one loopback WebSocket app-server per Codex sess

- `adapters/codex/` — one loopback WebSocket app-server per Codex session; the backend consumes
  structured JSON-RPC events while a `codex --remote` TUI for the same thread runs inside tmux.
  **Controles nativos do chat (07/09/2026, CLI 0.153.4):** `thread/read` e `thread/resume`
  informam `reasoningEffort`, enquanto `thread/settings/updated.threadSettings` usa `effort`.
  `thread/settings/update` compartilha modelo, esforço e `collaborationMode` com a TUI; o
  `turn/start` herda esses valores, pois reenviar o sidecar sobrescreveria uma escolha do terminal.
  `skills/list` fornece nomes e caminhos de entradas `UserInput` do tipo `skill`; o texto `/nome`
  permanece no histórico para reconciliar os ecos da fila. `turn/steer` exige `expectedTurnId`:
  uma orientação para um turno encerrado falha, preservando a mensagem. Contexto estendido usa
  `model_context_window=1000000`, com restauração do valor anterior e sem editar o catálogo;
  o Codex aplica `max_context_window` de cada modelo (Astra/Sol: 872000 nessa instalação).
  **Reconexão do modo (09/09/2026, revisão do PR #4):** `thread/read` não devolve
  `collaborationMode`, e uma conexão nova não recebe o retrato de `thread/settings/updated`.
  Prova com dois clientes do CLI real, sem inferência: o primeiro escolheu Planejar e o segundo
  não recebeu essa escolha. O último `turn_context` do rollout é histórico, não estado atual.
  Até uma notificação ou troca confirmada, o backend devolve `null` e o seletor mostra modo
  desconhecido. A descoberta do plano Claude acontece nas transições de estado, não a cada
  mensagem/ferramenta: cada chamada percorre o transcript inteiro.
  **O app-server é do PANE, não do backend** (`scripts/hangar-codex-tui`, o lançador único que o
  backend e o terminal chamam igual): ele escolhe a porta, sobe o servidor em segundo plano, roda a
  TUI em primeiro plano — nunca `exec`, que é o que o deixaria sem quem matar o servidor na saída —
  e grava `endpoint`+`app_pid` no sidecar junto de thread/rollout/cwd. O backend só se **liga**
  nele (`AppServerClient.connect`), conferindo o pid antes: porta de loopback é reciclada, e
  conectar só pelo endereço pode cair num processo alheio. Pid morto é sessão morta, não sessão a
  reconectar. Por isso criar sessão Codex passou a ser o caminho normal de criação (`registry.create`
  com `provider="codex"`, transcript vazio como Pi/Kimi) — não há mais `create_codex`. Duas armadilhas
  que sobram: o `codex` está no `_EXEC_PROVIDER` porque entre o pane nascer e o sidecar existir o pane
  cairia no default `claude` e seria casado com o transcript do Claude do mesmo diretório; e nessa
  janela `info.jsonl` é `None`, então tudo que deriva chave do transcript (`session_key`) tem que
  desviar — `session_key(None)` levanta `TypeError` e derrubaria a lista inteira, de todas as sessões.
  E uma terceira, medida em 04/09/2026: **a TUI só sobe depois de a porta do app-server aceitar
  conexão** (`_esperar_porta`). O `codex --remote` conecta UMA vez e, recusado, sai com 1 — o pane
  morre, o tmux imprime `[exited]` e o `hangar-codex` apaga a sessão em menos de 1s. Só aparece com a
  máquina carregada (load ~5 com suítes rodando): aí o servidor perde a corrida pro bind e a TUI
  chega antes. Com a máquina folgada nunca reproduzia, em nenhum terminal.
  **A fila de notifications do app-server tem UM consumidor por sessão** (`_bombear`, 04/09/2026):
  cada SSE é um ouvinte que recebe cópia dos `StateEvent`s, e o primeiro evento é o retrato do
  que a sessão já sabe (estado + status line). Antes, cada SSE lia a fila direto e o comentário
  dizia que "ainda convergem" — o contrário: `queue.get()` entrega cada delta a UM consumidor, e
  desktop + celular no mesmo chat mostravam metade da frase cada ("Faria em pequenas, o atual."
  no lugar de "Faria em mudanças pequenas, preservando o comportamento atual.", reproduzido com
  o `AppServerClient` real). E sem o retrato inicial, reabrir o chat no meio do turno deixava a
  tela sem estado nem contexto até a próxima notification — o que parecia "o Codex perdeu o
  contexto" com o rollout íntegro. A bomba morre com o último ouvinte (drain-on-complete
  continua acoplado a haver um SSE aberto, como antes).
  **O `rtk` no `hooks.json` do Codex passa por `scripts/codex-hook-allow.py`**: o rtk 0.43.0
  devolve `updatedInput` sem `permissionDecision` quando reescreve só um pedaço de um comando
  com `;`; Claude Code aceita, o Codex recusa a reescrita ("PreToolUse hook returned
  updatedInput without permissionDecision:allow") e roda o original. Só o rtk é embrulhado —
  um pipe em volta de outro hook esconderia o rc=2 com que ele bloqueia.
  **O provedor `command-code` que o `agentes_sync` grava no `config.toml` do Codex não serve ao
  Codex** (medido 04/09/2026): o gateway só tem `/chat/completions` — `/responses` é 404 em
  `/provider`, `/provider/v1`, `/v1` e na raiz — e o codex-cli 0.153.1 recusa `wire_api = "chat"`
  ao carregar a config. O bloco fica lá como promessa vazia; testar Codex noutro modelo hoje só
  com provedor que fale Responses API (a OpenCode fala, mas a conta estava sem saldo).

## Voz Codex no web

(`codex_voice.py`, `CodexVoice.svelte`, `lib/codexVoice.ts`, 10/09/2026):
  é beta e opt-in por servidor: `codex_voice_beta` nasce `false` no runtime config e aparece em
  Harnesses → Codex → Opções. Desligada, o botão não monta e o backend recusa WebSocket e catálogo
  de vozes; um front antigo não contorna a trava. O botão e a opção exibem o badge Beta. Salvar
  atualiza o chat do mesmo servidor sem exigir reload; desligar durante uma chamada desmonta o
  componente e encerra áudio/WebRTC pelo teardown existente.
  o botão Voz usa a conta e o app-server da sessão aberta. A conversa roda numa thread efêmera
  organizadora; a thread de trabalho só recebe o pedido consolidado depois da confirmação.
  WebRTC com `version: "v3"` negociou com login ChatGPT Pro no CLI 0.154.0; o transporte WebSocket
  do Codex exigiu API key e o padrão WebRTC foi recusado por versão do protocolo. O WebSocket do
  **Hangar** só leva sinalização e mantém a posse da chamada (uma por sessão, heartbeat com prazo).
  `_consumir` continua sendo o único leitor das notifications; a chamada mantém um ouvinte de estado
  enquanto o SSE reconecta. O botão fica no compositor: o modal configura a chamada e fecha quando
  conecta, deixando o indicador "Em voz". Reabrir/fechar os controles não desliga a voz. Encerrar
  ou trocar de sessão libera áudio e envia realtime/stop,
  sem fechar o cliente compartilhado nem cancelar o turno do agente. Ditado fica bloqueado durante
  a chamada. A escolha fica em `cp_codex_voice`, por navegador/origem. O medidor usa RMS separado
  do microfone e do áudio recebido; anima apenas o span HTML da marca, nunca o SVG.
  Verificado com faixa silenciosa no navegador:
  ICE/DTLS conectados, silenciar desabilita a track, desmontagem encerra a track e o peer. A fala
  real foi exercitada na POC pelo usuário; interrupção nesta integração ainda requer teste falado.
  O app Expo ainda não tem esse controle.
  Encaminhamentos `<realtime_delegation>` são reconhecidos só na apresentação (`parseRealtimeDelegation`
  no core): cabeçalho "Conversa de voz", pedido no corpo e envelope integral nos detalhes. O texto
  original e seu ID continuam intactos; não usar `parsePeerMessage`, que também decide tráfego de pares.
  Só trocar o prompt não resolveu o envio de fragmentos. No CLI 0.154.0, `HandoffRequested`
  encaminha a transcrição da última fala antes de avisar o cliente. O organizador em
  `codex_voice_broker.py` usa ferramentas dinâmicas para preparar/cancelar/confirmar um rascunho.
  A confirmação deve vir de outra interação, pelo `userMessage` real do app-server, e corresponder
  a uma autorização curta; o texto enviado é o rascunho imutável, não a confirmação. Revisão muda
  o ID; repetição não reenvia. A fila durável existente recebe o pedido, inclusive se o alvo estiver
  ocupado. Rascunhos ainda não enviados valem apenas durante a chamada. Shell, apps, hooks e MCPs
  ficam desabilitados no organizador; o catálogo foi conferido no request do CLI real com provedor
  local, sem modelo externo. O modelo é o da sessão, com esforço baixo, e há custo de organização
  na mesma conta. Contexto inicial usa a cauda já lida pelo Hangar, sem pedir o histórico inteiro
  pelo app-server. O organizador resume o resultado final e usa `appendSpeech`: `appendText` sozinho
  adicionava contexto, mas não produziu fala no teste. Teste com dois turnos de organização e
  entrada de áudio silenciosa confirmou pedido completo, resposta da sessão e retorno transcrito
  "A sessão respondeu: pinguim azul". Pausas e confirmações por áudio ainda exigem teste falado.
- `adapters/kimi/` + `hooks/kimi_state_hook.py` + `kimi_hook_installer.py` — Kimi Code runs in the
  same tmux-native shape as Pi: TUI in the pane, chat from
  `~/.kimi-code/sessions/<wd>/<session_id>/agents/main/wire.jsonl`, state pushed by hooks in
  `~/.kimi-code/config.toml` (no pane scraping for state). The pane↔session link is the hook's
  ticket (`~/.claude/.hangar-kimi/<pane>.json`) — the CLI has no caller-chosen session-id.
- `sse.py` — merges the above into the SSE stream. `api.py` — FastAPI routes. `auth.py` — bearer token / `cp_token` cookie.
- Also: `pqueue.py` (durable input queue), `preview.py` (live in-flight block), `askquestion.py`
  (native AskUserQuestion stepper), `uploads.py`, `git_ops.py`, `commands.py`, `workflows.py`,
  `model_picker.py`, `config.py`, `fs.py`, `hook_installer.py`.

Frontend (`frontend/src/`): `screens/` (Chat, Board, …), `components/` (MessageList, NavBar, Composer,
bubbles, sheets, Spinner/Lottie, …), `lib/` (`api.ts` SSE client, `activity.ts`, `markdown.ts`,
`format.ts`, `types.ts`), `app.css` (design tokens + shared keyframes).
