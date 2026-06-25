# Claude Pocket — Frontend Design Brief

> Single source of truth for the build engineer.
> Stack: Svelte 5 + Vite + TypeScript, PWA (manifest + service worker), no UI framework.

---

## 1. Visual Direction

### 1.1 Design Language

Dark, calm, developer-tool aesthetic. Inspired by terminal UIs and iOS Notes/Messages dark mode.
No gradients, no glassmorphism, no shadows-for-decoration. Depth comes from background layers only.
High contrast for readability under bright sunlight / one-handed use.

### 1.2 Color Palette

All values are CSS custom properties defined on `:root` (dark-only; no light mode in v1).

```css
:root {
  /* Backgrounds — three layers, each 6-8% brighter than previous */
  --bg-base:      #0d0d0f;   /* page background, deepest */
  --bg-surface:   #141416;   /* card / bubble container */
  --bg-elevated:  #1c1c1f;   /* tool cards, inputs, modals */
  --bg-hover:     #252528;   /* hover/press state */

  /* Borders */
  --border-subtle:  rgba(255,255,255,0.06);
  --border-default: rgba(255,255,255,0.12);
  --border-strong:  rgba(255,255,255,0.22);

  /* Text */
  --text-primary:   #f0f0f2;   /* headings, message text */
  --text-secondary: #9696a0;   /* timestamps, metadata */
  --text-muted:     #5a5a66;   /* placeholders */
  --text-inverse:   #0d0d0f;   /* text on accent backgrounds */

  /* Accent — single, deliberate */
  --accent:         #7c6af7;   /* purple-indigo, Claude brand-adjacent */
  --accent-dim:     rgba(124,106,247,0.18);
  --accent-press:   #6857e0;

  /* Semantic */
  --success:  #34c759;   /* iOS green */
  --error:    #ff453a;   /* iOS red */
  --warning:  #ff9f0a;   /* iOS amber */

  /* Status pill */
  --pill-working-bg:  rgba(124,106,247,0.15);
  --pill-working-fg:  #a99cf9;
  --pill-idle-bg:     rgba(52,199,89,0.12);
  --pill-idle-fg:     #34c759;
  --pill-dead-bg:     rgba(255,69,58,0.12);
  --pill-dead-fg:     #ff453a;
  --pill-input-bg:    rgba(255,159,10,0.12);
  --pill-input-fg:    #ff9f0a;
}
```

### 1.3 Typography

Single family: **SF Pro** stack (available on iOS, graceful fallback everywhere).

```css
--font-ui:   -apple-system, BlinkMacSystemFont, "SF Pro Text", system-ui, sans-serif;
--font-mono: "SF Mono", ui-monospace, "Cascadia Code", "Fira Code", monospace;
```

Scale (unitless rem; base 16 px):

| Token          | Size    | Weight | Use                         |
|----------------|---------|--------|-----------------------------|
| `--text-xs`    | 0.75rem | 400    | timestamps, meta            |
| `--text-sm`    | 0.875rem| 400    | tool cards, secondary       |
| `--text-base`  | 1rem    | 400    | body, chat bubbles          |
| `--text-lg`    | 1.125rem| 500    | session names, pill label   |
| `--text-xl`    | 1.25rem | 600    | screen titles               |

Line-height: `1.55` for prose, `1.4` for UI labels, `1.3` for mono.

### 1.4 Spacing

8-pt grid. Tokens:

```css
--space-1: 4px;
--space-2: 8px;
--space-3: 12px;
--space-4: 16px;
--space-5: 20px;
--space-6: 24px;
--space-8: 32px;
--space-10: 40px;
```

### 1.5 Border Radius

```css
--radius-sm:  6px;    /* tool cards, tags */
--radius-md:  12px;   /* bubbles, inputs */
--radius-lg:  18px;   /* pill, option buttons */
--radius-xl:  24px;   /* session cards */
--radius-full: 9999px;
```

### 1.6 Prominent Status Pill

Fixed to the top of the chat view, centered horizontally, below the navbar.
Floats above the message list (sticky, not part of scroll flow).
Height 32 px, horizontal padding 14 px, `border-radius: var(--radius-lg)`.

```
State          Background           Text color        Indicator
─────────────────────────────────────────────────────────────────
working        --pill-working-bg    --pill-working-fg  spinning dot (CSS animation)
idle           --pill-idle-bg       --pill-idle-fg     solid dot, no animation
awaiting_input --pill-input-bg      --pill-input-fg    pulsing dot
dead           --pill-dead-bg       --pill-dead-fg     ✕ icon, no animation
```

The spinning dot is a 7 px circle with `animation: spin 0.8s linear infinite`.
The label text for `working` is the verbatim `label` from the SSE event (e.g., "Elucidating…").

### 1.7 Motion

Keep motion subtle and purposeful. Never animate content scrolling.

| Element                  | Property          | Duration  | Easing            |
|--------------------------|-------------------|-----------|-------------------|
| Pill state change        | background, color | 200 ms    | ease-out          |
| Spinner rotation         | transform         | 800 ms    | linear, infinite  |
| Bubble appear            | opacity 0→1, translateY 6→0 | 180 ms | ease-out |
| Tool card expand/collapse| height, opacity   | 200 ms    | ease-in-out       |
| Option buttons appear    | opacity, translateY | 220 ms  | ease-out, stagger 40 ms |
| Session card press       | scale 1→0.97      | 80 ms     | ease-in-out       |
| Screen transition        | none (instant)    | —         | —                 |

All animations respect `prefers-reduced-motion: reduce` — disable everything except state-color transitions.

---

## 2. Screens

### 2.1 Screen: Session List (`/`)

Displayed when no session is active.

```
┌─────────────────────────────────────┐
│ ████ safe-area-top ████████████████ │
│ ┌───────────────────────────────┐   │
│ │ claude pocket           [+]   │   │  ← NavBar (48px high)
│ └───────────────────────────────┘   │
│                                     │
│  ┌─────────────────────────────┐    │
│  │  ● nome-da-sessao      idle │    │  ← SessionCard
│  │  ~/projetos/foo             │    │
│  │  última atividade: agora    │    │
│  └─────────────────────────────┘    │
│                                     │
│  ┌─────────────────────────────┐    │
│  │  ● outra-sessao    working  │    │
│  │  ~/projetos/bar             │    │
│  │  última atividade: 5 min    │    │
│  └─────────────────────────────┘    │
│                                     │
│  ┌─────────────────────────────┐    │
│  │  ✕ sessao-morta      dead   │    │
│  └─────────────────────────────┘    │
│                                     │
│ [empty state if no sessions:        │
│   "Nenhuma sessão ativa"            │
│   "Toque em + para criar"        ]  │
│                                     │
│ ████ safe-area-bottom ██████████ │
└─────────────────────────────────────┘
```

**Session Card anatomy:**
- Background: `--bg-surface`; border: 1px solid `--border-subtle`; radius: `--radius-xl`
- Left status dot: 8 px circle, color matches state (accent for working/awaiting_input, success for idle, error for dead)
- Session name: `--text-lg`, `--text-primary`
- cwd: `--text-sm`, `--text-secondary`, truncated with ellipsis
- State badge: right-aligned small pill, same semantic colors as status dot
- "última atividade": `--text-xs`, `--text-muted`
- Tap target: full card, min 56 px height
- Long-press or swipe-left: reveal "Excluir" action (`DELETE /api/sessions/{name}`)
- Tap: navigate to Chat screen

**Create Session Sheet (bottom sheet, triggered by [+]):**
```
┌─────────────────────────────────────┐
│                  ───                │  ← drag handle
│  Nova sessão                        │
│                                     │
│  Nome          [__________________] │
│  Diretório     [__________________] │  ← optional, defaults to cwd
│                                     │
│  [         Criar sessão           ] │  ← full-width accent button
│                                     │
│ ████ safe-area-bottom ██████████ │
└─────────────────────────────────────┘
```

Bottom sheet: `background: --bg-elevated`, top-radius 20 px, closes on backdrop tap.
Calls `POST /api/sessions {name, cwd}`.

---

### 2.2 Screen: Chat (`/:sessionName`)

```
┌─────────────────────────────────────┐
│ ████ safe-area-top ████████████████ │
│ ┌───────────────────────────────┐   │
│ │ ‹  nome-da-sessao   [•••]     │   │  ← NavBar: back, session name, overflow menu
│ └───────────────────────────────┘   │
│ ┌─────────────────────────────┐     │
│ │     ◌  Elucidando…          │     │  ← STATUS PILL (sticky, not scrolled)
│ └─────────────────────────────┘     │
│                                     │
│ ┌──────── scroll area ────────┐     │
│ │                             │     │
│ │ [tool card: Write]          │     │
│ │ [tool card filled: Write ✓] │     │
│ │                             │     │
│ │              [user bubble]  │     │
│ │                             │     │
│ │ [assistant bubble]          │     │
│ │                             │     │
│ │ ─── awaiting_input ──────── │     │  ← only when state == awaiting_input
│ │ Qual abordagem prefere?     │     │
│ │ [  Opção A              ]   │     │
│ │ [  Opção B              ]   │     │
│ │ [  Opção C              ]   │     │
│ │ [  Cancelar (Esc)       ]   │     │
│ │                             │     │
│ └─────────────────────────────┘     │
│                                     │
│ ┌──────── COMPOSER ───────────┐     │  ← sticky above keyboard
│ │ [textarea            ] [↑]  │     │
│ │ [Interromper (Esc)       ]  │     │  ← shown when state == working
│ └─────────────────────────────┘     │
│ ████ safe-area-bottom ██████████ │
└─────────────────────────────────────┘
```

---

## 3. State Rendering — Detailed

### 3.1 Status Pill

Position: `position: sticky; top: 0; z-index: 10;` inside the chat screen header zone.
The pill is always visible; its content and color shift per state.

**`working`**
```
╭──────────────────────────────────╮
│  ◌  Elucidando…                  │   ◌ = spinning CSS dot
╰──────────────────────────────────╯
```
- Background `--pill-working-bg`, text `--pill-working-fg`
- Label = verbatim `label` from SSE StateEvent (e.g., "Elucidating…" — displayed as-is, no translation)
- Spinner dot: 7 px, `--accent`, `animation: spin 0.8s linear infinite`
- Composer textarea: still visible but disabled (`opacity: 0.4`, `pointer-events: none`)
- Interrupt button visible: "Interromper" → `POST /interrupt`

**`idle`**
```
╭───────────────────────────────╮
│  ●  Pronto                    │   ● = static dot
╰───────────────────────────────╯
```
- Background `--pill-idle-bg`, text `--pill-idle-fg`
- Composer enabled and auto-focused
- Interrupt button hidden

**`awaiting_input`**
```
╭────────────────────────────────╮
│  ◎  Aguardando resposta        │   ◎ = pulsing dot (scale 1↔1.3, 1s ease-in-out infinite)
╰────────────────────────────────╯
```
- Background `--pill-input-bg`, text `--pill-input-fg`
- Below the scroll area (appended as the last scroll item, not fixed): show `question` text in `--text-primary`, then option buttons (see below)
- Composer textarea: hidden entirely — options replace it
- Cancel/Interrupt button visible as last option

**`dead`**
```
╭────────────────────────────────╮
│  ✕  Sessão encerrada           │
╰────────────────────────────────╯
```
- Background `--pill-dead-bg`, text `--pill-dead-fg`
- Composer area hidden, replaced by: "Esta sessão foi encerrada." + "← Voltar" button

---

### 3.2 Awaiting Input — Options Rendering

When `state === "awaiting_input"`, append at bottom of scroll list (after all messages):

```
┌──────────────────────────────────────┐
│ Qual abordagem prefere?              │  ← question text, --text-primary, --text-lg
│                                      │
│ ┌──────────────────────────────────┐ │
│ │  1.  Abordagem incremental       │ │  ← option button
│ └──────────────────────────────────┘ │
│ ┌──────────────────────────────────┐ │
│ │  2.  Reescrever do zero          │ │
│ └──────────────────────────────────┘ │
│ ┌──────────────────────────────────┐ │
│ │  ✕  Cancelar                     │ │  ← interrupt pseudo-option, --error color
│ └──────────────────────────────────┘ │
└──────────────────────────────────────┘
```

Option button style:
- Background `--bg-elevated`, border `1px solid --border-default`, radius `--radius-lg`
- Height min 52 px (touch target), full-width, left-aligned text with left padding 16 px
- Number prefix: `--text-secondary`, monospace
- On tap: immediate visual press (bg → `--bg-hover`), then `POST /select {option: i}` (1-based)
- Cancel button: `POST /interrupt`, styled with `--error` border and text
- Composer area completely hidden (no textarea visible) when in this state

---

### 3.3 Tool Use / Tool Result Cards

Tool events are rendered inline in the message timeline, in chronological order.

**Phase 1: tool_use received (result pending)**
```
┌────────────────────────────────────────┐
│  ⚙  Write                         ⟳   │  ← tool_name, spinner on right
│  path: src/components/Foo.svelte       │  ← short summary of tool_input
└────────────────────────────────────────┘
```

**Phase 2: tool_result received (matched by tool_use_id)**
```
┌────────────────────────────────────────┐
│  ⚙  Write                         ✓   │  ← spinner replaced by check
│  path: src/components/Foo.svelte       │
└────────────────────────────────────────┘
```

**Phase 2b: is_error = true**
```
┌────────────────────────────────────────┐  ← border: 1px solid --error
│  ⚙  Bash                          ✗   │  ← ✗ in --error color
│  command: npm run build                │
│  ▸ Erro: exit code 1                  │  ← collapsible result preview
└────────────────────────────────────────┘
```

Card anatomy:
- Background `--bg-elevated`, border `1px solid --border-subtle`, radius `--radius-sm`
- Left margin: 0 (full-width, not indented like bubbles)
- Icon: ⚙ in `--text-secondary`, 14 px
- Tool name: `--text-sm`, `--text-primary`, font-weight 500, mono
- Input summary: one or two key fields extracted from `tool_input` (see Section 5 for field priority)
- Status icon (right): `--text-muted` spinner, `--success` checkmark, `--error` ✗
- Tap: toggle expansion to show full `result` text in a scrollable mono block (max-height 200 px, overflow-y auto)
- Collapsed result preview (error only): first 80 chars of `result`, prefixed with `▸`

**Tool input summary extraction priority (by tool_name):**
- `Write` / `Read` / `Edit` → `file_path` or `path`
- `Bash` → `command` (first 60 chars)
- `WebSearch` → `query`
- `WebFetch` → `url`
- Any other → first key-value pair in `tool_input`

---

### 3.4 Chat Bubbles

**User message (right-aligned):**
```
                ┌────────────────────────┐
                │  Cria o componente X   │
                └────────────────────────┘
                   [timestamp]
```
- Background `--accent`, text `--text-inverse`
- Radius: 18 px top-left, 18 px top-right, 18 px bottom-left, 4 px bottom-right (iOS style)
- Max-width: 80% of container
- Timestamp: `--text-xs`, `--text-muted`, right-aligned, below bubble

**Assistant message (left-aligned):**
```
┌────────────────────────────────┐
│  Vou criar o componente agora. │
│                                │
│  O arquivo será:               │
│  `src/components/X.svelte`     │
└────────────────────────────────┘
[timestamp]
```
- Background `--bg-surface`, border `1px solid --border-subtle`
- Text `--text-primary`
- Radius: 4 px top-left, 18 px top-right, 18 px bottom-right, 18 px bottom-left
- Max-width: 88% (slightly wider to accommodate code)
- Markdown rendering: use a lightweight renderer for `**bold**`, `*italic*`, `` `inline code` `` (mono, `--bg-elevated`, radius 4 px), fenced code blocks (mono, `--bg-elevated`, scrollable, border `--border-subtle`)
- Timestamp: `--text-xs`, `--text-muted`, left-aligned, below bubble

---

## 4. Composer

The composer is sticky above the keyboard. On iOS Safari, this is achieved via:

```css
.composer {
  position: sticky;        /* or fixed — see iOS note */
  bottom: 0;
  padding-bottom: env(safe-area-inset-bottom);
  background: var(--bg-base);
  border-top: 1px solid var(--border-subtle);
}
```

### 4.1 Layout (idle / working states)

```
┌──────────────────────────────────────────────┐
│ ┌────────────────────────────────────┐  [↑]  │
│ │ Mensagem para Claude…              │       │
│ │                                    │       │  ← textarea auto-grows, max 5 lines
│ └────────────────────────────────────┘       │
│  [    Interromper    ]   ← only when working  │
└──────────────────────────────────────────────┘
```

**Textarea:**
- `resize: none`, auto-height via `scrollHeight` on `input` event, min 44 px, max ~120 px
- Background `--bg-elevated`, border `1px solid --border-default`, radius `--radius-md`
- Text `--text-primary`, placeholder `--text-muted`
- Focus ring: `2px solid --accent`
- Font: `--font-ui`, `--text-base`

**Send button [↑]:**
- 44×44 px, background `--accent`, radius `--radius-md`
- Icon: upward arrow (SVG)
- Disabled (background `--bg-hover`, icon `--text-muted`) when: textarea is empty OR state is `working`
- On tap: `POST /input {text}`, clear textarea, scroll to bottom

**Interromper button:**
- Full-width, height 44 px, background transparent, border `1px solid --error`, text `--error`, radius `--radius-md`
- Visible only when `state === "working"`
- On tap: `POST /interrupt`

### 4.2 Keyboard Handling (iOS Safari)

```
visualViewport API approach (required for iOS):

window.visualViewport.addEventListener('resize', () => {
  const keyboardHeight = window.innerHeight - window.visualViewport.height;
  composer.style.transform = `translateY(-${keyboardHeight}px)`;
});
```

The composer is `position: fixed; bottom: 0` and translated upward when the keyboard appears.
`padding-bottom: env(safe-area-inset-bottom)` ensures it clears the iPhone home bar.

---

## 5. iOS Specifics

### 5.1 Safe Areas

All four edges use CSS env() insets:

```css
.navbar      { padding-top: env(safe-area-inset-top); }
.composer    { padding-bottom: env(safe-area-inset-bottom); }
.session-list{ padding-bottom: calc(env(safe-area-inset-bottom) + 16px); }
```

Set in `<meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover">`.

### 5.2 Momentum Scroll

The message list must use `-webkit-overflow-scrolling: touch` (legacy) AND `overscroll-behavior: contain`:

```css
.message-list {
  overflow-y: scroll;
  -webkit-overflow-scrolling: touch;
  overscroll-behavior-y: contain;
  scroll-behavior: auto;           /* NOT smooth — smooth disables momentum */
}
```

Auto-scroll to bottom on new message: `element.scrollTop = element.scrollHeight` (not `scrollIntoView` — avoid scroll jank).

### 5.3 Tap Targets

Minimum 44×44 pt on every interactive element (Apple HIG). Apply via:
```css
.tap-target {
  min-height: 44px;
  min-width: 44px;
  display: flex;
  align-items: center;
}
```

All buttons, list rows, and option items meet this threshold.

### 5.4 PWA Manifest & Meta

```html
<!-- index.html -->
<meta name="apple-mobile-web-app-capable" content="yes">
<meta name="apple-mobile-web-app-status-bar-style" content="black-translucent">
<meta name="theme-color" content="#0d0d0f">
<link rel="apple-touch-icon" href="/icons/icon-180.png">
```

`manifest.webmanifest`:
```json
{
  "name": "Claude Pocket",
  "short_name": "Pocket",
  "display": "standalone",
  "background_color": "#0d0d0f",
  "theme_color": "#0d0d0f",
  "orientation": "portrait",
  "start_url": "/",
  "icons": [
    { "src": "/icons/icon-192.png", "sizes": "192x192", "type": "image/png" },
    { "src": "/icons/icon-512.png", "sizes": "512x512", "type": "image/png" },
    { "src": "/icons/icon-180.png", "sizes": "180x180", "type": "image/png" }
  ]
}
```

### 5.5 Input Zoom Prevention

Prevent iOS from zooming on textarea focus:
```html
<meta name="viewport" content="width=device-width, initial-scale=1, maximum-scale=1, viewport-fit=cover">
```

Or set `font-size: 16px` on the textarea (iOS only zooms inputs < 16 px).

---

## 6. Auth Flow & Login Screen

On first open (no token in localStorage), show the Login screen before any other screen.

```
┌─────────────────────────────────────┐
│ ████ safe-area-top ████████████████ │
│                                     │
│     Claude Pocket                   │  ← --text-xl, centered, top 80px
│                                     │
│  URL do servidor                    │
│  [http://192.168.x.x:8000        ]  │  ← default same-origin or last saved
│                                     │
│  Token                              │
│  [••••••••••••••••••••••••••••••  ] │  ← type="password"
│                                     │
│  [         Conectar               ] │  ← accent button
│                                     │
│  [error message if auth fails]      │
│                                     │
└─────────────────────────────────────┘
```

On "Conectar":
1. Store `baseUrl` and `token` to `localStorage`.
2. Set cookie: `document.cookie = \`auth_token=\${token}; path=/; SameSite=Lax\``
3. Test connection with `GET /api/sessions` using `Authorization: Bearer <token>`.
4. On 200: navigate to Session List.
5. On error: show inline error, keep form.

Logout: overflow menu on Session List → "Sair" → clear localStorage and cookie, back to Login.

---

## 7. API Layer

All fetch calls go through a single module `src/lib/api.ts`.

```typescript
// src/lib/api.ts  (signature reference only)

export function getBaseUrl(): string        // reads localStorage
export function getToken(): string | null   // reads localStorage

export function getSessions(): Promise<SessionInfo[]>
export function createSession(name: string, cwd?: string): Promise<SessionInfo>
export function deleteSession(name: string): Promise<void>
export function getHistory(name: string): Promise<ChatEvent[]>
export function sendInput(name: string, text: string): Promise<void>
export function selectOption(name: string, option: number): Promise<void>  // 1-based
export function interrupt(name: string): Promise<void>

// SSE: returns an EventSource-like object; caller attaches .onmessage / .onerror
export function openEventStream(name: string): EventSource
// SSE URL: `${baseUrl}/api/sessions/${name}/events?token=${token}` (dev fallback)
// Production: same-origin httpOnly cookie handles auth; omit ?token param
```

Error handling: all functions throw on non-2xx. Caller catches and shows toast or inline error.

---

## 8. Component & File Structure

```
frontend/
├── index.html
├── vite.config.ts
├── svelte.config.js
├── tsconfig.json
├── manifest.webmanifest
├── public/
│   └── icons/
│       ├── icon-180.png
│       ├── icon-192.png
│       └── icon-512.png
├── src/
│   ├── app.css              ← CSS custom properties, resets, base styles
│   ├── main.ts              ← mount App, register service worker
│   ├── App.svelte           ← root: router (hash-based), auth guard
│   │
│   ├── lib/
│   │   ├── api.ts           ← all fetch + SSE calls (see Section 7)
│   │   ├── auth.ts          ← localStorage token/baseUrl helpers, cookie setter
│   │   ├── stores.ts        ← Svelte 5 runes: $state for sessions, activeSession
│   │   ├── markdown.ts      ← lightweight markdown → HTML (no deps, manual)
│   │   └── types.ts         ← SessionInfo, ChatEvent, StateEvent interfaces
│   │
│   ├── screens/
│   │   ├── Login.svelte          ← baseUrl + token form
│   │   ├── SessionList.svelte    ← list + create sheet
│   │   └── Chat.svelte           ← full chat screen, orchestrates SSE
│   │
│   └── components/
│       ├── NavBar.svelte          ← top bar with back button + title
│       ├── StatusPill.svelte      ← prominent state pill
│       ├── MessageList.svelte     ← scroll container, renders all events
│       ├── UserBubble.svelte      ← right-aligned bubble
│       ├── AssistantBubble.svelte ← left-aligned bubble, renders markdown
│       ├── ToolCard.svelte        ← tool_use + tool_result merged card
│       ├── OptionButtons.svelte   ← awaiting_input question + option list
│       ├── Composer.svelte        ← textarea + send + interrupt
│       ├── SessionCard.svelte     ← card in session list
│       ├── CreateSessionSheet.svelte  ← bottom sheet modal
│       └── icons/
│           ├── IconSend.svelte
│           ├── IconInterrupt.svelte
│           └── IconTool.svelte
│
└── sw.ts                    ← service worker (cache-first for assets only)
```

### 8.1 Key Svelte 5 Patterns

**Global state (stores.ts):**
```typescript
// Svelte 5 runes pattern
export const sessions = $state<SessionInfo[]>([]);
export const activeSession = $state<string | null>(null);
```

**Chat.svelte responsibilities:**
1. On mount: `GET /history` → populate message list
2. Open SSE via `openEventStream(sessionName)`
3. `"message"` event → append ChatEvent to list; if `kind === "tool_result"`, find matching `tool_use` card by `tool_use_id` and update it in-place
4. `"state"` event → update local `sessionState` rune → StatusPill reacts
5. On unmount: close EventSource

**ToolCard.svelte internal state:**
```typescript
let phase: 'pending' | 'done' | 'error' = $state('pending');
let expanded = $state(false);
```
Parent passes initial `ChatEvent` (tool_use); parent calls a `fill(result: ChatEvent)` method when tool_result arrives — or bind the tool_use_id → tool_result map in MessageList.

---

## 9. Routing

Hash-based routing (no server config needed for PWA):

```
#/             → Login screen    (when no token)
#/             → Session List    (when authenticated)
#/chat/:name   → Chat screen
```

`App.svelte` reads `location.hash`, checks auth, renders the correct screen.
Navigation is programmatic: `location.hash = '#/chat/' + encodeURIComponent(name)`.

---

## 10. Service Worker

Minimal: cache-first for static assets (JS, CSS, icons), network-first for `/api/*`.

```typescript
// sw.ts
const STATIC_CACHE = 'pocket-v1';
const STATIC_ASSETS = ['/', '/index.html', '/manifest.webmanifest'];

self.addEventListener('fetch', (event: FetchEvent) => {
  if (event.request.url.includes('/api/')) return; // never cache API
  event.respondWith(caches.match(event.request).then(r => r ?? fetch(event.request)));
});
```

Registered in `main.ts`:
```typescript
if ('serviceWorker' in navigator) {
  navigator.serviceWorker.register('/sw.js');
}
```

---

## 11. Non-Negotiables Checklist

| Requirement                                    | Implementation                                 |
|------------------------------------------------|------------------------------------------------|
| Status pill always visible                     | `position: sticky; top: 0; z-index: 10`       |
| Live working label verbatim                    | `label` from SSE state event, no translation  |
| iOS safe areas                                 | `env(safe-area-inset-*)` on navbar + composer |
| Composer sticky above keyboard                 | `visualViewport` resize + `translateY`        |
| Momentum scroll                                | `-webkit-overflow-scrolling: touch`           |
| Min tap targets 44×44                          | enforced via CSS on all interactives          |
| Tool cards matched by tool_use_id              | Map in MessageList, filled on tool_result     |
| Options POST with 1-based index                | `selectOption(name, i)` where i starts at 1   |
| No token in SSE URL on production              | same-origin httpOnly cookie; `?token` is dev  |
| Interrupt available during working             | "Interromper" button in Composer              |
| Dead state: composer hidden                    | `{#if state !== 'dead'}` on Composer          |
| Awaiting_input: textarea hidden, options shown | OptionButtons replaces Composer               |
| Markdown in assistant bubbles                  | lightweight inline parser (no heavy dep)      |
| `prefers-reduced-motion`                       | disable all transforms/opacity anims          |
| No light mode in v1                            | single dark CSS variable set                  |
