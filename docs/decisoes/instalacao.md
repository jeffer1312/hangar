# Instalação, atualização e serviços

Decisões medidas, com data e número. O `CLAUDE.md` carrega a regra;
a medição que a sustenta mora aqui. Conteúdo movido sem alteração.

## Restarting the backend.

No `--reload` (it holds SSE + watchfiles). `pkill -f app.main` can match your
  own shell; SIGTERM can hang on an open SSE connection. Kill `-9` the pid bound to the port and relaunch
  detached (`setsid`).

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
    anterior — já derruba a instância velha (`Pare-Servico`) e chama `Start-ScheduledTask`, e esse
    bloco NÃO é pulado no modo `-Update` (o que ele pula é firewall/Tailscale e o hook). Há ainda
    a tarefa `hangar-vigia`, que sobe a tarefa de novo se a porta não estiver escutando. Por isso
    `_reiniciar` não faz nada no ramo Windows: marcar "falta reiniciar" ali fazia a tela pedir um
    passo que já tinha sido dado. Medido em 25/08/2026 naquela máquina: três tarefas
    (`hangar-backend`, `hangar-frontend`, `hangar-vigia`), backend como cadeia de três processos,
    e todas em `Ready` mesmo com o servidor vivo — o `.vbs` não espera.
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

## O instalador do Windows NUNCA roda elevado, e admin é UAC pontual

(`install.ps1`,
  `Eleva-E-Roda`/`Espere-Ate`, 10/09/2026). Tudo que ele registra (tarefas agendadas, hooks,
  config) nasce com o dono do processo, e o Atualizar do app roda como usuário: instalar elevado
  deixava `hangar-backend`/`-frontend`/`-vigia` com dono Administradores e todo `-Update`
  seguinte batia em "Acesso negado" no `Register-ScheduledTask -Force` — a vigia nem tinha o
  fallback de reaproveitar, e a tela dizia "NAO terminou: vigia" com tudo no ar (relato de
  10/09). Hoje: `EhAdmin` no começo → `Pare` (menos `-SoChecar`); firewall e Modo Desenvolvedor
  saem por `Start-Process -Verb RunAs` de UM comando; tarefa de dono admin é reparada com
  `Unregister` elevado + re-registro como usuário (só interativo — no `-Update` não há quem
  confirme o UAC, então reaproveita). Junto: com Tailscale publicado o firewall nem é
  perguntado (o `serve` entrega em localhost, o firewall não vê porta); o HTTPS do tailnet
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
