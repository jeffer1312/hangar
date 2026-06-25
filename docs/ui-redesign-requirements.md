# UI/UX Redesign — Requirements & Research Brief

Source: user, 2026-06-25, during live phone testing. Goal: a deep UI/UX research pass
(workflows + design skills) → a concrete mobile redesign. Directive: **analyze everything,
leave nothing out; brainstorm doubts.**

## 1. Composer / input — ONE cohesive component
Today the input feels off (a big standalone "Interromper" button). Redesign into a single
unified component that holds the controls:
- **Model** shown + switchable from the input (not buried in the statusline).
- **Effort** shown + switchable from the input.
- **Context indicator**: a small ring/circle in the input showing context-window usage —
  like Codex's context dot.
- **Interrupt**: integrated cleanly (not a big separate button).
- **Slash commands**: a way to reach Claude Code's commands from here (how = TBD).
- Running time + tokens surfaced (improve the current display).

## 2. Information architecture — redistribute the statusline
- The raw statusline breaks / is too small on mobile → can't read it.
- Need to see ALL the info, or at least the most important (the **model**).
- Mobile → distribute pieces to where they fit: model → composer; context → composer ring;
  tokens/time → improved; rate-limit windows (e.g. the 5h window) → maybe a top bar.
  (Examples, not prescriptive — explore freely.)

## 3. Sessions & projects
- Go back / open other sessions easily.
- Open a session in a **specific folder**.
- **Scan a specific folder** that holds multiple projects → pick one to open/create a
  session (a project browser).

## 4. Commands & controls (from the phone)
- Reach Claude Code's **slash commands** somehow.
- Switch **model** + **effort** from the chat input.
- (Mechanism — real: the backend already drives the live session via `tmux send-keys`, so
  model/effort/commands = sending `/model`, `/effort`, `/<command>`; context/tokens/time =
  parsing the statusline / transcript usage. The UI is the open design question.)

## 5. Animations
- Good, **meaningful** animations that reflect what Claude is doing (its live activity/state)
  — "tipo o claude". The running time + tokens exist; improve them.

## 6. Process
- Deep UI/UX research via **workflows** + design skills (taste / ui-ux-pro-max /
  frontend-design / emil).
- Don't leave anything out. Brainstorm doubts.

## Decisions (resolved with the user)
- **Project scanner:** the user supplies a **parent folder**; list ALL its subfolders as
  openable projects (create/open a session in the chosen one). Parent path is configurable.
- **Slash commands:** built-ins **+ custom/skills**.
- **Research depth:** deep + competitive (web patterns: Codex, Claude, Warp, …).
- **Design skills:** the user is reloading `ui-ux-pro-max`, `frontend-design`, taste —
  these drive the design. NOTE: workflow *subagents* cannot invoke skills, so the skills are
  invoked in the main loop to load their principles, that direction is embedded into the
  research-workflow agent prompts, and the skills are applied during synthesis + implementation.

## Constraints (real, ground the design)
- Backend drives the live claude via `send-keys` + reads the statusline (`capture-pane`) +
  the JSONL transcript. So model/effort/commands = send the slash command; context/tokens/
  time = parse statusline/usage. All feasible without new claude integrations.
- Mobile-first (iPhone PWA, standalone). Secure context (HTTPS via Tailscale).
- Design skills available here: `design-taste-frontend`, `emil-design-eng`. (`ui-ux-pro-max`
  / `frontend-design` are not installed in this environment — use the available ones + the
  research workflow.)
