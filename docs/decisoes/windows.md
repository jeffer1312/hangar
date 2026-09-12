# Windows — psmux, ConPTY, instalador, armadilhas de plataforma

Decisões medidas, com data e número. O `CLAUDE.md` carrega a regra;
a medição que a sustenta mora aqui. Conteúdo movido sem alteração.

## Process info lives in `app/procinfo.py` — the only OS-bound layer.

Nine functions
  (`_proc_children_map`, `_descendant_pids`, `_open_jsonl`, `_cmdline`, `_config_dir_of`,
  `_proc_start_time`, `_engine_of`, + the two `_proc_*_path` test seams) hold **every** `/proc`
  read in the backend; `registry.py` imports them and no longer knows what OS it's on. Four rules:
  (1) the implementation is chosen **once, at import, by capability** (`Path("/proc").is_dir()`),
  never by OS name — "is it unix?" says YES for macOS, which has no `/proc` and would silently read
  nothing; (2) **Linux does not move to psutil** — `open_files()` is orders of magnitude slower than
  listing `/proc/<pid>/fd` and these run per poll, per session; (3) both implementations live in
  **one module**, not three — with `procinfo.py` importing from a `procinfo_proc.py`, a monkeypatch on
  `procinfo._proc_stat_path` wouldn't reach the caller inside it and the test would pass by *accident*
  reading real `/proc`; (4) `psutil` is a **platform-conditional** dependency
  (`sys_platform != 'linux'`), so a Linux install doesn't download it — but it's an unconditional
  *dev* dependency, because `tests/test_procinfo.py` forces `_TEM_PROC = False` on Linux to exercise
  the Windows/macOS path against real processes. Without that, code that only runs off-Linux would
  never be tested by anyone developing on Linux.

## No Windows o backend continua tarefa "no logon, interativa" — subir sem login foi medido e descartado

(10/09/2026, VM WinBoat). Tarefa S4U ("esteja o usuário logado ou não") ou serviço
  (WinSW) sobem sem ninguém logado, e funcionam: backend na sessão 0 criou psmux, o Claude Code
  abriu logado, e o envio multi-linha (clipboard + `M-v`) chegou inteiro, porque backend e o psmux
  que ele cria dividem a área de transferência da sessão 0 (o psmux se encontra por
  `~/.psmux/<sessão>.{port,key}`, TCP loopback, visível de qualquer sessão do mesmo usuário). O
  que levou a descartar esse modo naquele teste foi o TOKEN: o logon não interativo da conta
  administradora usada veio com integridade Alta (`S-1-16-12288`), mesmo com `RunLevel Limited`.
  Na época, o instalador recusava execução elevada e a atualização falhava; as sessões também
  herdavam admin. Hoje a instalação elevada mantém as tarefas e atalhos elevados (ver
  [instalacao.md](instalacao.md#instalação-windows-mantém-o-nível-de-permissão)), mas o gatilho
  continua sendo logon interativo; execução sem login não foi adotada. O que ficou do
  estudo: `tmux._sessao_windows_de` — o backend só cola pelo clipboard se o psmux do pane está na
  MESMA sessão do Windows que ele (cada sessão tem a sua área de transferência; um backend subido
  por SSH ou tarefa e um `claude` do terminal gráfico não dividem), senão cai no linha a linha.

## Windows runs on psmux, not tmux

(`marlocarlo.psmux` — native ConPTY multiplexer that publishes a
  `tmux` alias, so `tmux.py` calls it unchanged). Measured on psmux 3.3.7: `new-session -e`, exact
  `=NAME:` targets, `-F` formats incl. `#{?alternate_on,...}`, `capture-pane -S` with Unicode intact,
  named keys and the option picker all work. **`paste-buffer` works too** — what cannot carry a
  newline are the **buffers**: `set-buffer` truncates at the first one and `load-buffer` escapes it
  with nothing ever unescaping it back, so multi-line arrives cut either way (measured on this same
  version; an earlier note here claimed `paste-buffer` itself was missing, and that was wrong).
  That is why Windows sends multi-line **through the clipboard** — `Set-Clipboard` over stdin plus
  one `M-v` (`tmux.paste_via_clipboard`), under a module-wide lock, because the clipboard belongs to
  the machine and not to the session. The `paste_text` fallback stays for everything else and
  branches on the **return code**, not on the OS: a multiplexer that lacks a command says so, and on
  Linux the fast path returns 0 and never reaches plan B. Plan B is one `send-keys -l` per line
  with `C-j` between; a `\n` *inside* the argument makes psmux swallow everything after it, and `\r`
  as a separator glues the lines together (both measured) — and it is precisely the path that was
  measured delivering 309 of 600 lines while returning success, which is why the clipboard exists.
  Probe: `scripts/test-psmux.py` (+ `.ps1`).
  Install: `install.ps1`. Not there on Windows: systemd services and the `codex` shell wrapper. The
  `claude` one **is** there — `install.ps1` step 5/8 dot-sources `scripts/shell/claude.ps1` and
  `claude-conta.ps1` from the PowerShell profile — so a `claude` typed in PowerShell is trackable
  like on Linux; one typed in another shell (Git Bash) is not, and app-created sessions are always
  fine.

## Where psmux and tmux disagree about IDENTITY and TARGETS

(measured on psmux 3.3.7, 22/08/2026).
  These four are not cosmetic: three of them were live bugs, and the pattern is the same — a command
  the tmux docs say **fails** or **addresses one thing** quietly does something else.
  - **`%N` addresses nothing.** psmux numbers panes per SESSION (tmux, per server), so two sessions
    each have `%1`. `send-keys -t %1` did not reach either of them: it landed in the **client's
    current session** — i.e. the app can type a phone prompt, Enter included, into someone else's
    conversation. `agentpane.resolve_target` only returns a `%N` when the session has 2+ panes, which
    is why it stayed latent. The address that works is `=<session>:<window_index>.<pane_index>`;
    `tmux.alvo_de_pane` builds it, and on POSIX returns the `%N` unchanged. `pane_id` is still the
    **identity** (Pi ticket, agentpane cache) — what changed is what serves as an **address**.
  - **`kill-session -t "=<name>"` does not kill.** The `=` (exact match) is honored by `has-session`,
    `display`, `send-keys`, `new-window` and `split-window` — and NOT by `kill-session`, which waits
    **5s** and returns rc=1 with the session still alive. `tmux.alvo_de_kill` is the single place
    that knows this (production and tests share it); test teardowns that missed it left **65** orphan
    servers on this machine and made `test_termsock` fail the NEXT case with "duplicate session".
  - **Killing the last session does not end the SERVER, and `list-sessions` cannot tell you.** psmux
    keeps a pre-warmed `tmux server -s __warm__ -L <socket>` process alive per socket, forever, each
    holding a shell and a console. On an emptied socket `list-sessions` answers rc=0 with **empty
    output** — byte-identical to a socket that never existed — so there is no question to ask the
    multiplexer; the process table is the only answer. This is a *test* leak with a machine-sized
    bill: 70 orphans here on 22/08/2026, ~12,7 GB of working set, and the Claude session running the
    suite died with the VM at its memory ceiling (`0xc00000fd`). Not one test ever went red. The
    cleanup is `kill-server` **on the own `-L` socket** (rc=0, 0,1s, idempotent even on a virgin
    socket) — never bare, which would take down the user's default tmux server. `tests/tmux_teste.py`
    is the single place that knows it (`novo_socket`/`matar_servidor`, which refuses an empty socket),
    and a session-scoped conftest fixture fails the suite if any registered socket still has a live
    process. On Linux the same defect is harmless (the server exits with the last session; a 0-byte
    socket file stays), so the fix is the same command with no OS branch.
  - **`rename-session` to an occupied name overwrites instead of failing** (rc=0). The session that
    was there does not die: it becomes unreachable, with the name pointing at the other one, and both
    processes keep running. `registry.rename` depends on the refusal to fall back to killing the old
    hidden shell, so `tmux.rename_session` now checks first — non-POSIX branch only.
  - **`list-clients` ignores `-F` and invents the tty.** Any format string comes back as the default
    line, every client shows as `/dev/pts/0` (even clients of different sessions), and
    `detach-client -t <tty>` is parsed as a session name. A line from `list-clients` therefore does
    **not** prove a client is attached — what proves it is the `[activity=...]` suffix and the
    `(attached)` flag in `list-sessions`. Worse than useless, in fact: with the session provably
    empty (`#{session_attached}` = 0) `list-clients -t "={name}"` still returns rc=0 and **one
    line** for a client that does not exist, so `assert list-clients == ""` is not a regression
    check there — it is a question that command cannot answer. `#{session_attached}` answers it on
    both multiplexers. `detach-client` is unusable for a different reason: with the exact target it
    answers `no session '={name}'` (rc=1) — the `=` is **not** honored by it, same family as
    `kill-session` above — and without the `=` it drops **every** client of that session, the
    user's own native `attach` included. This is why the terminal panel's Windows teardown is
    **killing our own `tmux attach` process**: measured, it releases just that client, a client of
    another session stays attached, and the session keeps running.

## Where psmux and tmux disagree about CONFIGURATION — and how to tell a real setting from one it merely stored

(measured on psmux 3.3.7, 22/08/2026, on a throwaway `-L` socket). The section
  above is about commands that address the wrong thing; this one is about `bind`/`set` **accepting
  everything**. Same family as the `terminal-features` it once ignored in silence, except here
  reading the value back does not catch it either.
  - **`set -g <anything> <value>` returns 0 and the invented option comes back from `show -g`.**
    So "it accepted it, and I read it back" proves nothing about a psmux option — the read is just
    your own string. What proves a setting is real is it appearing in the **default `show -g`
    listing** (58 entries here) *before* anyone sets it. Use that as the test.
  - **No mouse key name can be bound.** `bind -T root WheelUpPane …` returns rc=0 and is **silently
    discarded** — `list-keys -T root` never shows it, in the plain form or in the nested `if -F`
    one. It is not the table and not `list-keys`: `NPage`, `Home`, `F5`, `C-a`, `M-v` and a prefix
    `bind X` all store fine, while `WheelUpPane`, `WheelDownPane`, `WheelUpStatus`, `MouseDown1Pane`
    and `MouseDrag1Pane` behave exactly like a `TeclaQueNaoExiste`. The consequence: the wheel
    recipe from the user's Linux `~/.tmux.conf` (`bind -T root WheelUpPane` + nested `if -F`)
    **cannot be ported**, and a future attempt will get rc=0 the whole way and look like it worked.
  - **`#{mouse_any_flag}` does not exist**; `#{alternate_on}` and `#{pane_in_mode}` do. Measured
    against an app holding the alternate screen and asking for mouse (`?1049h` + `?1000h` +
    `?1006h`): `alternate_on` went `0` → `1`, `pane_in_mode` answered `0`, and `mouse_any_flag` came
    back **empty** — same as an invented variable — even while the app was requesting mouse. So the
    "app that asks for mouse" branch of that recipe has no condition to test, either.
  - **The wheel-into-copy-mode behaviour is OUR option, not a psmux law.** psmux carries three
    settings tmux has no equivalent for — `scroll-enter-copy-mode` (default `on`), `mouse-selection`
    and `pwsh-mouse-selection` — and `docs/tmux.conf.windows.example` already writes
    `set -g scroll-enter-copy-mode on`. That line is what sends the wheel into copy mode; the lever
    is one word, and it is in our own managed block.
  - **Synthetic wheel events could not be delivered** (two attempts, both dead ends worth not
    repeating): writing the SGR sequence (`ESC [ < 64 ; x ; y M`) into a ConPTY's input measures the
    **ConPTY**, which translates input before the client sees it; and a `MOUSE_WHEELED`
    `INPUT_RECORD` posted with `WriteConsoleInput` into a console the client inherited never reached
    it either. Neither triggered copy mode even with the option `on`, which is the behaviour a real
    wheel produces every day — so the result is about the injection, not about psmux. Wheel
    behaviour here is verified by a human scrolling, not by a probe.

## No psmux, `display-message -p '#S'` responde a sessão de QUEM PERGUNTA, não a do cliente anexado

(medido 06/09/2026, psmux 3.3.7, VM WinBoat). É o oposto do tmux, e é por isso que o
  `hangar-send` abandonou esse caminho: lá a resposta é estado global do servidor. Aqui ela acertou
  em todas as configurações testadas — 1, 2 e 3 sessões vivas; com um cliente REAL anexado a outra
  sessão (`session_attached=1` nela, 0 na minha); com `$TMUX` forjado apontando para outro id; e de
  um filho destacado por `Start-Process`. De dentro de um pane de uma terceira sessão, veio o nome
  DELA. Consequência: `--sessao` não é obrigatório no Windows, e o `nomeSessao()` do
  `hangar-preview` (que só tinha esse caminho) estava certo. Copiar o primeiro critério do
  `hangar-send` seria **pior**: `TMUX_PANE` + `list-panes -a` contando ocorrências dá AMBÍGUO aqui,
  porque o psmux numera pane por sessão e duas sessões têm `%1` — contei 2. O que faltava era ler
  `CP_SESSION_NAME` antes (carimbo do nascimento, imune a cliente anexado), validado por
  `has-session -t "=<nome>"` porque um rename deixa o carimbo obsoleto e nome obsoleto endereça
  OUTRA sessão. Fallback continua o `display-message`.

## O `ln -sf` do Git Bash COPIA, devolve 0, e é isso que quebrava os dois CLIs no Windows


  (06/09/2026). Três defeitos em fila, um só culpado. O `hangar-send` se localiza por
  `dirname $(realpath $0)/../backend/.env` e a cópia em `~/.local/bin` procurava
  `~/.local/backend/.env`; o `hangar-preview` é pior, porque o `import` ESM **estático** de
  `../shell/preview_fmt.cjs` é resolvido pelo lugar do ARQUIVO — a cópia morria com
  `ERR_MODULE_NOT_FOUND` apontando `~/.local/shell/`, quebrada **até no Git Bash**. E o
  `install-hangar-send.sh` imprimia `ok: … -> …`, com a seta, nos dois casos: o fallback que diria
  "CÓPIA" só cobre o `ln` FALHAR, e ele não falha. Pior, o script **desfazia** o shim que o
  `install.ps1` já escrevia pro `hangar-send` — ou seja, o comando que a doc manda rodar depois de
  um `git pull` quebrava o `hangar-send`. Hoje a checagem é `test -L` DEPOIS do `ln`, na fonte, e o
  shim (idêntico nos dois instaladores) chama o script do repo por caminho absoluto. No Linux o
  `ln` linka, `test -L` é verdadeiro e o ramo não roda.

## Script sem extensão é invisível pro PowerShell, e a falha é MUDA

(06/09/2026). Sem
  `hangar-preview.cmd`, o `Get-Command` **achava** o arquivo (`CommandType=Application`) e executar
  não produzia nada, com `$LASTEXITCODE` **vazio**; só dentro de um pipeline aparecia
  `RuntimeException :: Não é possível executar um documento no meio de um pipeline`. O cmd.exe ao
  menos diz "não é reconhecido". O corpo do lançador é **node**, não bash — o `hangar-preview` é
  `#!/usr/bin/env node`, e copiar o `hangar-send.cmd` repetiria o erro que o `hangar-conta` já
  pagou (`bash arquivo` não honra shebang).

## O navegador embutido funciona no Windows — com a sessão gráfica ATIVA

(06/09/2026, Electron
  43.3.0 / Chrome 150, psmux 3.3.7). `open`, `list`, `snapshot`, `click`, `fill`, `type`, `press`,
  `wait` e `shot` passam; o `shot` grava PNG real (1280×800, assinatura conferida). Com a janela
  ocluída — sessão RDP/console **desconectada**, `query session` = `Disco` — o teclado
  (`type`/`press`) continua entregando e o **mouse não**: o `click` devolve `rc=0` e ZERO evento
  chega ao DOM (verificado com listener em captura). Não é do `hangar-preview`: um
  `Input.dispatchMouseEvent` por **CDP cru** na página do próprio app respondeu `ok` e também não
  entregou nada. O `fill` cai junto, mas alto, porque confere o foco depois do clique — e a
  mensagem dele culpa a ref, que estava certa. Defeito à parte, do mesmo dia, CONSERTADO: com a
  janela ocluída o `shot` recusava com "não produziu quadro" enquanto um `Page.captureScreenshot`
  **cru** no mesmo alvo devolve um PNG **íntegro** (1600×1000, 47382 bytes, app inteiro legível) —
  ou seja, o quadro existe e é o `capturarPagina` de `preview_ctl.cjs` que desiste dele. O
  culpado era o `TETO_SHOT_CDP` de 3000ms, provado por causalidade: baixado a 100ms ele produz
  exatamente aquela frase, com a janela VISÍVEL. O print sem compositor mede 2071-2902ms aqui —
  a primeira captura consumia 97% do teto. Descartados o `await quadro()` (`Promise.race` de
  500ms, não bloqueia) e o flag `oculto` (a sessão fora do painel tem `oculto=true`, e é
  justamente esse ramo que só tem o `captureScreenshot`). Teto agora 15000ms.

## No Windows, um recado do `hangar-send` pode chegar TRÊS vezes de UM envio só — e a culpa é do oráculo de entrega, não de quem mandou

(06/09/2026). A prova de que o texto chegou é
  comparação de string entre o que foi enviado e o que aparece no transcript; a mensagem
  perdeu **uma contrabarra** no caminho, a comparação não casou, e o reconcile redigitou. O
  log do backend registra `REQUEUE name=win-preview id=6fc37a2f… tentativa=1` e `tentativa=2`
  — um envio, três chegadas idênticas. Medido: a fila durável guardou `\\host.lan\Data\.hangar`
  (duas contrabarras antes de `host.lan`) e o transcript recebeu `\host.lan\Data\.hangar` (uma).
  **Onde some** (medido byte a byte em 06/09/2026, com o pane gravando num arquivo o que recebe,
  em vez de eu contar barra em tela renderizada — foi contando na tela que eu errei antes, e
  cheguei a registrar aqui que o multiplexador estava inocente): é o **argv entre o Python e o
  psmux**, e só quando o argumento vai **entre aspas**. `send-keys -l 'A\\x'` (tem espaço, então
  o `subprocess.list2cmdline` cita) chega no pane como `A\x`, uma a menos; `send-keys -l
  'B\\x'` (sem espaço, sem aspas) chega inteiro. Run maior encolhe igual: 3 viram 2. A causa é
  regra de escape divergente — o `list2cmdline` segue o MSVC, onde contrabarra só é especial
  **imediatamente antes de uma aspa**, e o psmux desescapa `\\` em qualquer lugar dentro das
  aspas. O clipboard está fora disso (round-trip preserva 6 de 6), e o composer do Claude Code e
  o transcript também: quando o pane já recebeu a menos, os dois só repassam o que chegou.
  Consequência prática enquanto isso não fecha: recado repetido no Windows não é o par insistindo; antes
  de responder, olhe `REQUEUE` em `%LOCALAPPDATA%\hangar\hangar-backend.log` e a fila em
  `<config>\.hangar-queue\<sessao>.jsonl`, que guarda o texto ORIGINAL.

## `send-keys` do psmux: `;` corta a linha e o Enter junto não executa

(06/09/2026). O `;` é
  separador de comando do tmux, então `send-keys "a ; b" Enter` digitou só o `a` — e mesmo esse não
  rodou: foi preciso um `send-keys … Enter` **separado** para o shell do pane executar.

## The pane's environment comes from the SERVER on tmux and from the CALLER on psmux — which is why `CLAUDE_CONFIG_DIR` cannot be exported unconditionally

(measured on psmux 3.3.7,
  22/08/2026). tmux gives a new session the env of whoever started the *server*, so `new_session`
  sent `-e CLAUDE_CONFIG_DIR=<value>` **always**, the default included: without it, a server started
  by a `claude-conta contaA` silently births every later session in contaA. psmux has neither half
  of that — the pane inherits the **caller's** env (`ZZ=x tmux new-session …` → the pane sees
  `ZZ=x`) and nothing crosses from one session to the next (a `-e` on session A is invisible to a
  later B). And exporting the default there is not free: for Claude Code, `CLAUDE_CONFIG_DIR` set —
  **even pointing at `~/.claude` itself** — means "read `.claude.json` from INSIDE that folder", a
  file it then creates empty (measured here: `~/.claude.json` 52236 bytes, the real one, against
  `~/.claude/.claude.json` 1259). So **every session created by the app on Windows landed on the
  welcome screen** ("Select login method", theme picker) with the credential intact, reading the
  wrong `settings.json` on the way (that is where the fullscreen TUI went). `tmux._e_config_dir` is
  the one place that decides: on POSIX always (the argument list is byte-identical to before); on
  psmux only when the value **differs** from `~/.claude`, or when the backend itself declares the
  variable — the pane would inherit that one anyway, so omitting would not erase it. Same rule in
  the Windows shell wrapper (`scripts/shell/claude.ps1`); the POSIX wrappers are untouched. The
  fallback everywhere else already reads absence as `~/.claude` (hooks, sidecars, `projects/`), so
  nothing else moves.

## Windows-only trap in the installer: encoding is per interpreter, and ASCII is never the answer.


  Every launcher `install.ps1` writes carries a PATH inside it (checkout, python, `%LOCALAPPDATA%`
  log — the user's profile name). Measured by executing each file with an accented path, console
  codepage 850: `.cmd` needs the console's OEM codepage (ANSI, UTF-8 and ASCII all fail; UTF-8 plus
  `chcp 65001` also works but changes the caller's console), `.vbs` needs UTF-16LE **with** BOM (ANSI
  works, OEM does not), `.sh` needs UTF-8 without BOM. `Escrever-Lancador` encodes per type; ASCII
  content still comes out byte-identical. And the same file's two other rules stand: the `.env` and
  `settings.json` must NOT have a BOM (`JSON.parse` in Node throws on it), while the PowerShell
  **profile** must — it is the only encoding both 5.1 and 7 can read when the path has an accent
  (5.1 alone writes ANSI, 7 alone writes UTF-8-no-BOM, and each fails to read the other's).

## Two Windows traps that a `subprocess` and a `tmp+rename` hide from you

(measured 22/08/2026 on
  this VM, python 3.14, locale cp1252, console codepage 850). Both are cases where the failure does
  **not** land where you would look for it.
  - **`Path.replace` IS `os.replace`** — `pathlib` calls `os.replace(self, target)` — so
    `tmp.replace(alvo)` carries the exact WinError 5 that `atomico.substituir` exists to survive.
    The first sweep converted only the `os.replace(tmp, alvo)` spelling and left 20 sites written the
    other way, the sidecars with a concurrent reader by design among them (durable queue, the state
    marker read by a hook in another process, the price cache). The guard is now on the **shape**:
    `tests/test_atomico_call_sites.py` walks the AST of `app/` for `<x>.replace(<one positional
    arg>)`, a signature only the file rename has (`str.replace` takes two, `datetime.replace` takes
    keywords). Testing this needs the fake `os.replace` patched on the **`os` module**, not on
    `atomico.os` — same object, and only that reaches the `pathlib` spelling, so the case fails
    against the old code on Linux too.
  - **A strict decode failure in `subprocess` dies in a reader THREAD.** With `capture_output` there
    are two pipes, so Windows reads them in threads: `encoding="utf-8"` without `errors=` on a byte
    that is not UTF-8 prints `Exception in thread` to stderr, `run()` raises **nothing** and
    `stdout` comes back **None** — the caller blows up later, far from the cause (on Linux the same
    code raises `UnicodeDecodeError` from `run()`). This is why `errors="replace"` stays in
    `conta_estado`/`pi_catalog`; what changed is that its output stops being stamped as good — a
    field carrying U+FFFD is dropped (`conta_estado`, so the account keeps working) or refused
    (`pi_catalog`, where the `id` is later TYPED into the TUI). Note also that `encoding="utf-8"`
    **alone** already fixes the cp1252 mojibake; `errors=` covers a different failure.

## `monkeypatch.setattr(os, "name", …)` in a test takes `pathlib` with it — and it does NOT blow up where you patched.

This is how `test_script_ao_lado_do_projeto_nao_e_acusado_de_inexistente`
  shipped green on Windows and could not even start on Linux (fixed in `b4d97790`): forcing
  `os.name = "nt"` to exercise a Windows branch made the test's own helper raise
  `UnsupportedOperation: cannot instantiate 'WindowsPath' on your system` before it asserted
  anything. Measured on 3.14 (win) and confirmed by the Linux run, the mechanism is worth knowing
  because none of it is where you would look:
  - The guard is a subclass `__new__` installed **at import time** by the REAL `os.name`
    (`class PosixPath: if os.name == 'nt': def __new__… raise`). Patching the attribute later never
    moves it, so the raising class is fixed for the whole process.
  - `Path(...)` itself **does not raise** — `Path.__new__` calls `object.__new__(cls)` and skips
    that guard, while still picking the class from the PATCHED `os.name`. So you get a `PosixPath`
    on Windows (or a `WindowsPath` on Linux) and nothing complains yet.
  - The blow-up lands on the first operation that RE-instantiates: `/`, `.parent`, `.with_suffix`
    (all go through `type(self)(...)`). And it is not uniform — measured, `PosixPath("a").is_file()`
    on Windows answers `False` instead of raising, so the wrong-class path can also just lie.
  So: in a test that patches `os.name`, use **`os.path`** (`join`/`isfile`), which is chosen at
  import and does not change class under you. Patching `os.name` is still the right way to exercise
  a `if os.name == "nt"` branch on both systems — it is the `pathlib` in the test's own scaffolding
  that has to go. Sibling cases only escaped by mocking `shutil.which` with a constant lambda.

## `stop_command` on Windows: the return code cannot be read as failure, and the shell won't tell you in a language you can parse

(`projects.py`, measured against the real `cmd.exe`).
  `taskkill /F /IM x` with no such process answers **128**, the Windows sibling of the `pkill`
  returning 1 that made this code ignore `rc` in the first place; a POSIX `stop_command` (common
  when the project came from a Linux box) answers **1** with "not recognized". Charging by `rc`
  turns every stop of an already-stopped project into an error on screen, and the two stderr
  messages come translated into the Windows UI language. What separates them without depending on
  either is whether the command **exists** — so the check runs only **after** a non-zero rc, on the
  first token of the line, with `cwd` added to the search (`cmd.exe` looks at the current directory
  before PATH) and cmd builtins skipped. Not-found → a `ProjectError` naming the command and warning
  about the orphan; found and failed → silence, with rc and the stderr tail in the log. The stderr
  never reaches the screen: it comes in the console's OEM codepage, not the locale's.
