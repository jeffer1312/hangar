# Windows — psmux, ConPTY, instalador, armadilhas de plataforma

Decisões medidas, com data e número. As regras vigentes ficam na seção abaixo (o `CLAUDE.md`
só aponta para cá); a medição que sustenta cada uma mora na entrada de mesmo assunto.

## Regras vigentes

- **Windows roda psmux, não tmux** — e ele aceita comando que não executa. O que a doc do tmux
  diz que falha, aqui às vezes "funciona" errado: `%N` endereça a sessão errada, `kill-session`
  com `=` não mata, `rename-session` sobrescreve em vez de recusar, `list-clients` inventa tty,
  `set -g <qualquer coisa>` volta do `show -g`. Endereço é `=<sessão>:<janela>.<pane>`.
- **No psmux a sessão de quem chama sai do pid no `TMUX` (`/tmp/psmux-<pid>/…`)**, casado com
  `list-sessions '#{pid}'`: o pane é sempre `%1` e o terceiro campo do `TMUX` é sempre `0`.
- **Fora do Linux o Rust lê processos pelo `sysinfo` (`list/procs.rs`), e ambiente vazio é
  ilegível, não ausência**: é o que ele devolve para processo de outro dono, e a exclusão de conta
  precisa separar os dois. Nascimento vem em segundos inteiros e não há fd aberto (o transcript não
  sai do handle, como no `procinfo._open_jsonl`).
- **Multi-linha vai pelo CLIPBOARD**, porque os buffers do psmux cortam no primeiro `\n`. O
  fallback ramifica pelo **código de retorno**, nunca pelo nome do sistema.
- **O composer se lê com `capture-pane -e` também no psmux.** Sem estilo, a sugestão esmaecida
  que o Claude desenha no composer vazio (`❯ Try "…"`) parece texto digitado, e o escritor Rust
  adia toda entrega com `composer_busy`. Medição em
  [O psmux entende `capture-pane -e`](#o-psmux-entende-capture-pane--e).
- **Buffer do psmux é da SESSÃO e ignora `-b`**: sem `-t` o comando fala com outra sessão,
  `show-buffer -b` devolve vazio com rc 0 e o `-F '#{buffer_name}'` não dá o nome real. Leia o
  mais recente com `-t`; no tmux, `-t` é recusado e o código de retorno decide.
- **O ambiente do pane vem do SERVIDOR no tmux e de QUEM CHAMA no psmux** — por isso
  `CLAUDE_CONFIG_DIR` não pode ser exportado incondicionalmente ali.
- **`Path.replace` É `os.replace`** e carrega o mesmo WinError 5; toda troca atômica passa por
  `atomico.substituir` (guarda de AST em `test_atomico_call_sites.py`).
- **Falha de decode em `subprocess` morre numa thread**: `run()` não levanta e `stdout` volta
  `None`. Use `errors="replace"` e não carimbe como bom o que tem U+FFFD.
- **`monkeypatch.setattr(os, "name", …)` leva o `pathlib` junto** e estoura longe de onde você
  aplicou. Em teste que faz isso, use `os.path`.
- **A porta pública em `0.0.0.0` leva um segundo socket em `[::]` dual-stack, que atende igual.**
  No Windows um `0.0.0.0` sozinho não impede outro processo de abrir a mesma porta em dual-stack
  (Node, `python -m http.server`), e quem chega por IPv6 cai nele. Só IPv4 + dual-stack bloqueia os
  dois tipos; quem abre só IPv6 ainda passa. Falha da guarda é aviso no log, nunca queda do servidor.
  Medição em [Porta pública dividida com outro processo](#porta-pública-dividida-com-outro-processo-13102026).
- **Código de retorno no Windows não se lê como falha** (`taskkill` sem processo devolve 128).
  Separe "comando não existe" de "comando falhou"; stderr vem na codepage do console.
- **Captura avulsa do pane (`state/capture.rs`) decide pela saída, não pelo código.** Saída com
  texto é o quadro, com qualquer código; sem saída, código 0 é pane vazio e código ≠ 0 é recusa.
  Recusa e vazio passam pelo `has-session`, que separa a sessão sumida (e o multiplexador calado).
  Byte que não decodifica é erro, nunca estado; no Windows também U+FFFD, que é o byte que o psmux
  não leu (no Linux é texto do pane). A lista e o `Monitor` do Windows usam essa fonte. Ver
  [Captura avulsa pela saída](#captura-avulsa-pela-saída-não-pelo-código).
- **`shutil.rmtree` em pasta onde o git escreveu precisa de `onexc`** que tira o somente-leitura:
  o git grava packs read-only e o Windows recusa o unlink (WinError 5); no POSIX passa.
- **Encoding é por interpretador**: `.cmd` em OEM, `.vbs` em UTF-16LE com BOM, `.sh` em UTF-8 sem
  BOM, `.env`/`settings.json` sem BOM, perfil do PowerShell com BOM. `bootstrap.ps1` é a exceção
  entre os `.ps1`: ASCII puro e SEM BOM, porque roda por `irm | iex` e o `irm` do 5.1 não tira o BOM.
- **PATH lido do registro se EXPANDE e se ACRESCENTA, nunca substitui `$env:Path`**; o PATH de
  usuário se grava como `REG_EXPAND_SZ` (`Set-ItemProperty -Type ExpandString` + `WM_SETTINGCHANGE`),
  nunca por `[Environment]::SetEnvironmentVariable`. O `powershell.exe` filho vem de `$PSHOME`, não
  do PATH.
- **Python em `WindowsApps` não é só o atalho da Store.** O Python Install Manager (o 3.14 do
  winget) também mora lá e executa. O instalador pergunta `sys.executable` ao atalho e grava o
  interpretador real nos lançadores, porque o bash não roda alias de app; quem não devolve caminho
  é a Store. Exceção: o `hangar-codex-tui` usa `backend\.venv\Scripts\python.exe` quando ele
  existe, e o Python dos outros lançadores sem o venv; sem Python real, nenhum lançador é criado. Ver [Python do Install Manager](#python-do-install-manager-22102026).
- **Atalho sem extensão que o instalador grava em `~/.local/bin` ganha um gêmeo `.cmd` na mesma
  pasta.** Sem ele, o
  PowerShell e o `cmd` resolvem o nome para o arquivo do bash e o Windows pergunta com que programa
  abri-lo. Ver [`python3.cmd`](#python3cmd-ao-lado-do-atalho-do-bash-23102026).
- **Instalação Windows mantém o nível de permissão**: iniciada como admin, registra tarefas
  interativas elevadas e atalhos elevados do Electron; comum, usa UAC pontual. Atualização
  manual sem elevação não pode rebaixar uma instalação elevada.
- **A tarefa Windows acompanha o processo até ele terminar.** Reinício controlado não encerra
  a árvore inteira: sessões e atualizador sobrevivem. A vigia confirma falha HTTP, respeita
  instalação/atualização e só inicia outra instância após confirmar a parada da anterior.
- **O backend abre o próprio log (`python -m app.main --log <arquivo>`), nunca pelo `>` do cmd.**
  O `>` abre sem escrita compartilhada, e qualquer neto que sobreviva ao backend com a cópia
  herdada (jest, `kubectl port-forward`, navegador de teste de uma sessão) fazia a subida seguinte
  da tarefa morrer antes do Python, sem uma linha no log. O `open` do Python compartilha escrita;
  preso mesmo assim, o log vai para `hangar-backend.N.log` e o backend sobe. O front segue no
  `cmd`. Evidência em [Log do backend preso por neto](#log-do-backend-preso-por-neto-20102026).
- **Tarefa do backend/front nasce com `-Priority 4` (normal).** O padrão do Agendador é 7
  (abaixo do normal) e passa para os filhos: backend e sessões perdiam a CPU para qualquer
  programa, passavam dos 3 s da vigia e eram derrubados.
- **Conta da tarefa é `MAQUINA\usuario` (`WindowsIdentity.GetCurrent().Name`), nunca
  `$env:USERNAME`.** Com o PC chamado igual ao usuário, o nome curto resolve para a conta da
  máquina e o `Register-ScheduledTask` recusa com `0x80070057 (7,27):UserId:<nome>` — reproduzido
  na DELPHI-02 com `-User $env:COMPUTERNAME`; o nome qualificado registra.
- **Abaixo do build 22523 o psmux roda com console próprio (`PSMUX_CONPTY_DIR`).** O conhost do
  sistema segura a resposta XTVERSION, o Claude Code não liga o mouse e a roda não rola, com ou sem
  Windows Terminal (psmux/psmux#597; a Microsoft não levou o conserto ao Windows 10). O
  `psmux.yml` compila um commit fixo do master do psmux com o `conpty.dll` + `OpenConsole.exe` do
  NuGet `Microsoft.Windows.Console.ConPTY` 1.24.260710001 (a versão provada no 19045) na release
  `psmux-latest`; `install-psmux-conpty.ps1` instala em `~\.hangar\psmux`, na frente do PATH do
  usuário. O psmux lê a variável uma vez, no servidor: só o instalador interativo aplica, porque
  reiniciar o servidor fecha as sessões e pede confirmação. Quando o psmux lançar versão com a
  variável, o build próprio sai e fica o winget.
- **`ln -sf` do Git Bash COPIA e devolve 0**; confira com `test -L` depois. Script sem extensão é
  invisível para o PowerShell, e a falha é muda.
- **O navegador embutido precisa da sessão gráfica ATIVA**: com a janela ocluída o teclado entrega
  e o mouse não. View escondido precisa de `setDeviceMetricsOverride` para ter viewport e print.
- **Recado repetido no Windows não é o par insistindo** — é o oráculo de entrega: o argv entre
  Python e psmux come uma contrabarra quando o argumento vai entre aspas, a comparação falha e o
  reconcile redigita. Olhe `REQUEUE` no log antes de responder.
- **Conexão abortada no accept não pode fechar o listener**: no ProactorEventLoop o
  `_start_serving` do asyncio fecha o listener em QUALQUER OSError do accept. `resilient_accept`
  refaz o AcceptEx nos códigos da conexão (64, 1236, 10053, 10054), e o cancelamento dele é
  síncrono, porque o `_stop_serving` fecha o socket na mesma pilha do `cancel()`.
- **Timeout de subprocesso no Windows mata a ÁRVORE, não só o filho**: o `subprocess.run` mata o
  processo e depois lê a saída sem prazo; o `git.exe` de `Git\cmd` é um lançador cujo filho segura
  o pipe, e a thread fica presa até o git real terminar. `git_ops._run` usa `Popen`, mata os
  filhos pelo `psutil` e drena com prazo.
- **Terminal de atalho no psmux: opções em chamadas separadas, comando num `.cmd`, `%` dobrado.**
  `new-session … ; set-option …` numa chamada só derruba a sessão; texto livre nunca vai no argv
  do psmux (`\n` e `;` quebram), então o comando mora em `<id>-cmd.cmd` e o "Rodar de novo" o
  relê de lá (`%%` desfeito). Opção de valor vazio nem é gravada. O código de saída vem do
  `<id>.exit`, nunca do `pane_dead_status`. Arquivo sem terminal dono é varrido. Medições completas,
  ver [Terminais de atalho no psmux](#terminais-de-atalho-no-psmux).
- **Terminal real do dono no Windows é do Rust, pelo ConPTY do `portable-pty`.** O filho
  (`tmux attach`) morre ANTES de o pseudoconsole fechar, e com filho vivo o pseudoconsole vaza em
  vez de fechar; não há reposição de tamanho; o escritor é o `take_writer` (pipe puro, soltar só
  fecha). A entrada do cliente fica segurada até o `tmux attach` emitir `ESC[?1049h` (o psmux
  joga fora tecla escrita antes dele subir), menos a resposta de posição do cursor, que é do
  conhost e passa na hora; prazo de 5 s ou 64 KiB segurados entregam e vão ao diário
  (`ready_timeout`, `input_held_overflow`; escrita que falha ali, `input_write_failed`). Medição em
  [Terminal real do dono no Windows](#terminal-real-do-dono-no-windows).
- **App nativo: botão de janela dentro de área `Drag` leva `.occlude()`, e a área `Drag` suprime a
  seleção de texto no apertar.** O `WM_NCHITTEST` do GPUI devolve a PRIMEIRA área de controle sob o
  ponteiro na ordem de pintura (`gpui-pre/src/window.rs`, `on_hit_test_window_control`); a barra pinta
  antes dos filhos, então sem tapar o Min/Max/Close vira `HTCAPTION` e o clique move a janela. Em
  `HTCAPTION` o `DefWindowProc` entra no laço modal de mover, que engole o `WM_NCLBUTTONUP`: o GPUI
  recebe o apertar e nunca o soltar, e a seleção de texto da janela (`gpui-base/src/text_selection.rs`,
  que segue o ponteiro sem olhar o botão) fica presa até o próximo clique. O botão não pode dar
  `stop_propagation`: apertar consumido pelo GPUI pula o `nc_button_pressed` e o sistema nunca fecha,
  minimiza nem maximiza. `start_window_move` é vazio no Windows; quem arrasta é o `HTCAPTION`.
- **App nativo: o exe anterior guardado na troca pode ser a imagem de um processo vivo**, e aí não
  se apaga nem se sobrescreve (`os error 5`). Quem guarda o anterior (`update.rs` e
  `install-native.ps1`) apaga os restos que der e, com o `.old` preso, usa `.old-<horário>`. Exe
  em disco que já tem o sha256 da oferta não se baixa nem se troca: o app só reabre. Medição em
  [Troca do app nativo com o exe anterior preso](#troca-do-app-nativo-com-o-exe-anterior-preso).
- **O terminal de sessão nasce FORA do Job do `hangar-server`.** O backend contém o Rust num Job
  que mata o grupo ao fechar (`runtime_process.WindowsJob`), e no psmux cada sessão tem servidor
  próprio, subido pelo `new-session`. Por isso o Job leva `0x800` (permite sair) e o `new-session`
  do `ProcessMux` nasce com `CREATE_BREAKAWAY_FROM_JOB`. Sem isso, todo reinício do backend
  (atualização incluída) mata as sessões que o Rust abriu. Medição em
  [Sessão morta no reinício do backend](#sessão-morta-no-reinício-do-backend-19102026).

## Sessão morta no reinício do backend (19/10/2026)

DELPHI-02, psmux 3.3.x. A atualização das 09:50 reiniciou o backend e a sessão `hangar`, criada
pelo `hangar-server` às 09:35, sumiu (`sessao.sumiu session_gone_without_close`), sem registro de
saída no transcript. Outra sessão, aberta fora do lançador, sobreviveu. O
`runtime-process.json` listava como membros do Job o `tmux.exe server -s <sessão>` e o `claude.exe`
das sessões que o Rust abriu; as abertas por outro caminho não estavam lá.

Sonda com o psmux real, lançada fora do Job da sessão (pelo WMI): processo num Job `0x2000` roda
`tmux new-session`, e o Job é fechado.

| Job do servidor | `new-session` | Sessão depois de fechar o Job |
|---|---|---|
| `0x2000` | sem flag | morta |
| `0x2000 \| 0x800` | `CREATE_BREAKAWAY_FROM_JOB` | viva |
| idem, dentro de Job externo sem `0x800` (como o do backend) | idem | viva |

Sem o `0x800` no Job, a flag no `CreateProcess` não dá erro: o filho fica no Job e morre junto.

## Terminais de atalho no psmux

(29/09/2026, psmux 3.3.8, DELPHI-02.) Medido: opção de usuário (`@cp_*`) grava e volta no `-F`;
`pane_dead_status` vem `0` mesmo para um comando que saiu com 3, por isso o código de saída é
gravado pelo `.cmd` externo num `.exit`; `new-session … ; set-option …` na mesma chamada derruba
a sessão, então as opções vão em chamadas separadas (uma por opção); `capture-pane`, `cursor_y` e
`send-keys -l` + Enter funcionam com `Read-Host` (`Porta [3000]:` lido de volta); `Start-Sleep` e
`Read-Host` dão o mesmo delta de CPU (0 em 2 s), então a pergunta é tela + CPU parados e um prompt
impresso seguido de `sleep` vira pergunta falsa; um filho gráfico do pane (notepad) fica na mesma
sessão do Windows do backend, o que deixa o backend trazer a janela para a frente.

Como o backend lança: o comando do usuário é gravado em `<id>-cmd.cmd` (codepage OEM, CRLF) e roda
num `cmd /c` filho de `<id>.cmd`, que grava `%ERRORLEVEL%` em `<id>.exit` e segura o pane com
`pause` em laço; o psmux recebe `cmd /d /c "<id>.cmd"`. O comando não vai para a opção `@cp_shortcut_cmd`
no Windows (o argv do psmux quebra em `\n` e `;`, e a contrabarra some), e dono vazio não grava
`@cp_shortcut_owner`: opção ausente volta vazia no `-F`.

Como arquivo de lote, `%` muda de sentido (`%20` numa URL, `%1`, `for %i`). O backend dobra todo `%`
que não forma `%NOME%` de variável existente (ambiente ou dinâmica: `CD`, `DATE`, `TIME`, `RANDOM`,
`ERRORLEVEL`); `%VAR:~0,3%` e `%VAR:a=b%` ficam literais. O "Rodar de novo" desfaz a dobra.
Os `.cmd`/`.exit` podem carregar credencial: são apagados ao fechar o terminal, se o start falha e
por varredura (`list_all`) dos que não têm terminal, com carência de 60 s para um start em andamento.

**Medido nesta data** (29/09/2026 via `app.shortcut_terminals` em clone limpo de main at 3964985b):
o lançamento `cmd /d /c "<outer.cmd>"` como um argv só via `subprocess` funciona; o `.exit` recebe
o código real (exit 3 → `exit_code: 3`, `alive: False`); o `pause` segura o pane após saída; o
"Rodar de novo" reutiliza a mesma linha de comando (`exit 3` de novo); a segunda `start_hangar` com
a mesma chave reutiliza a cópia viva (`reused: True`); a regra de `%` funciona (echo URL=a%20b
USER=%USERNAME% imprime `URL=a%20b USER=administrator`, literal `%20`, env var expandida);
detecção de pergunta com `Read-Host 'Porta [3000]'` → `{"text": "Porta", "default": "3000"}`;
entrega de resposta via `terminal_prompt.answer` funciona; limpeza ao fechar é OK (sem orfãos).

**Ainda não medido no psmux:** `set-option` com `;` no valor e `logical_line` numa prompt envolvida
em múltiplas linhas. A sonda é `scripts/probe-shortcut-psmux.ps1` (não rode `taskkill` com PID
até 4: sessão que não sobe deixa o PID em 0). Um PowerShell 5.1 estraga aspas embutidas numa
string; a sonda passa os argumentos separados.

## Tarefa agendada rodava o backend abaixo do normal

(29/09/2026, VM `delphi-02`.) `Get-ScheduledTask hangar-backend` devolveu `Settings.Priority = 7`
e o `python.exe` do `app.main` e os quatro `claude.exe` abertos por ele estavam em `BelowNormal`:
o `New-ScheduledTaskSettingsSet` sem `-Priority` usa 7, e processo abaixo do normal passa a classe
aos filhos. Com a CPU cheia o backend demorava mais que os 3 s do `Test-HangarHttp` e a vigia o
reiniciava como se tivesse caído. Correção: `-Priority 4` no registro, conferido depois do
`Register-ScheduledTask`. Instalações existentes corrigem no próximo `install.ps1`/atualização,
que re-registra a tarefa com `-Force`.

## Timeout do git não liberava a thread no Windows

(26/09/2026, VM `delphi-02`, Git 2.55.0.windows.5 em `C:\Program Files\Git\cmd\git.exe`, Python
3.14.7 do venv do Hangar.) As quedas do backend no Windows vinham precedidas de timeouts de git e
do pool padrão lento (PR #15). Medição com `git -c "alias.dorme=!sleep 20" dorme` e limite de 2 s:
`subprocess.run(timeout=2)` voltou em 20,1 s, o tempo inteiro do git, porque no Windows o `run()`
chama `communicate()` sem prazo depois do `kill()` e o lançador morto deixa `git.exe` → `sh.exe`
→ `sleep.exe` segurando o stdout. Com `Popen` + `psutil.Process(pid).children(recursive=True)`
mortos antes do lançador + `communicate(timeout=1)`: 2,0 s, pipe drenado, nenhum órfão. Duas
rodadas, mesmos números. No Linux o `run()` só espera o filho morto, então o problema não aparecia.
Junto: a atualização em segundo plano do git da lista passou a um pool próprio de 4 threads, e o
"Mais alterados" do painel manda um `/git/files` por vez em vez de um por mudança de contador.

## Listener do backend morria com cliente abortando o accept

(19/09/2026 14:19:22 e 23/09/2026 09:17:57, PR #13.) O app Electron abriu com o backend lento, os
pedidos estouraram o timeout de 8–10 s e um cliente desistiu bem no meio do accept: o `f.result()`
levantou `OSError [WinError 64] The specified network name is no longer available`, o asyncio
logou "Accept failed on a socket" e fechou o socket da 8765. O processo seguiu vivo, sem ninguém
escutando a porta, até a vigia reiniciar a tarefa. O embrulho em `IocpProactor.accept`
(`backend/app/resilient_accept.py`) refaz o AcceptEx nos erros da conexão e deixa o resto seguir
pro asyncio. A primeira versão cancelava o AcceptEx interno por callback: o CancelIoEx rodava no
ciclo seguinte do loop, depois de o `_stop_serving` já ter fechado o socket. O teste ponta a ponta
(`test_proactor_server_keeps_listening_after_winerror_64`) só roda no Windows, e o CI não tem
pytest no Windows.

## PATH da máquina com `%SYSTEMROOT%` cru: o `Atualiza-Path` apagava o próprio `powershell.exe`

Em 22/09/2026, num Windows Server (conta Administrator) o `bootstrap.ps1` clonou e parou em
`The term 'powershell' is not recognized`. `[Environment]::GetEnvironmentVariable('Path','Machine')`
devolvia `%SYSTEMROOT%\System32\WindowsPowerShell\v1.0\` literal: o PATH da máquina estava gravado
como `REG_SZ`, e o .NET só expande `%VAR%` quando o valor é `REG_EXPAND_SZ`. O `Atualiza-Path`
(bootstrap e install) SUBSTITUÍA `$env:Path` por esse texto, e `%VAR%` literal dentro de
`$env:Path` não resolve nada; toda entrada com variável sumia da sessão, inclusive a do PowerShell.
O `install.ps1` fabricava o mesmo estado no PATH de usuário: `[Environment]::SetEnvironmentVariable`
grava `REG_SZ`, então uma entrada `%USERPROFILE%\...` que já existisse ali virava texto morto na
gravação seguinte. Quem lê o PATH do registro agora expande e acrescenta ao PATH vivo (dedup por
`Select-Object -Unique`), quem grava usa `Set-ItemProperty -Type ExpandString` e avisa o Explorer
por `WM_SETTINGCHANGE` (sem o aviso, terminal novo nasce com o PATH velho), e o filho `powershell`
vem de `Join-Path $PSHOME 'powershell.exe'`. Numa máquina com PATH `REG_EXPAND_SZ` nada muda:
expandir texto sem `%` devolve o mesmo texto, e `$PSHOME` é o binário que o PATH acharia.

O primeiro erro vermelho da mesma tela (`The term '#' is not recognized`, linha 1) era o BOM do
`bootstrap.ps1`: o `irm` do 5.1 entrega o U+FEFF junto, a primeira linha vira `﻿# ...` e o
`iex` tenta executá-la. Inofensivo (o script segue), mas o arquivo é ASCII e o BOM saiu.

## Esc após teclas de controle no ConPTY

Em 19/09/2026, no WinBoat com psmux 3.3.7 e Claude Code 2.1.278, o `/btw` respondeu mas não
fechou. `send-keys Escape`, `Esc` e o byte ESC cru retornaram sucesso, mantendo o diálogo
aberto; anexar um cliente também não resolveu. A sequência Win32 de pressionar/soltar Esc
fechou o mesmo diálogo em 304 ms. `tmux.send_keys` usa esse formato no Windows para os dois
nomes da tecla, preservando o caminho POSIX e a confirmação de fechamento do `/btw`.

A causa é documentada no [teste de regressão #588 do psmux](https://github.com/psmux/psmux/blob/master/tests-rs/test_issue588_win32_input_escape.rs):
após receber uma sequência de teclado Win32, o ConPTY pode reter o ESC isolado esperando
continuação, em vez de entregá-lo como tecla. O formato completo funciona nos dois estados.

Na instalação normal do WinBoat (8765), após reiniciar o backend, o `/btw` enviado pela janela
nativa retornou 200 em 6.799 ms, exibiu a resposta e deixou o terminal sem o diálogo. Um envio
normal multilinha em seguida retornou 200 em 3.454 ms, exibiu as duas linhas pedidas com acentos
e voltou a `idle`. Tempos do diário da API, até os cabeçalhos; não são o tempo total da resposta.
Essa validação não determina a causa da falha intermitente anterior em `linha.prova`.

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

  **Ressalva de 17/09/2026, corrigida em 18/09/2026:** a conclusão "não é do `hangar-preview`"
  vale para o que foi medido aqui — janela **ocluída**, sessão gráfica desconectada. NÃO
  generalize para o mouse que não entrega em outras situações: no Linux, com a sessão gráfica
  ativa e o painel apenas fora da tela, a aba que NAVEGA para de compor quadro e o Chromium passa
  a engolir mousedown e keydown. Ali o CDP cru também não entrega (a leitura de 17/09, de que ele
  entregava, não se sustentou na remedição), mas a causa é outra: falta de quadro, não falta de
  sessão gráfica. Sintomas iguais, causas diferentes — usar esta entrada para descartar a hipótese
  do `preview_ctl` custou uma sessão inteira de investigação no fantasma errado. Ver
  [a entrada em frontend.md](frontend.md#o-clique-do-preview-some-depois-que-a-aba-escondida-navega).

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

## Buffers do psmux: são da SESSÃO, o `-b` é ignorado e o nome do `-F` é outro

(14/09/2026, psmux 3.3.8, Claude Code v2.1.270). O `/btw` no Windows respondia sempre
"o /btw respondeu, mas não consegui ler a resposta" (502 `erro_btw_ilegivel`), com a resposta
pronta no overlay e copiada. O `c` do overlay manda OSC 52 e o psmux grava o texto em buffer; a
leitura é que falhava em silêncio, por três diferenças do tmux que se somam:

- **Buffers são por sessão.** Sem `-t`, `list-buffers` fala com a sessão padrão de quem chama
  (a do `TMUX` do ambiente, ou outra quando não há). O backend não tem `TMUX`: quando a padrão era
  a do `/btw`, o buffer aparecia; quando não era, caía na leitura do pane e "funcionava" — por isso
  uma reprodução feita de dentro de uma sessão passa e o backend falha.
- **`list-buffers -F '#{buffer_name}'` devolve `buffer0000`**, enquanto a listagem padrão mostra
  o nome real `buffer0`. Um OSC 52 vira **dois** buffers iguais.
- **`show-buffer -b <nome>` devolve vazio com rc 0** e `delete-buffer -b <nome>` não apaga nada,
  com o nome do `-F` ou com o real. Sem `-b`, os dois agem no buffer mais recente.

No tmux os buffers são do servidor e `-t` é flag desconhecida nesses comandos. Por isso
`btw._buffer_cmd` tenta com `-t =<sessão>` e, se o código de retorno recusar, guarda a resposta e
segue sem `-t` — nunca pelo nome do sistema. A leitura tenta `-b` e, vazia, lê o mais recente (o
`_COPIA_LOCK` garante que é o do `c`); a limpeza apaga pelo nome e, enquanto sobrar buffer além
dos de antes, apaga o mais recente. Buffer vazio cai na leitura do pane em vez de virar erro.
Validado ao vivo: resposta de 4574 caracteres lida inteira do buffer, sem buffer sobrando.

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
- **Superseded on POSIX (2026-09-27).** The "always `-e` in tmux" half split the default account in
  two: terminal-opened Claude went through the wrapper, which since `51ea3f7d` unsets the variable,
  and read `~/.claude.json` (168 KB, hangar MCP, trusted folders, history). App and
  `hangar-send --new` sessions got `-e CLAUDE_CONFIG_DIR=~/.claude` and read
  `~/.claude/.claude.json` (96 KB, no hangar MCP). Now the default account never becomes `-e` in
  tmux either; the server leak is blocked by `exec env -u CLAUDE_CONFIG_DIR <command>`. Measured
  on an isolated tmux server started with `CLAUDE_CONFIG_DIR=/tmp/conta-vazada`: a pane without
  protection printed `[/tmp/conta-vazada]`, and with `env -u` it printed `[]` (under zsh and under
  fish). Only the hidden login shell keeps the explicit `-e` in tmux: it has no command to carry the
  `env -u`.

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

## Deleting a folder that git has written into: `shutil.rmtree` stops at the first read-only file

(`codex_contas.delete_account`, measured 2026-09-16 with Python 3.14.7 on Windows 11.)
  Deleting an additional Codex account failed every time with `PermissionError` errno 13 /
  **WinError 5**, logged as `conta.apagar.fim` → `codex_account_delete_failed`. The Codex CLI clones
  marketplaces into `<CODEX_HOME>/.tmp/marketplaces/.staging/marketplace-upgrade-*/` and never cleans
  them; git writes `.git/objects/pack/*.idx|.pack|.rev` **read-only** (48 such files in the real
  account). On Windows `os.unlink` refuses a read-only file; on POSIX only the directory's permission
  matters, so the same code passes there and the test suite never saw it. A copy of the account
  folder reproduced the exact error on the first `.idx`; no process was holding any file (the SQLite
  files opened with share mode `None`). The fix is `rmtree(..., onexc=...)` that clears the attribute
  and retries only on `PermissionError`; anything else still surfaces. Claude accounts are not
  affected: their `plugins/` is a symlink to the shared folder, so no git clone lives inside them.

## Envio pelo plugin no Windows: tempos e o que não funcionou na DELPHI-02

(01/10/2026, Claude Code 2.1.287, psmux, `main` local `a78cc533` levada por bundle sobre
`27ef48e2`.) Mensagens `responda só: ok N` por `POST /api/sessions/<nome>/input` no backend da
VM. O pedido e a linha `SEND` foram carimbados no relógio da VM, por um leitor do log a cada 5 ms,
porque o log não tem hora. O instante do prompt vem do `timestamp` no `.jsonl`.

| caso | caminho | pedido → `SEND` | pedido → prompt no transcript | HTTP `/input` |
|---|---|---|---|---|
| antes, `27ef48e2`, sessão do Hangar, 10 msgs | plugin | 383,5 ms | 124 ms | 519,5 ms |
| depois, sessão do Hangar (`--new --terminal`), 10 msgs | plugin, `modo=user` nas 10, inclusive a 1ª | 913 ms | 146 ms | 968 ms |
| depois, `claude` digitado num psmux aberto à mão, 5 msgs | teclas | 754 ms | 390 ms | 751 ms |

(medianas; todas as mensagens entraram uma vez só)

- A linha de base já ia pelo plugin, então a comparação é plugin antigo contra plugin novo. O
  prompt chega ao transcript praticamente no mesmo tempo (124 → 146 ms). O `/input` ficou ~450 ms
  mais lento porque, no `modo=user`, o backend espera o `/submitted` do plugin, e ele só sai quando
  o `$.prompt.submit` termina, ~750 ms depois de o prompt já estar no transcript. Contra as
  teclas, o plugin põe o prompt no transcript 2,7× mais rápido, mas responde o HTTP mais tarde.
- **`/whoami` não acha sessão aberta fora do Hangar no psmux.** Log:
  `plugin whoami pane=%1 sessao=None origem=tmux`. No psmux todo pane é `%1` (três sessões vivas,
  as três `%1`), e o `TMUX` é `/tmp/psmux-37148/default,55189,0`: o terceiro campo é `0`, não o
  id da sessão (`$92`). O `_sessao_do_tmux` procura `$0` e não acha. A sessão fica sem plugin e
  recebe pelas teclas.
- **O plugin foi copiado, não ligado por junção.** O `install-hangar-send.sh` chama
  `cmd //c mklink /J`; o Git Bash converte o `/J` solto em `J:/` (`cmd //c echo /J` imprime `J:/`),
  o `mklink` responde `Invalid switch` e o script cai no `cp -r`. Com `//J` a junção nasce
  (testado numa pasta temporária). O `install.ps1` descarta a saída do script quando ele sai 0, por
  isso o `-Update` não mostra o `COPIA do plugin`. O `claude plugin list` carrega `hangar@skills-dir`
  da cópia. Cada reinstalação regrava a pasta, e a sessão aberta recarrega o mod
  (`hooks.json changed — reloaded`).
- Segunda execução do `install.ps1 -Update`: termina com 0 e não cria `plugins\hangar\hangar`. Continua
  em `COPIA do plugin`, nunca em `ja linkado`.
- `~/.hangar/plugin.json` só tem `url` e `chave`; o bearer do `backend\.env` aparece nele 0 vezes. A
  outra conta da VM (`.claude-claude-200-5`) tem `skills` como symlink para `~\.claude\skills`.
- Reinício do backend no meio de uma resposta (`Restart-HangarTask`, porta de volta em 19,4 s): a
  mensagem enviada 1,7 s depois da volta da porta foi pelas teclas e entrou uma vez. A seguinte, ~25 s
  depois, já foi pelo plugin, `modo=user`. A mensagem longa enviada logo depois da segunda
  reinstalação, que reinicia o backend e regrava a cópia, também foi pelas teclas.

## Identidade da sessão no psmux, e o envio pelo plugin medido de novo na DELPHI-02

(01/10/2026, psmux 3.3.8, Claude Code 2.1.287, `main` local `d09d030d` levada por bundle sobre
`27ef48e2`.)

- **Cada sessão do psmux tem servidor próprio.** De dentro do pane, `TMUX` é
  `/tmp/psmux-<pid>/default,<porta>,0`: o número do caminho é o pid do servidor DAQUELA sessão (pai
  do shell), o segundo campo é a porta TCP de loopback dele e o terceiro é sempre `0`, nunca o id
  da sessão. O pane é sempre `%1`, em todas as sessões. `list-sessions -F '#{pid} #{session_name}'`
  liga pid a sessão, e o pid não muda com `rename-session` (as variáveis `PSMUX_SESSION*` ficam com
  o nome antigo). `#{socket_path}` é `~\.psmux/default` para todas, nunca igual ao caminho do `TMUX`.
  No Linux todo `#{pid}` é o mesmo servidor; o prefixo `psmux-` separa os dois casos.
- `/whoami` com três sessões psmux vivas (pids 24036, 10316, 24828), `claude` digitado num pane aberto
  à mão (`cx-plugin-d`): `plugin whoami pane=%1 sessao=cx-plugin-d origem=psmux-pid`. Na rodada
  anterior era `sessao=None`.

| caso | caminho | HTTP `/input` (mediana) | 1ª mensagem |
|---|---|---|---|
| antes, `27ef48e2`, sessão do Hangar, 10 msgs | plugin | 519,5 ms | — |
| rodada anterior, sessão do Hangar, 10 msgs | plugin, `modo=user` | 968 ms | 963 ms |
| agora, `claude` aberto à mão no psmux (`cx-plugin-d`), 5 msgs | plugin, `modo=user` nas 5 | 502 ms | 1016 ms |
| agora, sessão do Hangar (`cx-plugin-e`, `--new --terminal`), 10 msgs | plugin, `modo=user` nas 10 | 488 ms | 1031 ms |

(cada prompt contado no `.jsonl`: uma entrada só, em todos)

- O `/input` voltou ao tempo da linha de base porque a prova pelo transcript responde antes do
  `/submitted` do plugin. Só a 1ª mensagem de cada sessão passa de 1 s.
- **Junção:** a primeira `install.ps1 -Update` criou `~\.claude\skills\hangar` como `Junction`
  (`claude plugin list`: `hangar@skills-dir` carregado), mas 65 ms depois apareceu uma cópia em
  `plugins\hangar\hangar` (22 arquivos, sem link): o `mklink //J` criou a junção e o script ainda
  caiu no `cp -r`, que copiou para dentro dela. Não deu para ver a saída do script (o `install.ps1`
  a descarta quando sai 0). A segunda execução saiu 0, manteve a `Junction`, não aninhou de novo,
  mas recriou a junção (a data de criação mudou), então o ramo `ja linkado` não casa.
- Reinício do backend no meio de uma resposta longa (`Restart-HangarTask`, porta de volta em 19,2 s):
  a mensagem seguinte, enviada ~1,7 s depois da volta da porta, já foi pelo plugin (`modo=user`,
  HTTP 689 ms) e entrou uma vez; a longa também ficou com uma entrada só.

## Troca do app nativo com o exe anterior preso

Medido em 02/10/2026, Windows 11, app nativo 0.1.0.3981. O botão de atualizar do app respondia
"Não consegui trocar o app: Acesso negado. (os error 5). Nada foi trocado." em toda tentativa.

- O app estava aberto quando o `install-native.ps1` rodou (16 s depois da abertura): ele renomeou
  o exe em uso para `Hangar.exe.old` e pôs a versão nova no lugar. O processo aberto passou a
  rodar a imagem `.old`.
- Na troca do app, `remove_file(.old)` falhava calado e o `rename(Hangar.exe -> .old)` por cima
  da imagem viva devolvia o erro 5. Com o processo vivo, abrir o `.old` para escrita dava "em
  uso" e o `Hangar.exe` estava livre; `Hangar.exe`, o `.new` baixado e o manifesto da release
  tinham o mesmo sha256. Ou seja, não havia o que trocar, só reabrir.
- Não era permissão: o app, sem elevação, criou o `.new` na mesma pasta.
- Cada tentativa deixava um `Hangar.exe.new` de 94 MB ao lado do app.
- O `install-native.ps1` tinha a mesma sequência (`Remove-Item` calado + `Rename-Item`), que
  estoura na segunda execução com o app aberto.

O nome livre para o exe anterior entrou em `b9650562`/`eaf66071`. Reabrir sem baixar quando o
disco já tem a versão, apagar o `.new` na falha e a mesma regra de nome no instalador vieram
depois. No Linux nada disso acontece: o sistema deixa substituir o arquivo de um executável em uso.

## O psmux entende `capture-pane -e`

(05/10/2026, DELPHI-02, psmux da instalação, Claude Code 2.1.289, Hangar da branch `hangar-server-parte1`.)
Sessão `Crack` nova com terminal: a primeira mensagem ficou na fila e o `hangar-server.log` gravou 35
`composer_busy` em 54 s, um por tentativa. O composer estava vazio. Mostrava só a sugestão
esmaecida `❯ Try "fix typecheck errors"`. O escritor Rust lia a tela sem `-e` no Windows, então a
sugestão contava como rascunho do dono; `C-u` não a apaga, e o envio era adiado para sempre.
`capture-pane -p -e -t =Crack:0.0 -S -200` no mesmo pane devolveu SGR completo: a sugestão vem
como `❯\u00a0ESC[0;2mTry "fix typecheck errors"ESC[0m` (NBSP depois do `❯`), e as réguas e a
statusline vêm em truecolor `0;38;2;r;g;b`. O `unstyle` já trata os dois formatos.

Efeito em cadeia: o backend reiniciou (troca do canal de atualização) no meio de uma tentativa.
A entrega ficou incerta, e a trava de escrita segurou as duas mensagens seguintes com
`terminal_write_barrier`, como manda a regra da entrega incerta.

## Captura avulsa pela saída, não pelo código

(06/10/2026, parte 4 da migração para Rust, Task 7.) A lista no Windows já era do Rust e capturava
o pane por um processo do psmux, mas decidia pelo código de retorno (`rc != 0` virava
`capture_refused`) e trocava byte inválido por U+FFFD sem marcar, contra as duas regras acima.
`state/capture.rs` passou a ser a fonte única da captura avulsa: a lista usa em toda plataforma,
e o `Monitor` de estado no Windows, onde o `-C` continua desligado (`terminal_control.rs`). O
`has-session` só roda quando a captura é recusada ou vem vazia, porque cada processo custa
~25 ms no Windows.
O psmux honra o `=` exato no `has-session`. Os testes usam um multiplexador falso (`.cmd` no
Windows, `sh` no resto); o caminho Windows é conferido pelo job Windows do CI.

## Terminal real do dono no Windows

(06/10/2026, parte 4 da migração, Task 10.) O `term/pty.rs` usa o `portable-pty` 0.9 também no
Windows. Diferenças conferidas na fonte do crate e no `conpty.py`:

- O `take_writer` do Unix tem um `Drop` que escreve `"\n"` + Ctrl-D no PTY (por isso o Unix usa
  `dup` do mestre); o do Windows devolve o `FileDescriptor` do pipe de entrada, e soltá-lo só faz
  `CloseHandle`. Teste: `term::conpty::dropping_writer_writes_nothing`.
- O `Drop` do mestre chama `ClosePseudoConsole`, que pode travar com o cliente vivo
  (microsoft/terminal#17716). O `close` mata o filho (`TerminateProcess`, código 1), espera até
  3 s e só então solta o mestre numa thread de bloqueio, com prazo de 5 s (`pty_close_timeout`);
  filho que não saiu vaza o conhost com `client_not_reaped`, como o `conpty.py`. Teste:
  `term::conpty::child_killed_before_close` exige código 1, não o 0xC000013A do CTRL_CLOSE.
  Os testes `term::conpty::*` só rodam no job Windows do CI.
- Sem `detach-client` e sem `@hangar_term_size`: o psmux não tem identidade de cliente e
  `resize-window`/`setw` voltam 0 sem efeito (medido em 22/08/2026 para o `termsock`).
- O crate cria o ConPTY com `INHERIT_CURSOR | RESIZE_QUIRK | WIN32_INPUT_MODE` (o Python usa 0).
  Com `INHERIT_CURSOR` o console pede a posição do cursor (`ESC[6n`) e espera a resposta do
  cliente; a prova com web e nativo na DELPHI-02 é o Step 23 do plano da parte 4.
- Tecla escrita antes de o cliente psmux subir se perde (06/10/2026, DELPHI-02, psmux da VM,
  sessão de teste com PowerShell). Pelo Rust (WS na 8765, cliente respondendo o `ESC[6n`), os
  bytes chegam assim: `ESC[6n` em 70 ms, `ESC[?9001h ESC[?1004h` e o título do conhost logo
  depois, e só então o próprio psmux (`OSC 4;n;?`, `ESC[?996n`, `ESC[?1049h`, pintura) entre 75 e
  80 ms. `echo` mandado em 0, 40 ou 60 ms, junto com a resposta do cursor, ou ao ver o
  `ESC[?9001h` ou o título: perdido em todas as tentativas. Mandado ao ver o primeiro `OSC 4` ou o
  `ESC[?1049h`, ou a partir de 100 ms: chegou em todas. O ConPTY do `conpty.py` (flags 0, sem
  backend) tem o mesmo defeito, só que a janela é de ~10 ms porque não há pedido de cursor: perdeu
  em 0 ms e chegou de 50 ms em diante. No Linux (tmux 3.7b, `-L` próprio) o `echo` escrito antes
  até do `exec` chega: o tty guarda; por isso a porta só liga no Windows. macOS não conferido.
  O `ESC[?1049h` atravessa o ConPTY também vindo de um `prompt $E[?1049h` do `cmd.exe`, que é o
  teste `term::conpty::held_input_reaches_the_child_after_ready`. A primeira tecla do teste troca
  o prompt por um sem `?1049h`: repetido a cada prompt, ele limpa a tela alternativa e apaga a
  saída do comando antes de o ConPTY pintar o quadro (06/10/2026, DELPHI-02: sem portão, `echo`
  mandado 1,5 s depois do pronto mostrou o eco da linha e nunca a saída; com o portão a tecla saiu
  ~15 ms após o `?1049h`, com o mesmo resultado). Foi a falha do CI run 37489975761.

## Clique de mod no psmux

Medido no psmux 3.3.8 com o Claude Code 2.1.289 (fase 3 dos mods). Quem clica é o `hangar-server`,
pelo mesmo executor do terminal do resto da sessão Claude no Windows. A geometria das abas e do `✕` e o
clique SGR são os do tmux; o que muda:

- `#{mouse_sgr_flag}` e os outros `mouse_*` voltam vazios: o sinal de mouse ligado é `#{alternate_on}`.
  Valor vazio de formato nunca é lido como 0.
- `capture-pane -e` escreve o SGR com o estado absoluto (`ESC[0;1;7;48;2;…m`): a aba ativa e a cor da
  borda são lidas parâmetro a parâmetro.
- A roda rola três linhas por evento espaçado e acelera muito em rajada: o clique acompanha o `offset` do
  `ui.scroll`, nunca uma conta de linhas.
- O controle do terminal pelo Rust (`terminal_control.rs`) é recusado no Windows, e com ele o observador
  da prévia e o vigia de tamanho: o psmux não avisa a mudança de tamanho feita por um cliente, e um
  cliente de controle do Hangar entraria no `#{session_attached}` com que o clique decide se há terminal
  de verdade ligado. Por isso a contagem de clientes usa só o `#{session_attached}` do psmux (0 ou 1; a
  tela do clique lê `display-message -p -t <pane> '#{session_attached}'`, e vazio nunca vale 0), e não
  o `list-clients` sem `control-mode` do tmux. Quem fizer um desses recursos rodar no Windows precisa
  trocar essa contagem. `detach-client` sem alvo derrubou outro cliente: nunca usar.
- Sem vigia de tamanho: o mínimo (144 colunas por 40 linhas) volta no preparo da operação de mod
  seguinte, não na hora em que o terminal se desliga.
- Sem cliente ligado, `resize-window` funcionou (redesenho em 70 a 200 ms, aba mantida) e deixa a janela
  em `window-size manual`; o Hangar repõe `latest` em seguida. A nota de 22/08 em `termsock.py` diz que
  os dois não fazem nada: o clique do `hangar-server` confere o tamanho depois de redimensionar e, se ele
  não mudou, segue pela roda.
- A pesquisa de satisfação do fim do turno esconde a faixa e responde aos dígitos: conta como diálogo.
- Contagem de clientes, medida na prova da VM (06/10/2026, psmux 3.3.8):
  - o `list-clients -F` é ignorado e o psmux imprime o formato padrão (`/dev/pts/N: sessão: título
    [100x29] (utf8) [activity=…]`), com o cliente de controle marcado só como `(control mode)` no texto,
    sem `client_flags`. O ramo do tmux, que separa `control-mode` por vírgula, contaria o cliente de
    controle como terminal;
  - o `#{session_attached}` só vale 0 ou 1: um terminal dá 1, dois terminais dão 1, e o cliente de
    controle sozinho também daria 1;
  - o ramo atual (`#{session_attached}` comparado com 0) acerta porque o vigia e o observador da prévia
    não rodam no Windows; quem ligar um cliente de controle lá precisa de outra contagem.
- O `window-size latest` pega: depois do mínimo do Hangar (144x40) e de um clique esticado (144x250 de
  volta a 144x40), um terminal de 100x30 que se liga deixa a janela em 100x29. Com dois terminais a
  janela segue o último que se ligou e, quando ele sai, fica no tamanho dele. `#{window_size}` sai vazio.
- Cada operação do clique é um processo do psmux, e cada passo do anel do `ctrl+x tab` custava 0,38 a
  0,44 s com a conferência do pane e a releitura do tamanho (seis processos). Dentro da reserva do pane o
  executor confere o pane uma vez, e o anel faz a tecla e uma leitura por passo; a volta ao prompt da
  limpeza tem prazo pelo tamanho do anel (até 9 s).

## Porta pública dividida com outro processo (13/10/2026)

Numa máquina Windows a atualização parava em "Porta 8765 ocupada por outro processo": uma sessão
Claude de outro projeto tinha subido `python -m http.server 8765`, que escutava em `[::]` enquanto o
`hangar-server` escutava em `0.0.0.0`. Medido no Windows 10 22H2, com um socket segurando a porta e
outro tentando abri-la (`10048` = em uso, `10013` = acesso negado):

| Hangar segura | só IPv6 | dual-stack | dual + `SO_REUSEADDR` (`http.server`) | `0.0.0.0` |
|---|---|---|---|---|
| `0.0.0.0` | abre | abre | abre | 10048 |
| `[::]` dual-stack | abre | 10048 | 10013 | abre |
| `0.0.0.0` + `[::]` só IPv6 | 10048 | abre | abre | 10048 |
| `0.0.0.0` + `[::]` dual-stack | abre | 10048 | 10013 | 10048 |

`SO_EXCLUSIVEADDRUSE` não mudou nenhuma linha. Trocar `0.0.0.0` por `[::]` dual-stack sozinho abriria
a porta para quem pede só IPv4, então ficaram os dois sockets. No Linux o `0.0.0.0` já recusa o
dual-stack de outro processo, e o segundo socket colidiria com o primeiro: a guarda é só do Windows.
No socket dual-stack o cliente IPv4 chega como `::ffff:a.b.c.d`; o `ipv4_peer` em `routes.rs` devolve o
IPv4 antes das rotas, que decidem "local" e proxy confiável por ele.

## Log do backend preso por neto (20/10/2026)

Dois usuários do Windows ficaram com o backend fora do ar depois de um reinício (auto-update das
07:55 num, das 20:24 no outro). A tarefa `hangar-backend` rodava
`cmd /c "python -m app.main > hangar-backend.log 2>&1"` e cada tentativa saía com código 1 sem
escrever nada. O `cmd` não abria `%LOCALAPPDATA%\hangar\logs\privado\hangar-backend.log` ("está
sendo usado por outro processo"): o Restart Manager mostrou quem segurava a cópia herdada do
backend morto — dois `jest` travados, cinco `kubectl port-forward`, um `ts-node-dev` e um `next dev`
numa máquina; um Edge headless de teste (perfil temporário) na outra. Todos nascidos de sessões.
O backend subia normalmente rodado à mão, fora da tarefa.

A tarefa agora roda `python -m app.main --log <arquivo>` sem o `cmd`, e o `app.main` abre o log com o
`open` do Python, que no Windows compartilha leitura e escrita: a cópia herdada por um neto não
impede o próximo `open`. Os descritores 1 e 2 vão para o mesmo arquivo, então a saída do
hangar-server e dos outros filhos continua lá. Preso mesmo assim (o handle de um `cmd` antigo, na
primeira subida depois da atualização), a saída vai para `hangar-backend.1.log` … `.5.log`; sem
nenhum, o backend sobe sem log e o diário registra `backend.log_indisponivel`. O log abre antes dos
imports do `app.main`, para um erro de importação ou de configuração na subida também cair nele (o
`>` do cmd capturava isso). O instalador acompanha a escrita mais recente entre `hangar-backend*.log`
(`Get-LogBackend`). `test_main_log.py` prova as duas coisas no Windows: um segundo
`open` com o primeiro aberto, e o nome alternativo diante de um handle sem `FILE_SHARE_WRITE`.

## Python do Install Manager (22/10/2026)

Numa máquina com o Python 3.14 instalado pelo winget, o passo 7b do `install.ps1` dizia "nenhum
Python real encontrado" logo depois de o 1/8 dizer "ok Python", e o `hangar-conta`, o
`hangar-engine` e os lançadores do Codex não eram criados. O winget instala o 3.14 pelo Python
Install Manager (pacote MSIX `PythonSoftwareFoundation.PythonManager`): `py.exe` e `python.exe`
são aliases em `%LOCALAPPDATA%\Microsoft\WindowsApps`, os atalhos globais ficam em
`%LOCALAPPDATA%\Python\bin` (fora do PATH nessa máquina) e o interpretador em
`%LOCALAPPDATA%\Python\pythoncore-3.14-64\python.exe`. O 7b descartava todo candidato em
`WindowsApps` como atalho da Store.

Agora, sem candidato fora de `WindowsApps`, o 7b tenta `%LOCALAPPDATA%\Python\bin\python.exe` e os
atalhos achados, roda cada um com `-c "import sys; print(sys.executable)"` e usa o caminho
devolvido. O atalho da Store não devolve caminho e segue recusado. Medido na máquina afetada: o
`py` do Install Manager respondeu `...\pythoncore-3.14-64\python.exe`, o `-Update` criou os
lançadores e o `python3` do bash de login passou a apontar para esse interpretador.

## `python3.cmd` ao lado do atalho do bash (23/10/2026)

Na DELPHI-02 o Windows abria a cada pouco o diálogo "Como deseja abrir este arquivo?" para
`~/.local/bin/python3`. Os lançadores `.cmd` põem `%USERPROFILE%\.local\bin` na frente do PATH e o
repassam aos filhos, e os hooks do Claude e do Codex chamam `python3 <hook>.py`. Com esse PATH,
tanto o `Get-Command python3` quanto o `where python3` resolviam para o atalho sem extensão, antes
do `Python314\python3.exe` de uma pasta posterior. O Windows não executa arquivo sem
extensão: abre pelo programa associado. O `hangar-send` não sofria disso porque o `hangar-send.cmd`
mora na mesma pasta, e na mesma pasta os dois shells preferem a extensão do PATHEXT.

Agora o instalador grava `python3.cmd` junto do atalho, com o mesmo interpretador. Medido numa
pasta temporária com os dois arquivos na frente do PATH: PowerShell e `cmd` resolveram e rodaram o
`python3.cmd` (entrada padrão incluída), e o bash do Git seguiu no atalho `sh`.
