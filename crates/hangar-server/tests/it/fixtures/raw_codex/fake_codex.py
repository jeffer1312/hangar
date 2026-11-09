"""Fake Codex TUI for the captures: paints a real `capture-pane` fixture of codex-cli and reacts to keys.

argv: <fixture> <logdir> <mode>
  mode "echo":   the last line starting with "› " is the composer. Printable bytes go into it (the
                 placeholder disappears), Backspace/C-u edit, Enter submits (logged to submitted.jsonl)
                 and the composer goes back to the placeholder. Bracketed paste is enabled, like the
                 real TUI; inside a paste, CR/LF become a newline in the composer.
  mode "static": the screen never changes (menus, trust dialog); keys are only logged.
Every read from the tty is logged to keys.jsonl. Shortcut: no slash popup, no working state, no
history redraw; enough for the Python's proofs, which only read the composer line.
"""
import json
import os
import sys
import termios
import tty

fixture, logdir, mode = sys.argv[1], sys.argv[2], sys.argv[3]
lines = open(fixture, encoding="utf-8").read().split("\n")[:45]
comp = max((i for i, l in enumerate(lines) if l.startswith("› ")), default=None) if mode == "echo" else None
buf = ""
in_paste = False
out = sys.stdout


def log(name, obj):
    with open(os.path.join(logdir, name), "a", encoding="utf-8") as f:
        f.write(json.dumps(obj, ensure_ascii=False) + "\n")


def render():
    parts = ["\x1b[?2004h\x1b[H\x1b[2J"]
    for i, line in enumerate(lines):
        if i == comp and buf:
            line = "› " + buf.replace("\n", " ")
        parts.append(f"\x1b[{i + 1};1H{line}")
    if comp is not None:
        parts.append(f"\x1b[{comp + 1};{3 + len(buf.replace(chr(10), ' '))}H")
    out.write("".join(parts))
    out.flush()


def feed(text):
    global buf, in_paste
    i = 0
    while i < len(text):
        if text.startswith("\x1b[200~", i):
            in_paste, i = True, i + 6
            continue
        if text.startswith("\x1b[201~", i):
            in_paste, i = False, i + 6
            continue
        ch = text[i]
        i += 1
        if mode != "echo":
            if ch == "\r":
                log("submitted.jsonl", {"enter_on_static_screen": os.path.basename(fixture)})
            continue
        if ch in "\r\n":
            # Inside a paste CR/LF are text; outside, C-j (LF) inserts a newline and Enter (CR) submits.
            if in_paste or ch == "\n":
                buf += "\n"
            else:
                log("submitted.jsonl", {"text": buf})
                buf = ""
        elif ch in "\x7f\x08":
            buf = buf[:-1]
        elif ch == "\x15":
            buf = ""
        elif ch == "\x1b":
            # bare Escape or an unknown sequence: skip the CSI tail, if any
            if i < len(text) and text[i] == "[":
                i += 1
                while i < len(text) and not ("@" <= text[i] <= "~"):
                    i += 1
                i += 1
        elif ch >= " " or ch == "\t":
            buf += ch
    render()


tty.setraw(0)
render()
try:
    while True:
        data = os.read(0, 65536)
        if not data:
            break
        text = data.decode("utf-8", "replace")
        log("keys.jsonl", {"read": text})
        feed(text)
finally:
    termios.tcsetattr(0, termios.TCSADRAIN, termios.tcgetattr(0))
