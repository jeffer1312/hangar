# Running the backend as a systemd user service

**This page only matters if you run the backend via `scripts/services-setup.sh`**
(a persistent systemd *user* service, optionally with `loginctl enable-linger` so it
survives logout/reboot). The default in the README — launching `uv run python -m app.main`
from a terminal — needs **none** of this: that process inherits your shell/session
environment directly, and its tmux server lives in your login session, not a service cgroup.

Two things change once the backend is a systemd service.

## 1. The tmux server must not live in the service cgroup

The backend spawns the tmux server that hosts every session. If that server is born as a
plain child of the service, it lands in the service's control-group. Then
`systemctl --user restart claude-pocket-backend` sends SIGTERM to the **whole** cgroup
(default `KillMode=control-group`) and kills the tmux server and **every session** — including
the one driving the app. No `KillMode` value fixes this cleanly (`control-group` kills tmux;
`process`/`mixed` orphan the Python worker on port 8765).

The backend handles this itself: `tmux.py:_scope_prefix()` wraps `tmux new-session` in
`systemd-run --user --scope`, so the server is born in its own transient scope, immune to
backend restarts. It is gated on `systemd-run` + `XDG_RUNTIME_DIR`; on non-systemd hosts
(macOS, plain Linux) it is a no-op plain spawn, where the cgroup teardown problem doesn't exist.

Nothing to configure — it's automatic. It just means restarts are safe.

## 2. Environment inheritance (headless / linger)

A systemd user service starts with a **minimal** environment. It does *not* automatically
inherit your graphical login session's env — no `SSH_AUTH_SOCK`, no unlocked keyring, no
`WAYLAND_DISPLAY`. So sessions spawned by the app can't reach `ssh-agent` or the secret store,
and tools like `gh`/`git push` fail ("token invalid" = keyring unreachable), even though the
same commands work in your terminal.

Fix: at graphical login, push the session env into the systemd user manager, then restart the
backend so it re-inherits. Most desktops already run `dbus-update-activation-environment`; if
yours doesn't fully, add it to your compositor's startup:

```sh
dbus-update-activation-environment --systemd --all && \
  systemctl --user restart claude-pocket-backend.service
```

- **Hyprland:** an `exec-once` line in your config.
- **GNOME/KDE/sway:** usually already imported at login; add the line above if not.
- **Fully headless (no graphical session, e.g. driving only from the phone):** there is no
  session env to inherit and the keyring is locked. Provision credentials that don't depend on
  a login instead — a `GH_TOKEN` in the service's `Environment=`, or a passphrase-less ssh key
  loaded into a user `ssh-agent` service.

The env value changes per login, so this must run each login (an `exec-once` handles that).
Backend restart is safe (see §1), so it won't drop live sessions.
