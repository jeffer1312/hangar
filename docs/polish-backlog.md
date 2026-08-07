# Polish & feature backlog

Captured from live testing (phone, real session). Deferred by the user to the polish
phase — not blockers. Newest first.

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

### A second send path, through a throwaway PTY

Today every message the app sends goes through `tmux send-keys` — which sends **keystrokes**. The
terminal panel proved a different path exists: bytes written straight into a PTY master, delivered
by the kernel's tty layer, indistinguishable from a physical keyboard. That path handles what
`send-keys` handles badly: bracketed paste (a multi-line block arrives as *one* paste instead of N
lines the TUI may read as N submissions), image paste, and — the concrete pain that motivated this —
`cp-send` messages between Claude sessions arriving mangled.

**Do not replace `send-keys`.** It works, it is stateless, and it is the only path on Windows today.
The shape that pays off is a *throwaway* PTY used only where pasting matters: open, size it to the
window's current size, write as a bracketed paste, close. Nothing persistent, nothing to supervise.
The pieces already exist in `app/termsock.py`.

Measured, and it invalidates the obvious objection ("a PTY client would fight for the window size"):
`man tmux` on `window-size latest` says tmux uses *"the size of the client that had the most recent
activity"*. An idle PTY never claims the size; one sized to match the window changes nothing when it
does. The cost is a line of code, not a structural trade-off.

Two things the design must carry from the start:
- **the `send-keys` fallback is mandatory, not optional** — see the Windows item below;
- **the choice between paths must be explicit in code** (has a newline / exceeds N bytes), never
  "sometimes pasting fails".

**Measure before building any of it.** The whole idea rests on one assumption nobody has tested:
that a bracketed paste written into the PTY master survives the trip through `tmux attach` and
reaches the agent's TUI *as a paste*. If tmux does not forward the `\e[200~`/`\e[201~` markers to
the pane, the benefit evaporates and only the image case is left. Settle that first — it is a
ten-line script, not a project.

The experiment, once that holds. Same payload down both paths (`tmux send-keys` as it is today, and
a throwaway PTY), then compare what actually **arrived** against what was sent:

| Payload | What it is probing |
|---|---|
| one short line | the boring case must not regress |
| multi-line block | does it arrive as ONE submission or as N lines the TUI submits separately |
| ~100 KB paste | truncation, ordering, and how long each path takes |
| text with `\n`, tabs, `%`, quotes, accents, emoji | the escaping the `send-keys -l` fallback already struggles with |
| an image | only the PTY path can carry it at all |
| a real `cp-send` message | the concrete pain that motivated this |

Read the result from the agent's own transcript (the `.jsonl`), not from `capture-pane` — the pane
is a rendering and will lie about what was received.

The order of the criteria decides the design, so fix it up front: **fidelity first** (did it arrive
intact, and as a single submission), **failure mode second** (which path fails loudly and which
fails silently), **latency last** — a send is a human-triggered action, and tens of milliseconds do
not matter next to a mangled paste.

Then write the predicate from the measurements, not from intuition, and keep `send-keys` as the
fallback for everything the PTY path did not measurably win — plus Windows, unconditionally, until
the psmux items below are answered.

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
file descriptors and `asyncio.to_thread` workers over a long run, the `capture_pane` burst
described at `registry.py:787`, and what happens when sessions are created and killed repeatedly
while the phone is subscribed.

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
