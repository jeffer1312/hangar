# tmux + Claude Code: fixing wrong/washed theme colors (teal / pink)

claude-pocket needs `claude` running inside a `tmux` session. Inside tmux, Claude Code
renders its theme colors **wrong** — the message/input bars come out teal, or pink /
washed-out — while the **same** Claude renders correctly when run directly in the terminal
(no tmux). This is a Claude Code behavior, not a tmux or terminal misconfiguration, and it
bites everyone who runs Claude in tmux on a truecolor terminal.

This doc explains the cause and the exact fix.

## Symptoms

- Message background / input box renders **teal** (a fallback color), or
- Theme colors render **pink / pale / washed-out** (especially on terminals using a
  custom 256/16-color palette — pywal, material-you, end-4/dots-hyprland, base16, etc.),
- ...but everything looks correct when you start `claude` **without** tmux.

## Root cause (verified against the Claude Code binary, v2.1.193)

There are **two independent gates**, both triggered by being inside tmux:

1. **Teal** — Claude checks `TERM`. If it `startsWith("tmux")` or `startsWith("screen")`,
   the fullscreen renderer takes a fallback path that paints a plain `background` token
   (teal). Fix: make the `TERM` Claude sees **not** start with `tmux`/`screen`.

2. **Pink / washed** — Claude caps its color depth to 256 whenever the `$TMUX` env var is
   present, **ignoring `COLORTERM` and `FORCE_COLOR`**. The relevant function in the binary:

   ```js
   swd(){
     if (process.env.CLAUDE_CODE_TMUX_TRUECOLOR) return false;   // escape hatch: skip downgrade
     if (process.env.TMUX && Et.level > 2) return Et.level = 2;  // in tmux: force 24-bit -> 256
     return false;
   }
   ```

   At 256-color, the theme's true 24-bit hex (e.g. a `#4d4d4d` gray) is quantized to the
   nearest palette index. On a terminal whose 256/16 palette was re-tinted (pywal,
   material-you, etc.) that index can be a pink/salmon — hence a gray bar renders pink.
   Outside tmux, `$TMUX` is unset, Claude emits true 24-bit, and the palette is never used.

Related upstream issues: anthropics/claude-code #60788, #59867, #35148, #39566.

## The fix

You need **both** of these in the environment **before** `claude` starts (putting them only
in `~/.claude/settings.json` `env` is not reliable for this — set them in your shell):

| Var | Why |
|---|---|
| `COLORTERM=truecolor` | makes Claude's base color level = 3 (24-bit). Without it the level is already 256 and the var below can't help. |
| `CLAUDE_CODE_TMUX_TRUECOLOR=1` | short-circuits the in-tmux downgrade above (undocumented; found in the binary). |

### bash / zsh — `~/.bashrc` / `~/.zshrc`
```bash
export COLORTERM=truecolor
export CLAUDE_CODE_TMUX_TRUECOLOR=1
```

### fish — `~/.config/fish/config.fish`
```fish
set -gx COLORTERM truecolor
set -gx CLAUDE_CODE_TMUX_TRUECOLOR 1
```

### tmux — `~/.tmux.conf`
Avoid the teal gate (don't let `TERM` start with `tmux`/`screen`) and pass truecolor through:
```tmux
set -g default-terminal "xterm-256color"
set -ga terminal-overrides ",xterm-256color:Tc"
set -ga terminal-features ",xterm-256color:RGB"
set-environment -g COLORTERM truecolor
```
A full reference config is in [`tmux.conf.example`](./tmux.conf.example).

> Apply order matters: open a **fresh** tmux pane/window (TERM and env are fixed at pane
> creation) and start a **new** `claude` (env is read at launch).

## Verify

1. Truecolor passes through tmux (tests tmux, not Claude):
   ```bash
   printf '\033[48;2;77;77;77m GRAY \033[0m \033[48;2;255;105;180m PINK \033[0m\n'
   ```
   The `GRAY` block must look gray (if it looks pink, truecolor isn't passing).

2. Start `claude` in the tmux pane — the UI/theme colors should match what you see outside
   tmux. Quick one-off test without editing rc files:
   ```bash
   COLORTERM=truecolor CLAUDE_CODE_TMUX_TRUECOLOR=1 claude
   ```

## Notes

- `FORCE_COLOR=3` does **not** fix this (the in-tmux downgrade ignores it) — use
  `CLAUDE_CODE_TMUX_TRUECOLOR=1`.
- The teal vs pink distinction is just which gate fired; the fix above covers both.
- `CLAUDE_CODE_TMUX_TRUECOLOR` is undocumented and was found in the Claude Code binary; it
  may change in future versions.
