# claude-pocket

Drive a live Claude Code session (running in a `tmux` session on your machine) from your
phone over LAN/VPN, as a mobile chat. Backend: Python 3.14 + FastAPI (`backend/`).
Frontend: Svelte 5 PWA (`frontend/`). See `README.md` for architecture and API.

## tmux + Claude Code truecolor (IMPORTANT)

This project runs `claude` inside tmux. Inside tmux, Claude Code renders theme colors
wrong (teal, or pink/washed-out) while rendering correctly outside tmux. It's a known
Claude Code behavior: it caps color depth to 256 when `$TMUX` is set, and takes a fallback
render path when `TERM` starts with `tmux`/`screen`.

Fix (set in the SHELL rc before launching claude — `settings.json` env is not reliable for this):

```sh
export COLORTERM=truecolor
export CLAUDE_CODE_TMUX_TRUECOLOR=1
```
Plus a `~/.tmux.conf` with `default-terminal "xterm-256color"` (not `tmux-256color`).

Full explanation, fish syntax, reference config, and verify steps:
[`docs/tmux-truecolor-setup.md`](docs/tmux-truecolor-setup.md) and
[`docs/tmux.conf.example`](docs/tmux.conf.example).
