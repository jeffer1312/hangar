# Instalação, atualização e serviços

Decisões medidas, com data e número. As regras vigentes ficam na seção abaixo (o `CLAUDE.md`
só aponta para cá); a medição que sustenta cada uma mora na entrada de mesmo assunto.

## Regras vigentes

- **Quem serve a interface é o BACKEND; o `frontend/dist` chega pronto do CI.** Gate que pergunta
  "o front mudou?" tem que conhecer TODAS as árvores de onde o front é compilado (`frontend` **e**
  `packages`) — senão serve tela velha ou apaga edição local.
- **Instalador com portão de prova por etapa**: essencial que falha para na hora; extra opcional
  que falha entra na lista e o fim nunca diz "Pronto". Nada é dado como feito sem prova.
- **O backend escuta em `0.0.0.0` por padrão**, gravado pelo instalador e pelo `--update`/
  `-Update` quando o `.env` está sem a chave ou em loopback; IP escolhido à mão fica. É o que
  deixa os clientes usarem a rede local antes do Tailscale (`baseOf`, em
  [plataforma.md](plataforma.md#rede-local-antes-do-tailscale-baseurl-é-identidade-a-rota-é-baseof)).
  Nunca `auto`: tira o loopback de que o `tailscale serve` depende. Firewall do Windows no
  `-Update` só sem UAC (já admin); senão vira pendência com o comando.
- **Instalador guiado: duas perguntas, o resto é padrão.** Sem terminal, tudo é NÃO — exceto no
  `--app`/`-App` (assistente do app nativo), que responde pelas telas: `ask` e `ask_senha` sim,
  `ask_extra` não, Tailscale pela opção, nada espera teclado. O log nunca carrega o token, e no
  `--app` nem a saída: a senha chega por `HANGAR_TOKEN`. O portão do 1/8 do `install.ps1` só barra
  pendência sem código; as com código são extras e vão ao portão do fim. A senha do celular recusa
  ASCII não visível (acento), `#`, `$`, aspas, `\` e espaço nas pontas (o `.env` é lido pelo python-dotenv; o app nativo monta o cabeçalho só com ASCII visível). Ver
  [a entrada](#modo---app-o-assistente-responde-pelas-telas).
- **O instalador exige UM agente de código, não o Claude Code.** Usa os que já existem; nenhum →
  instala o Claude Code (o padrão). `--agentes=`/`-Agentes` e o `--avancado` escolhem; o comando
  dos que não são o Claude sai de `app/harness_commands.py`, a mesma tabela do painel de
  Harnesses. Ver [a entrada](#o-instalador-exige-um-agente-de-código-não-o-claude-code).
- **Atualizar pelo app faz tudo sozinho, mas nada é irreversível**: resgate antes de qualquer
  passo destrutivo, com a ref conferida. Passo só entra no registro depois da prova passar, e o
  registro é do que JÁ RODOU aqui — não do intervalo de commits.
- **O Atualizar segue a main, ou a branch de `CP_UPDATE_BRANCH`.** Só ela e a branch em que a
  própria atualização pôs o checkout (`<config>/.hangar-update/branch`) escapam da recusa de
  branch de trabalho; esvaziar o campo volta para a main com o mesmo resgate. Branch ausente no
  origin falha antes de tocar no disco, e fora da main a tela é compilada aqui (o CI só publica o
  dist da main) e o auto-update fica parado. Passo de `docs/atualizacoes/` aplicado na branch de
  teste continua no registro ao voltar pra main; e, esvaziado o campo, o auto-update não tira o
  checkout da branch de teste: a volta é pelo botão.
- **O Atualizar só avança até o commit cujo binário do Rust foi publicado para este sistema.**
  O binário é o de `platforms.<sistema>` no `server-latest.json` da release da branch (sem a
  chave, o `commit` do topo); o contrato é o `protocol` dele ou o `RUST_SERVER_PROTOCOL` do commit. O checkout vai ao commit mais novo de
  `origin/<branch>` (primeiro pai) com o mesmo número, nunca recua, e o download usa o manifesto
  lido na escolha. Atrás do topo, a tela avisa que a versão mais nova ainda não tem binário e é
  compilada aqui (também no Reiniciar); checkout já à frente com binário de outro contrato também
  avisa. Sem Rust (`CP_RUST_SERVER=0`), sem release própria da branch (404) ou sem build deste
  sistema no manifesto: topo, como antes, calado. Sem como conferir (rede, git, commit ilegível):
  topo com aviso na tela, e o auto-update espera; ele só avança com o topo inteiro publicado.
  Evidência em [Atualizar para no binário publicado](#atualizar-para-no-binário-publicado-06102026).
- **O app nativo segue o canal do servidor desta máquina.** `pre_voo.alvo` fora da main → release
  `native-<branch>`, que o `native.yml` publica a cada push na branch (mesma limpeza de nome no
  workflow e no `update.rs`); a `native-latest` continua só da main. Branch sem release, ou sem o
  build da plataforma, mantém o app e avisa na página Sobre, nunca cai na main calada. Trocar de
  canal aceita versão de contagem menor porque o CI embute a branch (`HANGAR_NATIVE_CHANNEL`); build
  local não tem canal e só troca por versão mais nova. Os `install-native.*` seguem na `native-latest`.
- **O botão Atualizar NÃO roda o instalador.** Sozinho ele faz dist do CI, `uv sync`, `npm ci` por
  hash do lock, restart e prova de vida por **pid** (HTTP o processo velho também responde).
  Wrapper/tarefa/statusline só chegam por passo em `docs/atualizacoes/` — o pre-commit e o CI
  recusam commit em `install.*`/`scripts/`/`hooks/` sem passo (`HANGAR_SEM_PASSO=1` é o escape).
  Falha do instalador vai pra tela pela marca `##HANGAR-FALHA##`, nunca pela cauda.
- **No Windows, `npm ci` na raiz só com o front parado**, e pasta que o atualizador cria dentro de
  `frontend/` entra no `.gitignore` — sobra não ignorada vira "mudança local" e desliga o dist do CI.
- **Passo com comando diferente por sistema usa `comando_posix` e `comando_windows`.** `comando`
  continua sendo o fallback comum; qualquer variante que executa algo exige `prova`.
- **Passos seguidos com o mesmo comando rodam o comando uma vez; a prova de cada um continua
  conferida**, e a que faltar faz o comando rodar para aquele passo antes de ele falhar. Só o vizinho
  imediato: comando diferente no meio faz o repetido rodar de novo. Em
  02/10/2026 a DELPHI-02 rodou o `install.ps1 -Update` quatro vezes seguidas (~20 s cada) para
  quatro passos da mesma atualização.
- **Versão é `VERSION` + número de commits** (`0.1.0.2533`): major.minor.patch à mão no
  arquivo da raiz, build calculado — nunca tag de release nem commit do CI.
- **Reiniciar o backend**: sem `--reload`; mate `-9` o pid da porta e suba destacado. No Linux é
  `systemctl --user restart`.
- **Criar sessão embrulha o tmux em escopo transiente do systemd, sob sonda** — um gerenciador que
  recusa escopo transiente derrubava toda criação de sessão.
- **`scripts/verificar-local` é exigido ao abrir PR e ao subir para a `main` ou para branch com PR
  aberto; branch sem PR sobe livre.** O hook PreToolUse do `gh pr create` (`.claude/settings.json`)
  confere o PR inteiro, desde o ponto em que a ponta saiu da base; a trava `pre-push` confere cada
  push, e consulta o PR com prazo de 10 s: sem `gh` ou sem resposta, segue com aviso. PR aberto pelo
  site fica com o CI. Ele roda o que o CI rodaria para o
  que o commit muda, num clone fixo por máquina (`~/.cache/hangar-verificacao/arvore`), em fila
  única (`flock`) e prioridade baixa, com HOME vazio e o `omp` fixo do CI; Windows por SSH na VM
  de `git config hangar.verificarWindows`, com mutex. O registro é pelo tree sha; a trava só o lê.
  Escape nos dois: `HANGAR_SEM_VERIFICACAO=1`. Na VM roda só o que o CI roda no Windows (Rust e os 6
  `test_runtime_*`), nunca o pytest inteiro. Evidência em
  [Verificação local antes do push](#verificação-local-antes-do-push-06102026).
- **No Linux a verificação prepara uma vez e roda os passos em faixas paralelas; o registro é por
  passo.** O preparo é o único que compila e instala, e o pytest roda com `pytest-xdist`, um arquivo
  por processo (`git config hangar.verificarProcessos`, padrão 4; `1` = em fila). Um passo que passou
  fica gravado para a árvore, e a rodada seguinte só repete o que falta. O Node é o do `.node-version`
  da árvore verificada. Teste do backend não pode deixar timer, thread que age ou `sys.modules`
  alterado para o arquivo seguinte: com a suíte repartida, quem paga é o vizinho. Evidência em
  [Verificação local em paralelo](#verificação-local-em-paralelo-09102026).
- **O bloco do MCP `hangar` no `config.toml` do Codex é reconhecido pela TABELA, não só pelos
  marcadores.** O app desktop reescreve o arquivo sem comentários; quem só procura `# >>> hangar`
  anexa de novo, e o TOML com chave duplicada derruba o ChatGPT e o Codex juntos.

## Restarting the backend.

No `--reload` (it holds SSE + watchfiles). `pkill -f app.main` can match your
  own shell; SIGTERM can hang on an open SSE connection. Kill `-9` the pid bound to the port and relaunch
  detached (`setsid`).

## Instalação seletiva dos workspaces

Registro trazido do `CLAUDE.md` em 11/09/2026. O comando web, no CI e nos instaladores, é
`npm ci --workspace=@hangar/core --workspace=frontend`. O EAS Build detecta o monorepo pelos
`workspaces` da raiz; retirar `mobile` fazia o envio conter só o app, deixando
`@hangar/core` (`file:../packages/core`) como link quebrado. Reproduzido com
`git archive HEAD mobile` e `npm install` em pasta isolada: a instalação saía 0, mas o bundle
falhava com `Cannot find module '@hangar/core'`.

Na medição registrada, a instalação seletiva incluiu 170 pacotes, contra 167 quando o app
estava fora dos workspaces. O lock da raiz passou de 7617 para 14348 linhas porque descreve
todos os workspaces; isso não significa instalar React Native no caminho web. Para desenvolver
o app, `npm install` dentro de `mobile/` usa seu lock próprio. O `metro.config.js` declara
`watchFolders`/`nodeModulesPaths`, e o `react-dom` dos testes acompanha a versão de `react`.

## Quem serve a interface é o BACKEND, e o `frontend/dist` chega pronto do CI.

Duas mudanças de
  25-29/08/2026 que andam juntas, e as duas têm o mesmo antônimo: uma máquina de quem usa não
  compila nem serve nada além do necessário.
  - O backend monta o `frontend/dist` na raiz (`api.py`, `_UIStatic`), então o `vite preview` num
    serviço à parte era um SEGUNDO servidor para o mesmo arquivo. Instalação nova não registra mais
    esse serviço — no Linux o `services-setup.sh` já decidia assim; o `install.ps1` passou a seguir.
    Quem **já** tem o serviço fica com ele: trocar a porta muda a ORIGEM, e origem nova é
    `localStorage` vazio (`cp_servers` com os tokens, tema, layout do canvas). Ninguém perde
    configuração por causa de um `git pull`. O que decide é a existência da unit/tarefa, nunca uma
    pergunta nova.
  - `CP_FRONT_PORT` deixou de ter `5173` cravado como default (`config.porta_do_front`): vazio = a
    porta do próprio backend. Com o serviço do front fora, o QR e o painel de alcance apontavam para
    uma porta onde ninguém escuta — foi o `Rede local … não respondeu` do painel. Quem mantém o
    preview tem o `5173` **gravado** pelos instaladores, e é isso que preserva a origem dele.
    O firewall também segue essa decisão: a 5173 só é liberada quando há serviço de front.
  - O `ci.yml` publica `frontend-dist.tar.gz` + `frontend-dist.sha` na release fixa `dist-latest` a
    cada push na main, e os instaladores baixam de lá **só** quando o `.sha` bate com o `HEAD` e o
    `frontend/` não está editado. Não bateu (CI ainda compilando), sem rede, ou tar quebrado → build
    local, como sempre foi. `tar.gz` e não zip porque o Windows 10+ traz `tar.exe` — um comando só
    nos dois instaladores. O `npm ci` **continua** para quem mantém o preview, que precisa do
    `node_modules`.
  - **Os dois gates dos instaladores perguntam pelo `packages/` junto do `frontend/`, e essa
    palavra é a única coisa que os separa de servir tela velha** (09/09/2026, quando os 46
    arquivos saíram de `frontend/src/lib` para `packages/core`). A tela passou a ser buildada a
    partir de DUAS árvores, e os dois gates ainda perguntavam por uma:
    - *Precisa rebuildar?* (`install.sh`, por mtime) — `find frontend/src …` não olhava o core, e
      um `git pull` que mexesse só em `api.ts`/`format.ts` respondia "já buildado e atualizado",
      servindo o dist velho, calado. Reproduzido em sandbox: com o core alterado, a lista antiga
      diz "já buildado" e a nova diz "precisa rebuildar". Hoje o `find` leva `packages/core/src`.
    - *A pessoa está editando o front?* (`install.sh` e `install.ps1`, por `git status
      --porcelain`) — cego para o core, ele baixava o dist do CI **por cima** de uma edição local
      em `packages/` e apagava da tela o que ela estava escrevendo. Efeito oposto ao de cima, mesma
      causa. Hoje o pathspec é `-- frontend packages`; em repositório descartável, com só o core
      editado, `-- frontend` devolve vazio e `-- frontend packages` devolve a linha do arquivo.
      No `install.ps1` esse mesmo valor (`$sujo`) alimenta os dois gates, porque a `$marca` de
      rebuild é `commit HEAD` + `$sujo`.
    Duas coisas medidas que o conserto NÃO precisou tratar: `find` com um dos caminhos ausente
    (checkout pré-migração) erra em stderr só naquele argumento e segue avaliando os outros, então
    não vira falso "não precisa"; e `packages/core/src/paraglide/` é gerado e gitignored, e
    `git status --porcelain` sem `--ignored` não o lista — ele não dispara rebuild à toa.
    A regra que sobrevive à próxima mudança de layout: **gate que pergunta "o front mudou?" tem
    que conhecer todas as árvores de onde o front é compilado.**

## Session creation's systemd-scope probe.

Creating a session wraps `tmux` in
  `systemd-run --user --scope` so the tmux server doesn't inherit the backend's cgroup, but the wrap
  is now gated on a probe: a systemd user manager that refuses transient scopes was making **every**
  session creation fail (app and terminal both). Failing the probe, sessions are created without the
  scope and the backend logs a warning (commit `23da052`).

## Atualizar pelo app

(`app/atualizar.py` + `app/atualizacoes.py` + `docs/atualizacoes/`): o
  botão faz tudo sozinho — decisão do usuário em 25/08/2026 —, `reset --hard` e reinstalador
  incluídos, porque quem usa não administra nada e não deve precisar saber que passos existem.
  Quatro coisas que o desenho decide de propósito:
  - **Roda destacado do backend** (`setsid` / `DETACHED_PROCESS`), e o progresso mora em
    `<config>/.hangar-update/estado.json`. A atualização reinicia o backend: dentro do processo ela
    se mataria no meio, e a máquina ficaria com código novo no disco e processo velho no ar — o
    estado que `install.ps1:1242` já registra como o pior. O arquivo é também o que deixa a tela
    dizer "atualizando…" enquanto o servidor volta, em vez de "desconectado".
  - **Automático não é irreversível.** `resguardar()` roda antes de qualquer coisa destrutiva: o
    que estava no disco vai pra `resgate/<data-hora>` + stash, e a função **confere a ref** antes
    de devolver. Falhou o resgate, a atualização para com o disco intacto. O único `reset --hard`
    que não passa por ali é o rollback, cujo alvo é um commit da própria máquina de minutos antes.
  - **O registro é do que JÁ RODOU aqui** (`aplicados.json`), não do intervalo de commits. O
    intervalo fura em instalação nova e em quem reclonou ou resetou. Instalação do zero marca tudo
    como aplicado (os dois installers), senão a primeira atualização roda a história inteira.
  - **Passo só entra no registro depois da PROVA passar** — comando com exit 0 e efeito ausente é
    a falha que o campo `prova` existe pra pegar. Passo novo: um arquivo em `docs/atualizacoes/`
    (formato no README de lá), no mesmo commit que o exige. Os não destrutivos também rodam na
    **subida do backend** (`main.py`), pelo motivo do `migracao_sidecars`: atualizar aqui é
    `git pull` + reiniciar, e ninguém garante que o botão foi usado.
  - **Quem reinicia o serviço é diferente em cada sistema, e no Windows já é o installer.** No
    Linux é `systemctl --user restart`; no Windows o `install.ps1 -Update` — chamado na etapa
    anterior — já derruba a instância velha (`Restart-HangarTask`) e chama `Start-ScheduledTask`, e esse
    bloco NÃO é pulado no modo `-Update` (o que ele pula é firewall/Tailscale e o hook). Há ainda
    a tarefa `hangar-vigia`, que confirma falha HTTP e recupera a instância identificada. Por isso
    `_reiniciar` não faz nada no ramo Windows: marcar "falta reiniciar" ali fazia a tela pedir um
    passo que já tinha sido dado. Medido em 25/08/2026 naquela máquina: três tarefas
    (`hangar-backend`, `hangar-frontend`, `hangar-vigia`), backend como cadeia de três processos,
    e todas em `Ready` mesmo com o servidor vivo — o `.vbs` não esperava. Esse lançador foi
    substituído pelo acompanhamento descrito abaixo.
  - **O installer matava a própria atualização, e a proteção é por COMANDO, não por linhagem.** O
    `Pare-Servico` derruba a "instância anterior" casando o caminho do checkout mais `uv|python`, e
    o motor roda como `<repo>\backend\.venv\Scripts\python.exe -m app.atualizar` — casa nos dois.
    Ou seja, o instalador chamado PELA atualização matava quem o invocou, no meio dela (medido
    25/08/2026: lock e processo morreram no minuto do "instância anterior derrubada"). A proteção
    por linhagem já existia e não bastou; hoje há exclusão explícita de quem tem `app.atualizar` na
    linha de comando. No Linux quem cobre isso é o escopo transiente do systemd, que lá não existe.
    **E a linhagem tinha o furo oposto (09/09/2026):** como a cadeia do app é `backend
    (python -m app.main) → app.atualizar → powershell install.ps1 -Update`, o backend VELHO era
    ancestral do instalador e, protegido por isso, ficava de fora dos alvos — `porta 8765 continua
    ocupada (pid N) apos parar hangar-backend` em toda atualização pelo app, com código novo no
    disco e servidor velho no ar. Hoje a subida da linhagem PARA no motor (cmdline casando
    `app.atualizar`): ele entra na linhagem, os pais dele não.

## O botão Atualizar não roda o instalador; o instalador só roda por passo declarado

(`atualizar._preparar`, `scripts/check-passo-de-atualizacao.sh`, job `passos` do CI, 14/09/2026.)
O motor chamava `install.ps1 -Update` / `install.sh --update` em toda atualização — 8 etapas, 24
ramos `if ($Update)` só no `.ps1`, cada um uma chance de falhar sem ninguém ter pedido nada dele.
Foi assim que usuários Windows receberam "NAO terminou: tarefas agendadas, backend no ar" num
`git pull` que não mudava tarefa nenhuma: o commit `41c1a0ff` ("Windows runtime remains
unverified") pôs `throw` de RunLevel/RestartCount no passo 7/8 que estouravam numa tarefa
reaproveitada com a recuperação pausada pelo passo 0 — ANTES do `Restart-HangarTask`, então o
processo velho seguia na porta com o código novo no disco. E o modal mostrava as 12 últimas linhas
(o portão do fim), com o motivo acima do corte.
  - **O caminho padrão é o que o `git pull` não traz**: dist do CI, `uv sync` (sempre — idempotente
    e rápido com o lock igual; comparar `de..para` seria o intervalo de commits, que mente em
    máquina reclonada), `npm ci` só quando o hash do `package-lock.json` gravado no sidecar mudou
    e já há `node_modules`, restart, prova de vida.
  - **Wrapper, tarefa, statusline, hangar-send só chegam por passo em `docs/atualizacoes/`**, com
    o comando cirúrgico da área (tabela no README de lá). O pre-commit e o CI recusam commit que
    toque `install.*`, `scripts/` ou `hooks/` sem passo novo; `HANGAR_SEM_PASSO=1` é o escape.
  - **O `-Update` continua existindo e tendo que funcionar**: é o hook post-merge de quem atualiza
    por `git pull` na mão, e o que um passo chama quando não há script por área (Windows).
  - **Prova de vida por pid**, não só HTTP: o processo velho responde `< 500` igual. No systemd é
    o `MainPID` da unit; no Windows é `psutil.net_connections` na porta. Pid igual = rollback.
  - **O dist anterior fica em `frontend/.dist-velho` até o fim** e volta no rollback junto com o
    código — antes o rollback deixava o backend velho servindo a tela nova.
  - **No Windows quem reinicia é `Restart-HangarTasks` do `windows-tasks.ps1`** (mesmo mutex
    `Local\HangarInstall` do instalador e da vigia), chamado pelo motor. O `.ps1` é lido do disco
    depois do pull, então o conserto chega pela própria atualização quebrada.
  - **Falha do instalador chega à tela pela marca `##HANGAR-FALHA##`** (irmã da `##HANGAR-AVISO##`),
    impressa por `Falha`/`Pare` no `.ps1` e por `fail` no `.sh`; a cauda de 12 linhas é só o
    fallback sem marca.

## No Windows o `npm ci` do botão derruba o front antes, e as sobras do dist são ignoradas pelo git

(`atualizar._stop_windows_front`, `Stop-HangarFrontend` do `windows-tasks.ps1`,
`frontend/.gitignore`, 01/10/2026.) Três defeitos encadeados deixaram uma máquina Windows sem
conseguir atualizar, nem pelo botão nem pelo instalador:
  - **O `_preparar` rodava `npm ci` com a tarefa `hangar-frontend` de pé.** O `vite preview` dela
    mapeia `node_modules\@rolldown\binding-win32-x64-msvc\rolldown-binding.win32-x64-msvc.node`, e
    o `npm ci` morria em `EPERM ... unlink` (errno -4048) depois de apagar o resto — `node_modules`
    sem `.bin`, front vivo só da imagem em memória. Como a marca `package-lock.sha` só é gravada no
    sucesso, toda tentativa repetia o `npm ci` e a mesma falha (12:25, 12:59 e 13:28). O
    `install.ps1` já derrubava o front antes do `npm ci` desde 08/08/2026; o caminho do botão,
    criado depois, não. Hoje o motor para os processos da tarefa antes, o `_reiniciar` a sobe, e
    na falha do `npm ci` a tarefa é iniciada de volta.
  - **`frontend/.dist-velho` não era ignorada pelo git.** Ela só sai no sucesso; a falha acima a
    deixou no disco, e `git status --porcelain -- frontend packages` passou a devolver
    `?? frontend/.dist-velho/`. Os dois gates leram isso como "frontend editado", descartaram o
    dist do CI e foram compilar local. `.dist-velho/` e `.dist-baixado.*` estão no
    `frontend/.gitignore`.
  - **O lock da raiz só tinha o `@rollup/rollup-linux-x64-gnu`** (gerado no Linux, bug 4828 do
    npm). O `vite build` passa — o Vite usa rolldown —, mas a etapa do service worker
    (`workbox-build`, que usa o rollup) morria em `Cannot find module
    @rollup/rollup-win32-x64-msvc`. A entrada do pacote win32 foi acrescentada à mão no lock.
    Quem regenerar o lock no Linux confere se ela continua lá.
  - **O passo desta mudança para o front, e é `destrutivo: true` só para não rodar na subida.**
    O motor carrega o `atualizar.py` antes do `git pull`, então a atualização que ENTREGA este
    conserto ainda roda o `_preparar` antigo — e como ela também muda o lock, cairia no mesmo
    `EPERM`. Os passos rodam depois do pull e antes do `npm ci`: o comando Windows do passo chama
    `Stop-HangarFrontend`, e o `_reiniciar` sobe a tarefa. Na subida do backend ninguém a subiria
    de volta, por isso o passo espera o botão.

## Passo com comando por sistema (14/09/2026)

Um único `comando` não expressava a própria tabela de `docs/atualizacoes/README.md`: reinstalar
`hangar-send`, skills e o bloco global usa `./scripts/install-hangar-send.sh` no POSIX e
`install.ps1 -Update` no Windows. Rodar o primeiro pelo `cmd.exe` falha antes da prova; rodar o
segundo em Linux nem existe. O frontmatter agora aceita `comando_posix` e `comando_windows`, com
fallback em `comando`. A variante da plataforma é escolhida ao ler o passo, e qualquer uma das
três chaves exige `prova`. O teste lê o mesmo passo simulando as duas plataformas e confirma o
comando efetivo.

## Versão é `VERSION` + número de commits (`0.1.0.2533`)

(`VERSION`, `diag.versao_legivel`, `vite.config.ts`, 14/09/2026.) `pyproject` ficou em 0.1.0 e
`package.json` em 0.0.0 para sempre: número que só sobe à mão apodrece. A primeira versão desta
regra era data + hash (`2026.09.14-360978f7`), e o usuário recusou: "isso não é versão". O
formato pedido é `major.minor.patch.build`: os três primeiros vêm do arquivo `VERSION` na raiz
(mexido à mão quando ele quiser marcar algo) e o `build` é `git rev-list --count` — sobe sozinho
a cada commit na `main`, sem tag e sem commit do CI, e é comparável entre disco, processo e
`origin/main` (o `VERSION` é lido do próprio ref, `git show ref:VERSION`, porque o remoto pode
ter um bump que o checkout ainda não puxou). `GET /api/atualizacao` traz as três em
`versao_legivel` (`repo`, `backend`, `remoto`) e `atras`; a barra do desktop e o rodapé do
celular mostram `v0.1.0.2533` com o tooltip dizendo atualizado / N atrás / falta reiniciar.
Sem git, o campo é `null` — palavra fixa virava `vindisponivel` na tela.

## Instalador com portão de prova por etapa

(`install.sh` / `install.ps1`, 02/09/2026). Duas
  gravidades: **essencial falhou → para na hora** com causa e conserto (`fail` / `Pare`): deps,
  backend (`uv sync`), token (releitura do `.env`), frontend (rc + `dist/index.html` existe).
  **Extra opcional que a pessoa pediu falhou → entra na lista** (`PROBLEMAS` / `$pendencias`) e o
  fim diz "terminou com pendências"/"NAO terminou", nunca "Pronto". Motivo: instalações saíram
  "Pronto" com o `tailscale serve` quebrado (HTTPS não habilitado no tailnet) porque a falha só
  imprimia amarelo no meio da tela — o `serve` agora tem **prova** (relê o `serve status` e exige
  a raiz do `:443` na porta do backend; `Get-Proxy443`, reusada da detecção). Parar no meio por um
  extra trancaria a instalação de quem nem consegue habilitar HTTPS no tailnet, então o extra vai
  pro portão do fim, não pro stop. No `--update`/`-Update` os extras seguem moles
  (`##HANGAR-AVISO##`): falhar ali derrubaria a atualização inteira do app por causa de um extra.
  Cara nova: banner, barra `[###-----] N/8` nos títulos numerados (dentro de `say`/`Titulo`, sem
  mexer nas chamadas), spinner nos comandos longos do sh (`gira` — sem TTY ou `--update`, passa
  direto com saída ao vivo), caixa RESUMO no fim (token só com TTY, mesma regra do passo 3/8).

## A tarefa Windows acompanha o processo até ele terminar

(12/09/2026). O `.vbs` usa `Run(..., 0, True)` e devolve o código do filho com `WScript.Quit`.
Backend e frontend legado usam `MultipleInstances=IgnoreNew`, `RestartCount=3` e intervalo de
um minuto; permanecem no logon interativo, com o nível de permissão escolhido na instalação.
A vigia também aguarda seu PowerShell, impede sobreposição e limita cada execução a dois minutos.

`scripts/windows-tasks.ps1` compartilha a recuperação entre instalador e vigia. A vigia faz
duas tentativas HTTP de três segundos, separadas por dois segundos; aceita respostas abaixo de
500, inclusive 404 sem o dist, e respeita dez minutos desde o início da tarefa/processo.
Não é uma verificação funcional de todas as rotas. WMI indisponível, PID não identificado,
porta de outro processo ou processo que não encerra impedem a criação de outra instância.

`Restart-HangarTask` encerra só os processos do serviço identificados pelo checkout/lançador,
confere nascimento do PID (tolerância inferior a um milissegundo entre WMI e GetProcessTimes),
aguarda a tarefa sair de `Running` e a porta ficar livre, e só então inicia novamente. Não usa
`Stop-ScheduledTask` nem mata toda a árvore: psmux e `app.atualizar` não são alvos.

**Não** pausa o reinício automático (12/09/2026): zerar o `RestartCount` por `Set-ScheduledTask`
gera `<RestartOnFailure>` com `<Interval>` e sem `<Count>`, e o Agendador recusa o XML inteiro —
`HRESULT 0x80041319`, "Um elemento ou atributo necessário está faltando no XML da tarefa.
(43,8):Count:". Como as tarefas nascem com `RestartCount=3`, isso falhava em **todas** as
chamadas: o passo 7/8 morria antes de iniciar a tarefa (pendência "nenhuma tarefa chegou a ser
iniciada", instalador em exit 1) e a vigia nunca recuperava travamento nenhum. Além de
impossível, era desnecessário — o reinício por falha só dispara um minuto depois da morte do
processo, e aí a nova instância já está `Running` e o `MultipleInstances=IgnoreNew` a descarta;
se a nova instância não subir, o reinício por falha é a rede de segurança que a pausa jogava fora.

O instalador segura `Local\HangarInstall` e, na janela longa dele, pausa os reinícios automáticos
existentes por **re-registro do XML** (`Suspender-Recuperacao` remove o nó `<RestartOnFailure>`,
`Restaurar-Recuperacao` o repõe no `finally`). `Set-ScheduledTask` não serve nem recebendo um
`-Settings` novo sem recuperação: ele funde com o XML existente e o `<Interval>` sobrevive sozinho.
O re-registro preserva gatilhos, principal, ação, diretório e demais configurações, e **não**
derruba instância em execução (medido com a tarefa `Running` e o processo filho vivo antes e
depois). O restore é cirúrgico — repõe o nó no XML **atual**, porque entre a pausa e o `finally` o
passo 7/8 re-registra a tarefa com caminhos novos e reescrever o XML velho desfaria a instalação
que acabou de rodar; tarefa que já voltou com recuperação própria sai sem toque. A vigia usa a
mesma exclusão e também reconhece processos vivos de instalação/atualização. A exclusão por tarefa
serializa reinícios concorrentes; não há PID ou arquivo de manutenção permanente que possa ficar
preso após um crash.

Verificação no Linux com PowerShell 7: `scripts/test-windows-tasks.ps1` exercita seleção,
identidade, ordem de parada/início, recusa de duplicação, falhas, exclusão entre processos e
restauração após erro. A sonda HTTP foi executada contra servidor local com 200, 404, 500 e
conexão sem resposta.

Verificação no **Windows**, contra o Agendador de verdade: `scripts/test-windows-tasks-reais.ps1`
(pula fora sozinho onde o módulo `ScheduledTasks` não existe). Registra uma tarefa descartável com
a forma das reais, põe para rodar e prova pause/restore: recuperação desligada, definição inteira
preservada, instância viva sobrevivendo ao re-registro, restore cirúrgico depois de um 7/8
simulado e restore idempotente. Existe porque o teste simulado não reprova XML inválido — foi
assim que a pausa por `Set-ScheduledTask`, recusada em 100% das chamadas, chegou a produção. A
prova de UAC e de sobrevivência real do atualizador/psmux no Windows ainda é necessária; os testes
de processo continuam simulados.
Referência: [configurações nativas do Agendador](https://learn.microsoft.com/en-us/powershell/module/scheduledtasks/new-scheduledtasksettingsset?view=windowsserver2025-ps).

## Instalação Windows mantém o nível de permissão

(12/09/2026). `install.ps1` aceita execução comum ou elevada. O processo escolhe `Limited` ou
`Highest` para backend, frontend quando necessário e vigia, sempre com `LogonType Interactive`.
O atualizador é filho do backend e herda sua permissão. O instalador confere nível e logon após
registrar; falha no modo elevado não pode reutilizar silenciosamente uma tarefa comum.

Os atalhos no Menu Iniciar e na Área de Trabalho usam `shell/build/icon.ico`, gerado do
`assets/brand/icon.svg` (com a versão de dois arcos em tamanhos pequenos), e o flag `RunAsUser` acompanha o nível da instalação.
`shell/build/icon.png` usa a mesma imagem na janela Electron e no empacotamento. A Área de
Trabalho vem de `GetFolderPath`, inclusive quando redirecionada. O Electron já aberto precisa
ser fechado pelo usuário para a próxima abertura assumir a elevação.

`Get-InstallRunLevel` consulta a tarefa do backend antes de alterar a instalação: se ela está
em `Highest`, uma chamada comum para com orientação para atualizar pelo app ou usar PowerShell
elevado. Isso preserva a escolha sem configuração adicional. No modo comum, firewall e Modo
Desenvolvedor continuam usando UAC pontual; o reparo de tarefas antigas com dono Administradores
permanece. A antiga proibição está em [superado.md](superado.md).

Verificação: `scripts/test-install-elevation.ps1` exercita os dois níveis, os quatro pontos de
registro com comandos simulados e os bytes de elevação do atalho. Executado com PowerShell 7 no
Linux; UAC, Agendador, atalhos e reinício precisam de validação na máquina Windows.
Referências: [principal das tarefas](https://learn.microsoft.com/en-us/powershell/module/scheduledtasks/new-scheduledtaskprincipal)
e [flags do atalho](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-shllink/ae350202-3ba9-4790-9e9e-98935f4ee5af).

Outras correções medidas em 10/09: com Tailscale publicado o firewall nem é perguntado
(`serve` entrega em localhost, o firewall não vê porta); o HTTPS do tailnet
  desligado abre `login.tailscale.com/admin/dns` e ESPERA Enter pra tentar de novo (um usuário
  ficou parado ali sem saber onde ir); o symlink sem admin é conferido no 1/8 (`Symlink-Funciona`)
  e, faltando, o instalador liga o Modo Desenvolvedor por UAC ou abre `ms-settings:developers` e
  espera — antes a pessoa só descobria ao criar a primeira conta. Medido no 5.1: `Remove-Item`
  num symlink de pasta estoura `NullReferenceException`; apagar link é `[IO.Directory]::Delete`.
  **A sonda é `cmd /c mklink /D`, não `New-Item -ItemType SymbolicLink`** (medido 10/09/2026 na
  VM, PowerShell 5.1.26100, token restrito via `runas /trustlevel:0x20000`): com o Modo
  Desenvolvedor LIGADO o `New-Item` do 5.1 ainda falha com "requer privilégio de administrador",
  porque não pede o flag de criação sem privilégio; `mklink` e o `os.symlink` do Python (quem cria
  os atalhos das contas) funcionam. Com o `New-Item` o instalador dizia "modo desligado" pra quem
  tinha acabado de ligar. A sonda nova dá `False` com o modo desligado e `True` ligado.
  E `(Get-Command npm).Source` é `npm.ps1`: o `.vbs` da tarefa do front chamava
  `cmd /c "...\npm.ps1" run preview`, o cmd abria o `.ps1` no Notepad e a vigia repetia isso a
  cada 5 min — `-CommandType Application` pra resolver lançador.

## Instalador guiado: duas perguntas no passo 0, o resto é padrão

(`install.sh --avancado` /
  `install.ps1 -Avancado` devolvem o wizard; 09/09/2026, spec em
  `docs/superpowers/specs/2026-09-09-instalador-guiado-design.md`). Decisão do usuário para o
  time não técnico: token (continua pergunta — é o que se digita no celular quando o QR não
  dá) e "usar fora de casa?" (Tailscale instalado e logado no 1/8, publicado no 6/8 — o Linux
  passou a rodar `sudo tailscale serve` como o Windows já fazia). Quatro sabores de pergunta:
  `ask` sim por padrão, `ask_senha` sempre pergunta (sudo nunca aparece "do nada"), `ask_extra`
  não por padrão (persistência tmux, painel), e sem terminal tudo é NÃO — o `Pergunte` do
  Windows foi alinhado a isso (antes, sem console, respondia sim). Log em
  `~/.hangar/install.log` / `%LOCALAPPDATA%\hangar\install.log`, nunca no `--update` (o app
  lê `##HANGAR-AVISO##` da saída crua) e SEM o token: a URL de pareamento do `print_pairing`
  carrega o token, então QR e URL vão só pro `/dev/tty` e, no Windows, com o `Start-Transcript`
  pausado (ele captura `Read-Host` e `Write-Host`). O "pull automático" que parecia redundante
  com o Atualizar do app é o hook `post-merge`: complementares (o botão chama o mesmo
  `--update`); ele passou a instalar sem perguntar. `hangar-doctor` é UMA implementação
  (`app/doctor.py`), com cwd em `backend/` porque o `Settings` lê o `.env` pelo diretório
  atual; login do Claude é `EstadoLogin.loggedIn`, não `estado` (que só diz se o CLI
  respondeu); LAN é `estado == "ok"` do `alcance`. `--check`/`-SoChecar` delegam a ele.
  `-Update`/`--update` só baixa o dist do CI (sha diferente → o mais recente publicado, com
  aviso); nunca `npm ci`/build no update; sem download vira pendência `frontend` e o backend
  reinicia mesmo assim. Motivo medido na VM Windows: `.Content` do `.sha` vem como `byte[]` no
  PS 5.1 (`.Trim()` estourava) e o build local morre em `@rollup/rollup-win32-x64-msvc` ausente
  (npm ci com lock gerado no Linux). No Linux, `--check`/`-SoChecar` e `hangar-doctor` chamam
  `uv run --no-sync`; no Windows, `-SoChecar` (`install.ps1`) e o `hangar-doctor.cmd` chamam o
  `python.exe` do venv direto, sem `uv` — o mesmo efeito, nada é sincronizado.

## O bloco do MCP `hangar` no Codex é reconhecido pela tabela, não só pelos marcadores (30/09/2026)

`scripts/registrar-mcp.py` gravava `[mcp_servers.hangar]` entre `# >>> hangar: mcp` e
`# <<< hangar: mcp` e, ao rodar de novo, só procurava os marcadores: sem eles, anexava o bloco no
fim. Medido no Windows com o app desktop do Codex (26.928) logado: ao trocar modelo, ativar
plugin ou logar, o app reescreve o `config.toml` inteiro a partir do modelo interno dele — sem
comentários e com `http_headers` em subtabela (`[mcp_servers.hangar.http_headers]`). O servidor
continua lá, os marcadores não. No `-Update` seguinte o registrador anexou de novo e o arquivo
ficou com `[mcp_servers.hangar]` duas vezes: o Codex loga `Invalid configuration; using defaults
… duplicate key`, descarta o config inteiro, e o app cai em "Não foi possível carregar as
configurações da organização" — parecia rede, era parse. Aconteceu duas vezes no mesmo dia; na
primeira, o bloco em subtabelas foi tomado por "escrito à mão" e apagado, e o ciclo voltou.
Agora o registrador remove o bloco marcado, confere pelo `tomllib` se o app já gravou o servidor
com a mesma URL e o mesmo token (nada a fazer) e, senão, tira toda seção `[mcp_servers.hangar…]`
antes de anexar o bloco marcado — idempotente contra a reescrita do app e autocorretivo num
arquivo já duplicado. Teste em `scripts/test_registrar_mcp.py`.

## O instalador exige um agente de código, não o Claude Code

(06/10/2026) O `install.sh` e o `install.ps1` tinham o Claude Code como dependência obrigatória
do passo 1/8: sem ele a instalação parava em "faltam: Claude Code", mesmo para quem só usa Codex.
O Codex era só verificado e Pi, omp e Kimi nem isso. O pedido foi: o Claude continua o padrão,
mas não pode ser o único, e tem que haver pelo menos um agente.

Sem a opção, a escolha sai do disco e não vira pergunta nova, porque o guiado mantém as duas
perguntas: os agentes que já existem bastam, e só sem nenhum o Claude Code é instalado.
`--agentes=`/`-Agentes` e o `--avancado` escolhem explicitamente. O Claude segue pelo caminho
próprio (no Windows o `Instale-ClaudeCode` conserta o PATH que o instalador da Anthropic não
põe). Os outros rodam no 2/8, já com o venv, por `python -m app.harness_commands <cli>`: um
módulo só com biblioteca padrão para o instalador não carregar o backend inteiro, e que o painel
de Harnesses também lê, para o comando conferido de cada fornecedor morar num lugar só. Kimi no
Windows não tem comando conferido e vira pendência com o link. A prova do 2/8 é ter algum agente
no PATH; o `hangar-doctor` só dá erro quando não há nenhum.

## O `sudo` sem terminal: `sudo -S`, não `SUDO_ASKPASS`

(06/10/2026, bloco 1 do instalador gráfico.) O assistente do app nativo roda o `install.sh --app`
sem terminal e precisa entregar ao `sudo` a senha de administrador que a pessoa digitou na janela
do app. O caminho óbvio, `SUDO_ASKPASS` + `sudo -A`, não existe no `sudo-rs`, o `sudo` padrão do
Ubuntu 25.10. Medido em contêiner descartável (`docker run --rm`, sem terminal: `setsid`, stdin
nulo), com usuário comum no grupo de administradores e um auxiliar que imprime a senha:

| Medição | Ubuntu 25.10 (`sudo-rs 0.2.8`) | Fedora (`Sudo version 1.9.17p2`) |
|---|---|---|
| `sudo -A id -u`, com `SUDO_ASKPASS` | `invalid option provided`, rc 1 | o `-A` chama o `SUDO_ASKPASS`: `0`, rc 0 |
| `sudo id -u` (sem `-A`), com `SUDO_ASKPASS` | o `SUDO_ASKPASS` é ignorado: `sudo: Authentication failed, try again.` (2×) e `sudo-rs: Maximum 3 incorrect authentication attempts`, rc 1 | o `SUDO_ASKPASS` é ignorado: `sudo: a terminal is required to read the password; either use the -S option to read from standard input or configure an askpass helper` e `sudo: a password is required`, rc 1 |
| `aux \| sudo -S -p '' id -u`, senha certa | `0`, rc 0 | `0`, rc 0 |
| `aux \| sudo -S -p '' sh -c 'sudo id -u'` | o `sudo` de dentro não pede nada: `0` | o `sudo` de dentro não pede nada: `0` |
| `sudo -S`, senha errada | `sudo: Authentication failed, try again.` (2×) e `sudo-rs: Maximum 3 incorrect authentication attempts`, rc 1 | `Sorry, try again.`, `sudo: no password was provided` e `sudo: 1 incorrect password attempt`, rc 1 |
| `sudo -n true` logo depois de um `sudo -S` bem-sucedido, mesmo processo-pai | rc 0 | rc 0 |
| `aux \| sudo -S -p '' -v` e depois `sudo -n true` e `sudo -n <comando>`, sem terminal (direto, segunda chamada, em `$(...)`, em cano) | rc 0 nas quatro | rc 0 nas quatro |
| usuário fora do sudoers, senha certa dele | `sudo-rs: I'm sorry <usuário>. I'm afraid I can't do that`, rc 0 1 | `<usuário> is not in the sudoers file.`, rc 0 1 (antes dele, o aviso "We trust you have received the usual lecture…", que o `sudo` clássico imprime no primeiro uso de cada usuário) |
| auxiliar sai ≠ 0 (a pessoa cancelou) | `sudo: Authentication failed, try again.` (2×) e `sudo-rs: Maximum 3 incorrect authentication attempts`, rc 1 1 | `sudo: no password was provided` e `sudo: a password is required`, rc 1 1 |
| auxiliar ausente (caminho que não existe) | `bash: line 1: /nao/existe/askpass: No such file or directory` e as mesmas linhas da senha errada, rc 127 1 | `bash: line 1: /nao/existe/askpass: No such file or directory`, `sudo: no password was provided` e `sudo: a password is required`, rc 127 1 |
| `sudo -S -k -v -p ''` (conferência do app), senha certa / errada | rc 0 0 / `sudo: Authentication failed, try again.` (2×) e `sudo-rs: Maximum 3 incorrect authentication attempts`, rc 0 1 | rc 0 0 / `Sorry, try again.`, `sudo: no password was provided` e `sudo: 1 incorrect password attempt`, rc 0 1 |
| `apt-get install` de pacote que não existe, pelo `sudo -S` | `E: Unable to locate package pacote-que-nao-existe-hangar`, rc 0 100 | — |

No `sudo-rs`, auxiliar cancelado ou ausente sai com o mesmo texto da senha errada: só o código de
saída do auxiliar separa os casos.

Decisão: no `--app`, todo `sudo` do `install.sh` passa pela função `app_sudo`. Ela tenta `sudo -n`
(a credencial que o sistema já guardou) e, sem ela, só autentica: `"$HANGAR_ASKPASS" "<motivo>" |
sudo -S -p '' -v`. A senha só existe no cano entre os dois, nunca em variável, argv ou saída. Senha
recusada pede de novo com `"$HANGAR_ASKPASS" "<motivo>" --retry`, até 3 vezes. O comando roda
depois com `sudo -n <comando>`, sem a senha no stdin: com `sudo -S <comando>`, uma regra `NOPASSWD`
deixaria a senha na entrada do comando, e a saída dele se confundiria com a do `sudo`. Se o `-v`
passou e o `sudo -n` ainda pede senha (`timestamp_timeout=0`), a `app_sudo` para com a frase
`SUDO_NO_CACHE` e o código `sem-sudo`. O script da
Tailscale, que chama `sudo` por dentro, roda inteiro como administrador
(`sh -c 'curl … | sh'` pela `app_sudo`). A causa da falha sai do texto: fora do sudoers →
`sem-sudo`; auxiliar saiu ≠ 0 ou três senhas erradas → `senha-cancelada`; `apt` sem o pacote →
`pacotes-desatualizados`. Sem `HANGAR_ASKPASS` (o `--app` rodado à mão) a `app_sudo` nem chama o
`sudo -S`, e a falha fica sem código. Nunca `SUDO_ASKPASS`, `sudo -A` nem `pkexec` (pede a cada
comando e, sem agente de polkit rodando, a janela nem aparece).

## Modo `--app`: o assistente responde pelas telas

(06/10/2026, spec `docs/superpowers/specs/2026-10-06-instalador-grafico-design.md`, bloco 1.) O
assistente do app nativo instala pelos mesmos `install.sh`/`install.ps1`, sem terminal, lendo
marcas na saída. A regra "sem terminal, tudo é NÃO" continua valendo para CI, ssh e `curl | bash`;
o `--app`/`-App` é a exceção declarada, porque a pessoa já respondeu nas telas: `ask` sim,
`ask_senha` sim, `ask_extra` não, Tailscale por `--tailscale=sim|nao` (`nao` vale mesmo com ela
instalada). A senha de administrador vem da janela do app: no Linux todo `sudo` passa pela
`app_sudo`, que pede a senha ao auxiliar do app (`HANGAR_ASKPASS`) e a entrega ao `sudo -S` pelo
cano; no Windows o `Eleva-E-Roda` pede o UAC mesmo sem terminal. Nada espera
teclado: o login da Tailscale vira `##HANGAR-LINK## tailscale-login` e o HTTPS desligado vira a
pendência `tailscale-https`, e quem roda de novo é o app. A senha do celular chega por
`HANGAR_TOKEN` (nunca argv nem saída) e sai do ambiente antes de qualquer instalador de terceiro;
token já gravado é mantido. Ela recusa o que não é ASCII visível (acento), `#`, `$`, aspas, `\` e espaço nas pontas: o backend lê o
`.env` pelo python-dotenv, que corta o valor em ` #`, expande `${VAR}` e tira aspas, e a senha
gravada sem aspas não chegaria inteira. O `--app` não abre o app nem o navegador no fim, não
imprime o token, e no Windows escreve em UTF-8. O bootstrap puxa sem hooks (`core.hooksPath`
vazio): o post-merge rodaria um `--update` inteiro antes do `##HANGAR-PROTOCOLO##`. Ele também
tira `HANGAR_TOKEN`/`HANGAR_ASKPASS*` do ambiente e só os devolve na linha do instalador. No `install.ps1`, o portão do 1/8 só barra
pendência sem código: as que levam código (`Add-AppPending`) são extras, e o app as mostra no fim.

Marcas, uma por linha: `##HANGAR-PROTOCOLO## 1` (primeira), `##HANGAR-PASSO## <etapa> <estado>`,
`##HANGAR-ITEM## <id> <estado> <texto>`, `##HANGAR-LINK## tailscale-login <url>`,
`##HANGAR-ERRO## <código>` (logo antes da falha essencial, também fora do `--app`),
`##HANGAR-PENDENCIA## <código> <texto>` e `##HANGAR-FIM## ok|pendente|falhou` (última, de um
`trap EXIT` no bash e do `finally` no PowerShell). `##HANGAR-FALHA##` e `##HANGAR-AVISO##` seguem
como estão (`atualizar.py`). Mudou alguma marca: sobe o número do `##HANGAR-PROTOCOLO##` nos dois
instaladores e no app.

Sudo sem terminal, medido em [O `sudo` sem terminal: `sudo -S`, não `SUDO_ASKPASS`](#o-sudo-sem-terminal-sudo--s-não-sudo_askpass):
a senha entra pelo `sudo -S`, porque o `sudo-rs` do Ubuntu não aceita `-A` nem lê `SUDO_ASKPASS`, e
o script da Tailscale roda inteiro como administrador dentro de `sh -c`.

## Atualizar para no binário publicado (06/10/2026)

Na máquina do Waldir (Linux, canal `hangar-server-parte1`) o Atualizar foi ao topo `a810120b0`,
no contrato 36, com o binário mais novo publicado ainda no 35: o Supervisor recusou o
hangar-server e o Python atendeu sozinho ("o binário fala outro contrato interno"). O
`server.yml` publica minutos depois do push, e só quando `crates/` ou os caminhos dele mudam.

O manifesto já traz o `commit` do build, então a escolha não precisa de nada novo no publish:
o protocolo daquele commit é o do binário. A busca percorre só os commits de primeiro pai que
mexeram na linha do `RUST_SERVER_PROTOCOL` (`git log -m --first-parent -G`; `--diff-merges` só
existe do git 2.31 em diante), e o pai de cada um é o último com o número anterior. O aviso não
promete que o build vem: o mesmo estado aparece com o CI quebrado. Prova em clone descartável, 06/10: checkout em `d99d02e9` (34), release
`server-hangar-server-parte1` com o binário de `ce73e54f3` (34), topo `73727328e` (36): parou
em `5b103eede`, o último commit no 34, e baixou os dois binários com o sha256 conferido.

Build de UM sistema que falha: no manifesto antigo ele sai da lista e o asset velho fica sem
commit conhecido, então esse sistema vai ao topo, como antes. O manifesto por sistema
(`platforms.<sistema>` com `commit` e `protocol`) resolve: a escolha usa a entrada do próprio
sistema, e o `commit` do topo só vale quando a chave `platforms` não existe.

## Verificação local antes do push (06/10/2026)

Cada tarefa levava ~4 h, ~3 delas esperando CI. A trava `pre-push` só rodava os testes ligados aos
arquivos quando conseguia perguntar pelo `/dev/tty`; o comando de push de um agente nunca tem esse
canal, então os testes eram pulados e o primeiro lugar onde rodavam era o CI (Windows: 25–40 min
por rodada).

Medido nesta máquina (16 núcleos, `CARGO_BUILD_JOBS=4`, `nice`/`ionice`), commit `c14dffe8f`:

| Passo | Frio | Quente, sem mudança | CI (mediana do job) |
|---|---|---|---|
| rust: `cargo test --workspace` + contrato do cano | 280 s | 169 s | Server Linux 865 s |
| backend: pytest inteiro (+ contratos Rust) | 711 s | 680 s | backend 848 s |
| front: check, vitest, build | 382 s | 386 s | frontend 791 s + dist 66 s |
| nativo: `cargo test` do `desktop-native` | 475 s | 2 s | (o CI só compila release) |
| Rodada inteira (`--tudo`) | 1874 s | 1263 s | — |

Carga máxima (load 1 min) durante as rodadas: 6,3 a frio, 3,9 quente. Uma rodada de verdade só roda
os passos do que mudou, e árvore que já passou não roda nada.

- **Árvore fixa, não `CARGO_TARGET_DIR` compartilhado.** Target dividido entre checkouts rodou
  binário velho. Aqui o checkout é sempre o mesmo, e o `git checkout` muda a data só do que mudou,
  então o cargo recompila só isso. Prova: dois commits irmãos mudando uma string do
  `hangar-server`, verificados em sequência; o binário que o pytest usa trazia só a string do commit
  da vez (depois de B: `A=0 B=1`; de A de novo: `A=1 B=0`). Essa rodada, mudança de uma linha
  em `crates/`, levou 761 s: Rust 170 s, pytest 585 s.
- **Depuração só de linhas** (`CARGO_PROFILE_DEV_DEBUG=line-tables-only`). Cada arquivo de teste
  vira um binário; com depuração completa o target do `crates/` tinha 27 GB no Linux, e na VM
  encheu os 14 GB livres do C: no meio da compilação. Com só linhas: 11 GB. No Windows também sem
  incremental: o target lá fica em 5,4 GB e mora na VM entre rodadas. Mínimo de 4 GB livres para
  começar e um vigia que interrompe o passo abaixo de 2 GB.
- **Ambiente de runner**: HOME vazio, `TMUX_TMPDIR` e `XDG_RUNTIME_DIR` próprios, `CI=1`, só o PATH
  das ferramentas. O `omp` é o da versão fixada no `ci.yml`, baixado com sha256; sem ele 20 casos
  falham. A pasta temporária do pytest fica em `/var/tmp`, fora do HOME e fora do tmpfs.
- **Windows na VM roda só Rust e os 6 `test_runtime_*`**, como o `server.yml`. O pytest inteiro lá
  nem coleta (`fcntl`, `os.geteuid`), e testes com `tmux -L … kill-server` falariam com o psmux
  real da VM. O `python3` da VM é o atalho da Microsoft Store: o teste do cano lança `python3`, e
  sem um executável de verdade (venv fixo em `C:\hangar-verificacao\py3`) dava `10054`.
- **O que a VM não reproduz**: o relógio dela está em 1 ms (`NtQueryTimerResolution`, algum
  processo o ergue), e o padrão do Windows é 15,625 ms: teste de prazo que passa aqui pode falhar
  no runner; 4 núcleos dedicados contra 4 vCPU compartilhadas, Windows 11 26100
  contra Server 2022, e o cache frio de cada rodada do CI.
- **O que só o CI faz**: build release com LTO fat (`zigbuild` com glibc 2.28 no Linux), macOS, a
  publicação dos binários e do `dist-latest`, e o `passos` (o pre-commit faz o mesmo por commit).
- **Cache do Rust no Actions: só a `main` grava, toda branch lê o dela.** Em 07/10 o repositório
  estava em 7,3 GB de 10 GB, com cópias de 2 a 3,6 GB gravadas por branches `fix/*`; a parte1
  tinha perdido o cache de Linux e Windows. O Windows do `server.yml` levou 1503 s frio contra
  1144 s quente na mesma árvore (PRs de medição #95/#96).
- **Exigido só onde vira PR ou main (decisão do dono, 07/10).** Exigir em todo push travava quem só
  quer guardar trabalho numa branch. O que importa é o que vai ser revisto e juntado: o PR nasce
  verificado inteiro, e cada push seguinte nele ou na `main` também.
- **`release.yml` saiu.** Empacotava o shell Electron em tag `v*` (última em 25/08); nada no
  instalador, no Atualizar, no app nativo nem no site lê essas releases.

## Verificação local em paralelo (09/10/2026)

A rodada inteira levava ~27 min no Linux, com os passos em fila: rust 176–247 s, backend 718–755 s,
front 366–448 s, mobile 97–114 s, nativo até 150 s. Um passo instável custava a rodada inteira, porque
o registro só era gravado quando todos passavam, e com o Node 26 do sistema front e mobile falhavam
em massa (o jsdom fica sem `localStorage`).

Medido nesta máquina (24 núcleos, outras sessões trabalhando, load average até 29), `--tudo`, padrão
de 4 processos: preparo 24 s; rust 177 s, backend 180 s e nativo 12 s em faixas próprias; front 318 s
e mobile 139 s na mesma faixa; shell, statusline e pi 8 s. Caminho crítico ≈ 8 min.

- **pytest-xdist com `--dist loadfile`.** O backend sozinho: 755 s em fila; com xdist, 171 s com 4
  processos, 95–117 s com 8 a 16. Arquivo inteiro por processo porque há testes com nome fixo de
  sessão tmux (`cp-test-termsock`), que colidiam quando o mesmo arquivo se dividia.
- **A suíte dependia da ordem dos arquivos.** Em fila, na ordem alfabética do CI, passava; repartida,
  cada rodada quebrava um arquivo diferente, e cada um passava sozinho. Três origens: o Timer de
  confirmação de entrega do `app.api` (~8,5 s) sobrava de um teste e falava com o tmux no meio do
  arquivo seguinte (`test_api.py` → `test_terminal_observer.py`, e por tempo o
  `test_create_modelo_api.py`); e `test_codex_wrapper.py` tirava `app.adapters` do `sys.modules`
  sem devolver, e o pacote reimportado nascia sem o atributo `adapter`; e `test_sync.py` recarregava
  o `app.api` (`importlib.reload`), trocando o `app` global, e o `test_create_modelo_api.py` seguinte
  perdia o serviço de contas Codex que tinha montado. Detector usado: um plugin de
  pytest que lista as threads vivas ao fim de cada arquivo; o terceiro, por bissecção da ordem.
- **Preparo único.** Com os passos em paralelo, um `cargo build` de uma faixa reescrevia binário que
  outra executava, e o pytest chegava antes das sondas de contas (~30 falhas falsas). Os passos não
  compilam mais nada no Linux.
- **O vitest usa `--maxWorkers`** com os processos configurados: os `vitest.config` limitam a 2 forks
  para várias sessões não encherem a RAM, e aqui a verificação roda sozinha na fila.
- **Dois testes do `hangar-server` liam a configuração de quem roda:** o tmux isolado lia o
  `~/.tmux.conf` (`base-index 1` mudava o alvo `=test:0.0`), e um `core.hooksPath` global fazia o
  remoto do teste de vagas ignorar o `pre-receive`. Agora `-f /dev/null` e o `core.hooksPath` do
  próprio remoto.
- **Cache Rust do CI.** O cache do `server` no Windows tinha 5,5 GB; com o macOS (2,8 GB) e o do
  backend, o repositório chegava ao limite de 10 GB, e o GitHub despejava o do Linux: o PR #118 deu
  `Cache not found` no `server-Linux` e compilou do zero (5 min 25 s de testes, 7 min 46 s de
  release, 1119 s no job). Os jobs que guardam `crates/target` compilam sem incremental e com
  depuração só de linhas, e as chaves ganharam `-v2-`: o `save` só grava quando a chave exata não
  existe, e sem chave nova o cache enxuto nunca seria gravado.
