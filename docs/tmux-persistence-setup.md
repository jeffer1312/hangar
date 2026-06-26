# tmux session persistence: survive reboot / OOM

claude-pocket mirrors a live `tmux` session to your phone. If the machine reboots or
the kernel OOM-kills the tmux server, that session — and the `claude` running in it —
is gone. This restores it automatically.

## Install

```bash
./scripts/tmux-persist-setup.sh
```

Idempotent, no root. It:

1. Clones **tmux-resurrect** + **tmux-continuum** (via TPM) into `~/.tmux/plugins/`.
2. Appends the plugin block to `~/.tmux.conf` if missing (backs it up first).
3. Installs a **systemd user timer** that auto-saves the layout every 15 min.

Then, in tmux: press `prefix + I` (capital i) once to let TPM load the plugins, and
`tmux source-file ~/.tmux.conf` to reload.

Restore is automatic on the next fresh tmux start. Manual: `prefix + Ctrl-s` (save),
`prefix + Ctrl-r` (restore).

## Why a systemd timer and not continuum's auto-save

continuum's periodic auto-save is driven by the **tmux status line**. The reference
conf (`tmux.conf.example`) runs `status off`, which kills that hook — so continuum
**alone never auto-saves**. The timer drives the save from outside the status line;
continuum is kept only for restore-on-start (`@continuum-restore on`,
`@continuum-save-interval 0`).

If you prefer the status bar ON, you don't need the timer: set
`@continuum-save-interval '15'` and skip `tmux-persist-setup.sh`.

## Gotchas (verified)

- **Save file only appears when state changes.** resurrect dedups identical consecutive
  saves: it writes the new file, `cmp`s it against `last`, and `rm`s it if unchanged. So
  "no new file in `~/.local/share/tmux/resurrect/`" is **not** a failure — it means the
  layout didn't change since the last save.
- **Saves live in** `~/.local/share/tmux/resurrect/` (XDG), not `~/.tmux/resurrect/`.
- **Socket path matters.** The default tmux server socket is `${TMUX_TMPDIR:-/tmp}/tmux-<uid>/default`.
  The service's `ConditionPathExists` points there; a wrong path makes systemd report
  "success" while silently skipping the save (`unmet condition`). The install script
  resolves it at install time.
- **Logout stops the timer** (systemd *user* units run only while your session is live).
  For a headless box that should keep saving after logout: `sudo loginctl enable-linger $USER`.

## Verify

```bash
systemctl --user list-timers tmux-resurrect-save.timer   # next/last fire
# force a state change + save, then confirm a fresh .txt appears:
tmux new-window -t 0: -n zz && systemctl --user start tmux-resurrect-save.service
ls -lt ~/.local/share/tmux/resurrect/*.txt | head -2
tmux kill-window -t 0:zz
```

## Uninstall

```bash
./scripts/tmux-persist-setup.sh --uninstall
```

Removes the systemd units. Leaves plugins and the conf block in place (it tells you how
to remove those too).
