# Polish & feature backlog

Captured from live testing (phone, real session). Deferred by the user to the polish
phase — not blockers. Newest first.

## Claude Code shipped native cross-session messaging (2026-08-07)

Anthropic released it the same day, in **v2.1.224**: two tools Claude drives itself — `ListAgents`
to discover reachable sessions, `SendMessage` to deliver text to one by name — plus `/list-agents`
(alias `/peers`) and a `Peer address` row in `/status`. Delivery is a **per-session unix socket**
registered in files on disk; the path is exported as `CLAUDE_CODE_MESSAGING_SOCKET` to hooks and
Bash *before any hook runs*. Docs: <https://code.claude.com/docs/en/cross-session-messaging>.

It overlaps `cp-send`'s core and does that part **better**: no terminal typing at all, so the whole
bug class measured above (16344-byte tmux ceiling, `\n` submitting a line, 2N−1 calls line-by-line)
simply does not exist there. It also rate-limits and drops identical repeats, so an A↔B loop stops
on its own.

**The plan is to route through it when it exists and keep everything else — not to remove what is
here.** What stays ours, because the native feature has no equivalent:

| | native | `cp-send` |
|---|---|---|
| starting an exchange with another machine | no — replies only, via Anthropic servers + Remote Control | yes, direct over the mesh (`peers.json`), no cloud |
| Codex / Pi sessions | no (Claude Code only) | yes |
| creating a session (`--new`, `--engine`) | no | yes |
| pairing + shared contract + PairSheet | no | yes |
| group broadcast (`--group`) | no | yes |
| phone UI | no | yes |

Two things measured here that decide the shape of the integration:

- **`cp-send` must NOT write into the peer's socket.** Claude Code only delivers without approval a
  message it can *verify* came from a child process of **that same session** (a hook or Bash posting
  to its own socket). A script posting to someone else's socket asserts no permission class, and a
  session that bypasses permission prompts — which is how these sessions run — **holds it for
  approval**, in a dialog that expires in 5 minutes. So the native path is the paired Claude using
  **its own `SendMessage` tool**; that is a change to the protocol text in the heredoc of
  `scripts/install-cp-send.sh`, not backend code.
- **What breaks is the pair conversation in the UI.** `PairSheet` mines nothing the backend routed:
  it fetches each member's `getHistory` and matches `user_msg` whose text starts with `[de: X]`
  (`PairSheet.svelte:67` → `parsePeerMessage`, `lib/format.ts:205`). A native message lands in the
  transcript with *its own* sender labeling, so the group timeline would go quiet with no error.

Also worth carrying into any design: a delivered message counts toward usage like a typed prompt,
and it arrives explicitly labeled *not from the user* — it cannot approve a permission prompt or
change configuration. That is right for pair traffic and **wrong for the app's main path**: a
message typed on the phone is the user, and routing it through the peer socket would strip it of
exactly that. The PTY item above stays the answer for the user's own text.

### Shipped the same day — what is live, and what the measurement showed

The rollout reached this machine hours after the entry above was written, so the two items were
measured on real traffic and built. See `transcript.py` (`_peer_msg`), `registry.inbox_socket_of` /
`name_of_pid`, `GET /api/sessions/{name}/peer-address`, and the guard in `scripts/cp-send`.

- **A peer message arrives in two different shapes, and only one of them was obvious.** Target
  **idle** → `queue-operation enqueue` + `dequeue` + a `type: "user"` entry carrying a structured
  `origin` (`kind:"peer"`, `from:"uds:.../cc-socks/<pid>.sock"`, `verifiedPeerPid`, `name`,
  `fromMode`, `body`). Target **mid-turn** → `enqueue` + **`remove`**, and **no `type: user` entry at
  all**: the harness consumes it between two tool calls, without interrupting the running tool. The
  `remove` shape carries only the wrapped text, no `origin`. Both paths had to be handled — the
  second one is not defensive coding, it is what happens whenever a peer writes to a session that is
  working, which for a paired session is most of the time.
- **`message.content` is not displayable** — it carries the `<cross-session-message>` wrapper plus a
  full paragraph instructing the receiver about permission laundering. `origin.body` is the clean
  text; for the `remove` shape it has to be pulled out of the wrapper.
- **`origin.name` is the session's TITLE, not its name** ("Revisar novo modo de envio no backlog"
  where the tmux session is `claude-cockpit`), so it does not match anything the app addresses by.
  `verifiedPeerPid` → pane → tmux name is what makes the group feed and the badges work.
- The whole integration is therefore **one branch in the parser** that normalizes a native message
  into the exact shape `cp-send` already produces (`[de: <session>] body`). Nothing in the front
  changed.
- **The routing rule is enforced in code, not only in the protocol text.** `cp-send` refuses a local
  1:1 (exit 3) only when the native path provably reaches *both* ends — sender has
  `CLAUDE_CODE_MESSAGING_SOCKET`, target's `/peer-address` returns a socket — and `--tmux` forces the
  old path. Checking the **fact** rather than the session type is what makes Pi and Codex fall out on
  their own: measured here, both Pi sessions return no socket while all three Claude ones do. Text in
  `CLAUDE.md` alone would not have been enough — an already-open session never re-reads it, but it
  obeys the script immediately.

## An opt-in headless session mode — no tmux, real streaming (idea, 2026-08-08)

Owner's idea, and it reframes what "headless is out" meant. The objection to `claude -p` was always
that owning the process costs the terminal: no `tmux attach` from kitty, no session living outside
the app. **For someone who never wants a terminal, that is not a cost.** And on Windows, where tmux
does not exist and psmux only partly stands in, it removes the dependency entirely.

Shape: the default stays exactly what is here and tested (TUI in tmux, the app as an extension of
the terminal). Headless is a **second mode**, chosen when the session is created or via a setting.
Precedent for the shape already exists in `adapters/codex`, which consumes structured events instead
of scraping a pane.

The mode is **not** one-shot `-p`. It is the long-lived streaming-input process:
`claude --input-format stream-json --output-format stream-json --verbose
--include-partial-messages`, fed one JSON message per line on stdin.

**Measured 2026-08-08, and the two hardest questions came back positive:**
- **One process holds a real multi-turn session.** Sent "guarde este número: 7391", then a second
  message asking for it back on the same stdin: it answered `7391`, the process stayed alive across
  both turns, and both turns share one `session_id`. Not a sequence of one-shots — it is a session.
- **Token-level streaming works**: `content_block_delta` / `text_delta` events arrived while the
  answer was being written. The preview stops being a scrape and becomes an event feed — the gap
  against Pi closes for these sessions, and this is the one thing no pipe over a TUI can give
  (see the item below).
- **The transcript is the same format.** `transcript.py::parse_line` read the long-lived session's
  `.jsonl` (129 KB, under `~/.claude/projects/`) with **no changes at all**: two `user_msg`, two
  `assistant_msg`, in order. The whole chat surface — history, windowing, dedup, bubbles — would
  work untouched. Same result for a one-shot `-p` transcript.
- **Nothing is lost.** Measured by starting the long-lived process in this repo's cwd and reading its
  init event: **226 slash commands/skills**, **94 tools**, **74 subagents**, **4 MCP servers** (3
  connected — `codegraph` fails here anyway), `permissionMode: bypassPermissions` inherited from
  settings, and the account's default model. CLAUDE.md loads: asked, it answered in pt-BR citing the
  identity rule, and named `superpowers:brainstorming` and `my-org:kubectl` correctly. **Hooks fire
  the same**: the transcript carries `SessionStart:startup` (including the caveman and ponytail
  plugin hooks), `UserPromptSubmit` injecting context, and 10 `Stop` hooks. It is the same context
  assembly as an interactive session, not a reduced one.
- **Usage comes out of the same window — today.** Anthropic's help centre, in its own words: *"We're
  pausing the changes to Claude Agent SDK usage described below. For now, nothing has changed: Claude
  Agent SDK, `claude -p`, and third-party app usage still draw from your subscription's usage
  limits."* The same article separates the cases under *What stays the same*: *"Using Claude Code in
  the terminal or your IDE continues to use your subscription usage limits exactly as before."*
  (<https://support.claude.com/en/articles/15036540-use-the-claude-agent-sdk-with-your-claude-plan>).
  Matches what the machine shows: `apiKeySource: none`, authenticating with the subscription's OAuth,
  no API key anywhere.
  **The caveat is real**: this is *paused*, not cancelled. The change was announced with firm numbers
  (\$20 Pro, \$100 Max 5x, \$200 Max 20x) and suspended on the day it would have taken effect, with
  only *"When we have an update, we'll share it before anything takes effect"* — no date. Anything
  built on this is built on a policy that has already been announced once.

**Still open, in the order a spike should take them:**
1. **Permission prompts.** Approving a tool call from the phone is the app's core value. There is no
   TUI dialog to read here; permissions surface as a callback / `--permission-mode`. Can the app
   render and answer them? If not, the mode only serves `bypassPermissions`, which narrows it
   sharply. **This is the one that decides the feature.**
2. **Attaching to an existing session id** — resuming a session started elsewhere, and what happens
   if two writers reach the same session.
3. **Slash commands.** `/model`, `/clear`, `/compact`, `/usage` are TUI-level. Lost, or
   re-implemented per command? `model_picker.py` exists precisely because there was no other way.
4. **Statusline** (context, cost, quota badges) comes from the pane or a sidecar today; here there is
   neither. The `result` event carries usage, which may be enough.
5. **What the Shell tab becomes** when there is no agent pane to attach to.
6. **Discovery has exactly one source of truth today: tmux.** `registry.list()` starts by asking tmux
   which panes exist and only then maps each to its `.jsonl`, so a headless session is invisible to
   the app — verified: the test session's transcript sits in `~/.claude/projects/` at 129 KB with
   both turns intact, while `/api/sessions` lists only the four that live in tmux. The backend would
   need a second source — a registry of the processes it owns (pid + session id + cwd) — and the
   listing becomes the union. That cascades: state comes from stream events instead of the pane,
   `kill` means killing a process rather than `tmux kill-session`, and there is no pane to attach.

Not a small change — but not speculative either: session lifetime, streaming and transcript
compatibility are answered, and answered well. What is left is mostly about the app's own surfaces.

## Windows: the send path has a fix, and it is not a buffer (measured 2026-08-08)

Measured on a real Windows box (psmux 3.3.7, build 05cc5d4 2026-07-20) against a real Claude Code
session, with the transcript as the oracle — never the pane. This closes the "Windows is unmeasured"
item and corrects a claim in `CLAUDE.md`.

**`paste-buffer` works. Our note saying it does not is wrong** — not outdated: it names this exact
version. Text pasted through it reached the Claude composer, with `send-keys` as a control in the
same run proving the instrument.

**Neither buffer command can carry a newline, and the reason is in psmux's source, not in a limit:**
- `set-buffer` truncates at the first newline — 22 bytes stored out of 13799, and **rc=0**. The
  control protocol is line-terminated (`cmd.push('\n')`) and the client does not escape, so the
  command ends at the first newline.
- `load-buffer` escapes it and nothing ever unescapes: `main.rs` does
  `content.replace('\n', "\\n").replace('\r', "\\r")` before forwarding as `set-buffer`, and the
  server's handler (`server/connection.rs`) stores `content_parts.join(" ")` verbatim. The escape
  has no matching unescape — a genuine psmux bug. `-r` and `-s` change nothing, because the newline
  is already gone by the time paste runs. CR is escaped the same way.

So the buffer route delivers **100% of the content** (600/600 lines, no truncation) and destroys
every separator. Worth knowing, since the path in use today loses far more than that.

**The fix is the clipboard, and it is better than the Linux one.** Writing the text to the Windows
clipboard and sending **`M-v`** (Alt+V — `Ctrl+V` is swallowed by the terminal on Windows, which is
why Claude Code binds Alt+V there, as the owner knew and the probe confirmed) delivered **600/600
lines with real newlines and no truncation**, verified in the transcript: 14445 bytes, one copy. The
receiving Claude tabulated both sends itself: same 600 lines, same range, `\n` literal on the buffer
route against a real newline on this one.

Why it is better than what Linux does: **no content byte passes through the multiplexer at all**, so
neither psmux bug matters. It is the same mechanism already measured for image paste on Linux, where
a bare `send-keys C-v` puts `[Image #1]` in the composer because the TUI reads the system clipboard
itself.

**It survives a locked workstation**, which was the go/no-go — the whole point of this app is driving
a session from the phone while the machine sits locked, and `send-keys` has no desktop dependency
while the clipboard does. Measured with the test proving its own premise (`LogonUI.exe` exists only
while locked, so the script waited for it and recorded the state at both ends):
`TRANCADA=True | Set OK | leu: MARCADOR-TRANCADO-789 | composer tem o marcador: True | ainda trancada
no fim: True`. Locking switches the *desktop* inside the same window station; the clipboard belongs
to the station, so it stays reachable.

**And the backend can reach it**: on Windows it runs as a scheduled task with
`New-ScheduledTaskTrigger -AtLogOn -User $env:USERNAME` and no S4U principal (`install.ps1:635`),
which means it runs *inside the user's interactive session* — same station, same clipboard. Derived
from the installer, not assumed; still worth one confirmation from a task-launched process before
writing code.

**The safety net already exists in the codebase, and it is the part not to skip.** A collapsed paste
renders as `[Pasted text #N]` with an incrementing `N`, and `terminal_input.py` already photographs
those ids before sending (`_paste_ids` / `pastes_antes`) and requires a *new* one afterwards — that
is precisely the evidence that was switched off when a probe in this session passed
`pastes_antes=None` and produced a false negative. Applied here the rule is: snapshot the
placeholders, write the clipboard, send `M-v`, and **send Enter only if a new placeholder (or the
text) appeared**. No evidence, no submit. If Claude Code ever starts treating a programmatically-set
clipboard differently from a user's Ctrl+C, this fails loudly instead of quietly sending nothing,
which is the exact failure mode this branch spent its day removing.

Two costs to carry into any design: it clobbers whatever the user had copied, and `Alt+V` is a Claude
Code binding — Pi and Codex panes need their own answer.

### Built on 2026-08-08, and what the five measurements settled

Shipped in `tmux.paste_via_clipboard` + the Windows branch of `send_prompt`. Full numbers in
[`docs/medicoes-2026-08-08-windows.md`](medicoes-2026-08-08-windows.md); what the code took from them:

- **The scheduled-task backend does reach the clipboard the TUI reads.** This was the go/no-go the
  section above left open, and it is now confirmed from a session created by the app itself, not
  inferred from the installer.
- **`Set-Clipboard` over stdin, not `clip.exe`.** `clip.exe` handles every encoding and even survives
  a missing BOM, but rewrites every LF as CRLF — the user's message must not change on the way. And
  **never send a BOM**: it becomes literal content (first character read is U+FEFF), invisible in the
  terminal and inside what the session receives. Cost is flat in size: ~250 ms for 13.8k, 69k or 278k
  chars alike, of which ~157 ms is PowerShell's cold start.
- **The TUI discards `M-v` while it is working** — three false negatives before anyone noticed the
  spinner. Nothing reports an error, so the strict proof is what catches it.
- **The proof deadline is 4.0s, three times the measured peak.** Five 600-line pastes showed the
  placeholder at 676, 665, 955, 1341 and 975 ms; the neighbouring 1.0s would have failed three times
  out of five, and a fixed 0.5s settle every time. The time **grows with the number of pastes in the
  same session**, so on timeout the code fails loudly rather than pressing Enter blind.
- **One `M-v`, never two.** After the first paste the footer reads `paste again to expand`, so a
  second key expands the block instead of re-pasting it.
- **Empty text is refused before PowerShell runs**: `Set-Clipboard` binds an empty string as null and
  returns rc=1, and on any failed write the clipboard still holds the *previous* message — pasting
  anyway would submit that whole message as if it were this one.

**There is no fallback to the old path**, deliberately, which supersedes the "fall back" line above:
that path is the one measured at 309 of 600 lines returning success, so falling back would restore
the exact silent loss. A failure becomes `partial`, which clears the composer and lets the queue
requeue.

**Two risks accepted, and the first is not the one it looks like.** (1) If the user copies something
between our `Set-Clipboard` and the `M-v`, nothing can distinguish their content from ours — the
placeholder proves *something* was pasted, never *what*. The consequence is **injection**, not
overwriting: whatever they copied becomes a user turn in a session that may be running in bypass. The
module lock closes this window between our own sessions and cannot close it against the user's hands.
(2) We clobber whatever they had copied.

**Bug found while verifying, same family, and it cost the rest of the afternoon** (diagnosed
2026-08-08): `install.ps1 -Update` prints an `X` for step 4/8 (`npm ci falhou (exit -4048)`) and then
still prints **`Pronto`** at the end. It is not cosmetic — it is the whole failure:

- **The installer sabotages itself.** It runs `npm ci` while the front it is about to replace is
  *still running*. `npm ci` wipes `node_modules` before reinstalling, so it deleted almost everything
  and then hit `EPERM: operation not permitted, unlink …\@rolldown\binding-win32-x64-msvc\
  rolldown-binding.win32-x64-msvc.node` — a **native `.node`** mapped into the live Vite process, and
  Windows does not let you delete a binary a running process has mapped. It aborted there, leaving
  `node_modules` half-installed and **without `.bin` at all**.
- **Nobody saw it**, because Vite kept running from its in-memory image. The damage only surfaced
  when the VM was suspended and the process died: nothing could restart it.
- **That is what the `502` was.** `tailscale serve` maps `/` to `127.0.0.1:5173`, so the chain is
  `ts.net → Vite → backend`. The backend was never down — it was the same process throughout — but a
  dead front takes every external route with it, `cp-send` included.
- **Fixed by a clean `npm ci`** once nothing held the binary (450 packages, exit 0, `.bin\vite.cmd`
  back). One warning left behind: `esbuild@0.28.1` has a postinstall not approved by allow-scripts.

**And the logon trigger does not heal this.** The front task (`claude-cockpit-frontend`, running
`vite preview` over `frontend/dist`) has an `MSFT_TaskLogonTrigger` with principal Interactive — it
fires **on logon**, so when suspension kills the process without a logoff/logon cycle, nothing brings
it back. Which is exactly what happened: the backend survived, the front did not, and the box sat
serving 502 until someone logged in. Note the asymmetry before "fixing" it with a service principal:
the interactive session is what gives the backend the clipboard, i.e. the Windows send path built
above.

**Both fixed and verified on the Windows box** (`641155b`…`cbc4ed9`): the front is stopped before
`npm ci`, and `-Update` refuses to print `Pronto` when any step failed — it prints what is missing and
exits 1.

Getting there cost three defects of my own, all found by the pair running it on the real machine, and
each is worth keeping:

- **A function declared inside an `if`.** PowerShell has no hoisting, so `Pare-Servico` did not exist
  on the common `-Update` — the one where the pull did not touch `frontend/` — and the call in step 7
  threw `CommandNotFound` with the backend task registered and never restarted. The `catch` there does
  not add to `pendencias`, so that run would still have ended in `Pronto`.
- **`$Nome:` inside a double-quoted string.** A dollar sign followed by a colon reads as a scope
  qualifier (`$env:PATH`), and PowerShell parses the whole file before running line one — so this took
  down `install.ps1` entirely. Since `-Update` fires from the post-merge hook, a pull would have left
  the machine with an installer that cannot run at all, and the hook only prints a warning.
- **Killing bystanders.** `Pare-Servico` matched any process whose command line merely *mentioned* the
  checkout path, so a `Start-Sleep` with that path in a comment died. Worse: edit a file under
  `frontend/` and call the installer in the same command, and the shell's own command line carries the
  path — the shell was killed, and the installer, being its child, went with it (exit 255 right after
  `4/8 Frontend`, front already down, `npm ci` never run). Fixed in two layers, because they cover
  different victims: excluding the installer's whole **ancestor chain** saves the shell that launched
  it, and requiring the **executable name** (`uv|python`, `node|npm|vite`) saves the unrelated third
  party. The executable filter needs a field of its own — the existing `Exe` is the program to
  *launch* (`uv`, `npm`), while what holds the socket is `python.exe`/`node.exe`.

Measured afterwards, no process leak: 0 orphans before and after three consecutive `-Update` runs
(4 processes from this checkout each time, all inside the service tree). Two notes from that
measurement, both worth more than the result:

- **A dead ppid does not mean orphan here.** The scheduled task runs through `wscript`, which exits
  and leaves the service alive with a dead ancestor *by design* — that criterion would flag a healthy
  backend and front as orphans. The right question is whether the process belongs to the tree of
  whoever is listening on the ports right now.
- **Editing the same file twice does not force a rebuild.** The build stamp is the commit plus
  `git status --porcelain` of `frontend/`, so touching `app.css` again yields the same line and the
  second run skips the build — which would have produced a measurement of three runs where two did
  nothing.

Two smaller ones came with it: the kill counter asked `Get-Process` on the line right after
`Stop-Process`, before Windows had torn the process down, so a successful kill counted as zero and the
note never printed; and a WMI hiccup silently degraded the lineage guard back to "own pid only", which
now refuses to kill anything instead.

**Still open:** Pi and Codex on Windows keep the old path — `Alt+V` is Claude Code's binding and
nobody has measured theirs. And the `cp-send`/`input` channel toward Windows was caught mutilating
text in passing (a quote died as `unexpected EOF while looking for matching quote`, and a test string
arrived stripped of accents and emoji): unrelated to this fix, unmeasured, and its own item.

## Reading the pane: what a PTY would and would not buy (measured 2026-08-08)

The PTY was cut as a *send* path (see the item above). It was then proposed twice more as a *read*
path, and both proposals were measured rather than argued. Numbers first, so nobody re-derives them.

**A PTY does not make the pre-Enter check droppable.** The claim was that writing bytes straight
into a PTY is reliable enough to stop reading the screen before pressing Enter. Measured against a
real Claude session, idle and working, with three payloads (short line, multi-line, 30 KB): PTY and
`send-keys`/`paste-buffer` are **identical** — same verdict, same 2 captures, same 0.16 s, zero false
successes on either. The reduction was an inference, and the inference was wrong.
(First run of that probe reported false successes on the 30 KB payload for *both* paths; that was a
defect in the probe — it passed `pastes_antes=None`, which is what switches off the collapsed-paste
evidence in `_composer_residuo`. With it captured as production does, both paths find the text.)

**A PTY reader costs more than `capture-pane`, exactly where it matters.** Per watched session:

| | session working | session idle |
|---|---|---|
| today, `capture-pane` | 6.7 reads/s × 2.0 ms = **1.33% of a core** | 1.33/s × 2.0 ms = **0.27%** |
| PTY + terminal emulator | 230 KB in 8 s → 276 ms of parse = **3.45%** | 440 B/s → negligible |

One `capture-pane` is 2.0 ms (median of 30, max 8.8). The preview loop polls every **150 ms** while
working — `preview.py` calls it "o poll mais caro que o backend tem" and it is right. So the PTY is
~2.6× more expensive precisely when the preview exists, and cheaper only when nobody needs it.

**And it would not improve the preview**, which was the strongest argument for it. `pyte` fed the
recorded stream reconstructs *the same rendered screen* `capture-pane` returns, so
`extract_assistant_text` would face the identical problem of separating prose from TUI drawing. Pi's
advantage is not the transport, it is the **semantics**: its extension receives `message_update` from
the agent and publishes the text block, not a rendering.

**The `.jsonl` is not a streaming source either.** Measured: the assistant text landed in the
transcript at t=9.5 s of a turn that ended at 9.6 s — the block is written when it **commits**. A
turn with tool calls does commit each text block before the next call, so mid-turn granularity
exists at block level; what is missing is only the block being typed right now, which is exactly what
the preview shows.

So the gap is structural, and no pipe closes it: PTY, `capture-pane` and the `.jsonl` are three ways
of seeing the same thing either too late or too rendered. What would close it is Claude Code
exposing the in-flight block the way Pi does. **What would flip the PTY verdict**: read frequency
climbing well past today's (polling under ~100 ms, or many watched sessions at once) — there the fork
cost overtakes continuous parsing. Not at 150 ms.

Attaching a second client is *not* a concern here — measured earlier that a PTY sized to the current
window disturbs nothing, and with `window-size latest` the client with recent activity wins.

Probes: `leitura.py`, `vazao.py`, `emul.py`, `jsonl.py`, written to the session scratchpad.

## Peers (the cross-machine mesh) have no UI at all (2026-08-07)

`backend/peers.json` is the only way to add, remove, enable or disable a remote machine. There is
no `/api/peers` route and `app/peers.py` only exposes `_load()` — nothing writes the file. So the
mesh that powers `cp-send <server>::<session>` is invisible in the app: a user who did not author
the project has no way to discover it exists, and the author has to remember the file path and the
JSON shape months later.

The two lists also collide by name, which is its own confusion: the account menu's **Servidores**
section (`+ Adicionar servidor`) is the list of *which backend this browser talks to* — client-side,
per device — while `peers.json` is *which machines this backend can relay messages to*. Same word,
different thing, and only one of them is visible.

What it needs: a settings screen listing the peers with an enable/disable toggle, reachability
status, and add/remove. Writing is the risky part — `peers.json` holds the tokens for the whole
mesh and `scripts/cp_panel_common.py:81` already notes a half-finished write would take the mesh
down — so the write path has to be atomic (temp file + rename), never a partial rewrite.

## Follow-ups from the real terminal panel (2026-08-07)

Left over from `feat/terminal-real` (WebSocket + PTY + xterm.js panel on the desktop). None of
these blocked the branch; they are the items a future plan should start from. The measurements are
here so nobody re-derives them.

### A second send path, through a throwaway PTY — closed, not built (2026-08-08)

Today every message the app sends goes through `tmux send-keys` — which sends **keystrokes**. The
terminal panel proved a different path exists: bytes written straight into a PTY master, delivered
by the kernel's tty layer, indistinguishable from a physical keyboard. That path handles what
`send-keys` handles badly: bracketed paste (a multi-line block arrives as *one* paste instead of N
lines the TUI may read as N submissions) and — the concrete pain that motivated this — `cp-send`
messages between Claude sessions arriving mangled.

**Image paste is NOT one of them, measured 07/08/2026** — this line used to claim it was, written as
a hypothesis and then read as fact, contradicting a code comment three files away. A bare
`tmux send-keys C-v` into a Claude Code session puts `[Image #1]` in the composer: the TUI reads the
machine's clipboard itself through `wl-paste` (which is why `tmux.py:292` (`new_session`) propagates
`WAYLAND_DISPLAY`), so the terminal only ever delivers the **keystroke** — no image bytes cross it on
any path. The PTY gains nothing here. Separately, the app's own attachments never touch the terminal
at all: an upload is saved to `<cwd>/.claude-pocket-uploads/` and the prompt carries the **path** as
text (`Composer.svelte:917`), which is also why a phone attachment works when the phone's clipboard
is not the machine's.

#### Measured (2026-08-07, tmux 3.7b, claude 2.1.220, this machine)

The assumption the whole idea rested on **holds**: a bracketed paste written into the PTY master
survives `tmux attach` and reaches the pane as a paste. Read from the receiving process's own
stdin, never from `capture-pane` — the pane is a rendering and lies about what arrived.

1. **The markers arrive.** `\e[200~`/`\e[201~` reach the process inside the pane intact. tmux only
   forwards them when the pane app has enabled DECSET 2004 — same precondition `paste-buffer -p`
   already depends on in production, so the PTY path is never worse on that front.
2. **On the real Claude TUI**, a 3-line paste through the PTY became **one 3-line composer entry**,
   not 3 submissions. Verified with no Enter ever sent (so: no API call); the proof is the composer
   *accumulating* across two consecutive pastes — nothing submitted on its own.
3. **Fidelity.** The PTY delivers byte-for-byte; `\n` stays `\n`. `paste-buffer -p` **rewrites
   `\n` → `\r`** inside the paste. Both were accepted by the TUI (tested with each separator), but
   the difference is real and the PTY path has to pick its separator deliberately — `\r` is the one
   production has already proven.
4. **The current path has a hard ceiling of 16344 bytes** — the tmux command-length limit. Above it
   `set-buffer` and `send-keys -l` return `rc=1` + `command too long`, so it fails **loudly**, and
   `paste_text` falls back to `_paste_linha_a_linha`: **2N−1 tmux calls** (~2500 subprocesses for a
   1250-line text). The PTY delivered **1 MB intact**.
5. **Latency is a non-issue**: open PTY + confirmed attach 26 ms, write-and-arrive 10 ms for a normal
   message (459 ms for 1 MB), close 3 ms — ~40 ms per send.
6. **The window-size objection, measured** (with a "native" client already attached,
   `window-size latest`): a PTY sized to the current window leaves it at 150x45 before, during and
   after — zero disturbance. Sized *wrong*, the window shrinks to the PTY's size while attached and
   recovers on detach. So sizing it isn't a nicety, it's what makes the path harmless.

Probe scripts: `probe.py` + `recv.py`, written to the session scratchpad (not committed — they are
a ten-line receiver plus a driver; re-derive from this list if needed).

7. **Image paste needs the keystroke, not the path** — `send-keys C-v` alone yields `[Image #1]`, so
   this is not a reason to build the PTY path (see the correction above).

#### Verdict (2026-08-08): not worth building

`tmux load-buffer -` closes the PTY's one measured win — the 16344-byte ceiling in item 4 above — in
one line of shell instead of a ~200-line module: **1.088 MB in 0.32s**, faster than the PTY's 459ms
for 1 MB (shipped in `docs/superpowers/plans/2026-08-07-envio-por-pty.md`; see
`docs/decisoes-2026-08-08-envio.md` for the full call).

The PTY also carries a targeting problem the terminal panel does not have: `attach` delivers to
whichever pane the tmux client last touched, but a send has to land on the **agent's** pane
specifically — `_pane_target` (`tmux.py:103`). On a session with a manual split, a PTY attach could
put the text in the owner's own shell instead. The window-size measurement above (item 6) is still
true and it is what makes an attached PTY *harmless to have open* — it says nothing about whether it
delivers to the right pane, and it doesn't.

What's left to justify building the PTY path: nothing measured. It stays on record here as a known
path, not as pending work — reopen only if a problem surfaces that `load-buffer` and `send-keys`
together can't solve.

What's still genuinely pending, unrelated to the PTY: swapping the post-Enter delivery check (a pane
read — it lies, see `docs/decisoes-2026-08-08-envio.md`) for reading the transcript instead, and all
three Windows unknowns in the section below.

#### Live keystroke streaming — considered and rejected (2026-08-07)

Typing into the Composer and forwarding each keystroke to the pane as you type is the *opposite* of
what this item is for. `send-keys` already sends keystrokes, and that is the defect: every `\n` in
the payload is an Enter the TUI submits on the spot. Streaming keeps that and adds three problems —
one network round trip per key, half a message stranded in the agent's input if the connection drops
mid-typing, and edits (backspace, cursor moves, phone autocorrect) that would have to be replayed
into a line editor that isn't ours. Bracketed paste exists precisely to say "this is a paste, not
keys" so the newlines land *inside* the field. Whoever wants to watch their typing land live already
has the terminal panel.

### Mirror the machine's terminal theme in xterm.js

The panel currently passes three colours to xterm (`foreground`, `background`, `cursor`), read from
the app's own tokens. The 16 ANSI colours are xterm.js's **defaults**, not the user's. Reading
`~/.config/kitty/kitty.conf` (`color0`–`color15` plus `font_family`) and passing them through would
make the embedded terminal identical to the user's kitty. It is a file read and a 16-key map.

### Windows: ConPTY exists, it is the measuring that is missing

Do not write "Windows has no PTY" — it does. **ConPTY** ships since Windows 10 1809, and psmux (the
multiplexer the app runs on there) is itself a native ConPTY multiplexer. What is missing from the
stdlib is the Python wrapper (`pywinpty` provides it; it is what Jupyter's terminal uses).

Genuinely unknown, and only answerable on a Windows machine:
- does psmux have `attach-session`? (the panel depends on it)
- does `pywinpty` work as the writer for the throwaway-PTY idea above?
- does bracketed paste survive the trip through psmux? (this is the whole point of the idea)

`scripts/test-psmux.py` is the vehicle — it is how this repo learned `paste-buffer` does not exist
there — and it covers **neither** `attach` nor paste today. Until measured, the terminal panel stays
gated off on Windows (`terminal_panel: os.name == "posix"`) and `send-keys` stays the only send path
there — **for lack of measurement, not because it is impossible**.

### Known limitations shipped on purpose

- **The hidden shell is keyed by name.** `term-<session>` dies with its agent session and follows a
  rename, but a session killed *outside* the app leaves it orphaned and invisible; reusing the name
  in another repo then reattaches a shell born in the old directory (`new_hidden_shell` compares
  `#{session_path}` and recreates on divergence, so this only bites the orphan case).
- **Attaching the panel resizes the session** to the panel's size while it is open, and the
  operations that count lines in the pane (option picker, AskUserQuestion stepper, model picker)
  answer **409** meanwhile. Sending a prompt is deliberately never blocked.
- **Closing the panel detaches, it does not kill** — anything running in the shell tab survives.
- Killing a session from the app kills its hidden shell too; this is what stops orphans accumulating.

## "Adicionar servidor" dialog hides the token field (2026-08-07)

`submitAddServer` (`frontend/src/screens/SessionList.svelte:492-509`) reads two pieces of state,
`addUrl` and `addToken`, and passes both to `addServer(url, token)`. But the rendered dialog shows
a single input labelled "Colar URL do servidor (com token)" — the token field is not on screen, so
`addToken` is always empty on that path.

It happens to work because the QR path (`handleScanServer`, same file, :512-527) accepts a URL
carrying the credential as a query param (`?token=…`, plus an optional `?api=` when the API lives on
another origin), and pasting that same URL into the one visible field works by accident. Either
render the token field the submit handler already expects, or drop `addToken` and make the
URL-with-token the documented, single input — but the current state is a form whose handler and
markup disagree.

## Stability under load — never measured (2026-07-29)

Performance **was** measured and is a non-issue at this scale; stability under load was not, and
that's the open question. Numbers from the machine that runs this daily (483 processes, 4 live
sessions), so nobody has to re-measure the cheap part:

| Path | Cost | Note |
|---|---|---|
| `procinfo._proc_children_map()` | 4.9 ms | reads `stat` of every process — grows with the machine, not with sessions |
| `tmux list-panes -a` | 2.1 ms | one fork for all sessions |
| `capture_pane`, per session | 2.0 ms | one fork **each** → 8 ms at 4 sessions, ~40 ms at 20 |
| `_open_jsonl` over 9 descendants | 0.30 ms | returned **nothing**, exactly as its own comment predicts |

A poll cycle is ~15 ms. Both heavy paths are linear (processes, sessions), so they only matter on
a busy host or with many sessions — not here.

What's actually untested is what a soak run would show, and none of it is visible in a 15 ms
timing: SSE connections left open for hours (the 25s watchdog reconnecting on a half-open socket),
file descriptors and `asyncio.to_thread` workers over a long run, the `capture_pane` burst described
at `registry.py:928` (`list_with_state`), and what happens when sessions are created and killed
repeatedly while the phone is subscribed.

Worth doing as a real experiment (N sessions, SSE open, forced reconnects, watch RSS/fd count over
hours) rather than by reading more code. Deferred deliberately — nothing observed is broken.

## Structural debt in the session list (2026-07-16)

> **Items 1–3 DONE (2026-07-17).** All three extractions shipped in full. Real numbers below.

Measured while building the kanban board. Nothing is broken — this is about the shape of
the code, and it already cost real bugs this session. In order of value per risk:

1. ✅ **DONE (2026-07-17) — Extract the multi-server SSE aggregation into a store.** The
   `slots`/`recompute`/`connect` trio now lives in `lib/sessions.ts` (pure dedup/order/classify,
   7 unit tests) + `lib/sessionsStore.svelte.ts` (a refcounted singleton: one `openSessionsStream`
   per server for the whole app, `retain`/`release` per consumer, Board's parse strategy — try/catch
   + `onServersChanged`). The three drifting copies (`Sidebar`, `SessionList`, `Board`) are gone;
   `Canvas` is a fourth consumer that reuses the same store instead of a fourth copy.

2. ✅ **DONE (2026-07-17) — `ConfirmDialog.svelte`.** Extracted as a chassis
   (`.confirm-backdrop`/`.confirm-card`/`.confirm-actions`); the two non-plain confirms (resume with
   a candidate list, add-server with an input) pass their body via a `{#snippet}` children slot, with
   that body's CSS kept in `Sidebar`. The shared `withServer` helper moved to `lib/auth.ts`.

3. ✅ **DONE (2026-07-17) — `SessionContextMenu.svelte`.** The row's context menu is now its own
   component, owning `menuMuted`/`branchView`/`chainView`; it also uses the shared `withServer`
   from `lib/auth.ts`.

Real result: `Sidebar.svelte` went from **1859 lines / 44 `$state`** to **1570 lines / 37 `$state`**
(the backlog's ~1100/~25 estimate was optimistic — the three items were done in full; the rest of
what remains is legitimate list template/CSS, not duplication).

**The bigger fish, still deliberately NOT done — and the gap is widening.** `Sidebar.svelte` and
`SessionList.svelte` are *the same feature written twice*: the session list, one for desktop and
one for mobile. At the time of writing that was 1570 + 1371 = 2941 lines combined. Re-measured
**2026-08-07: 2051 + 1550 = 3601** — 660 lines added to a duplication that was already the largest
item here, so every feature since has been paid for twice. CLAUDE.md already flags the risk ("make
the change in BOTH views and verify BOTH — they drift apart easily"). The three extractions above
all survive (`lib/sessions.ts`, `lib/sessionsStore.svelte.ts`, `ConfirmDialog.svelte`,
`SessionContextMenu.svelte`, `lib/auth.ts` — all still in place), so the remaining bulk really is
the duplicated view, not the parts already factored out. Worth its own session with the repo at
rest.

Not worth touching: the kebab (30 lines), the hover preview (already a component), and
resize/collapse (5 states, cohesive with the sidebar chrome).

## From phone testing (2026-06-25)

- **Mobile UI needs real adjustment** (general). The current layout is a working first
  cut on a phone but not refined — spacing, touch targets, widths, scrolling. Do a proper
  design pass with the front-end skills on a running phone session.
  *Note (2026-08-07): this one has no definition of done, which is why it survives every
  sweep. Either give it a concrete list of screens and defects, or close it.*

## Notes
- These are UX/feature items; the core loop (chat, live state, input, statusline) works.
