---
id: 2026-11-17-atalhos-python-removidos
titulo: Os atalhos do Python do Hangar saem da máquina; os comandos do Hangar rodam sem Python
comando_posix: s=; for f in "$HOME/.local/bin/hangar-python" "$HOME/.local/bin/python3" "$HOME/.hangar/cache/install-bin/python3"; do if [ -L "$f" ]; then case $(readlink "$f") in */hangar-python) rm -f "$f" || s="$s $f" ;; esac; elif [ -f "$f" ] && head -n 1 "$f" | grep -qx '#!/bin/sh' && grep -q 'scripts/hangar-python' "$f"; then rm -f "$f" || s="$s $f"; fi; done; mkdir -p ~/.hangar/passos && printf 'sobrou:%s\n' "$s" > ~/.hangar/passos/2026-11-17-atalhos-python-removidos
comando_windows: powershell -NoProfile -ExecutionPolicy Bypass -Command "$b = Join-Path $HOME '.local\bin'; $s = @(); function Tira($n) { try { Remove-Item -LiteralPath (Join-Path $b $n) -Force -ErrorAction Stop } catch { $script:s += $n } }; function Tem($n, $p) { $f = Join-Path $b $n; (Test-Path -LiteralPath $f -PathType Leaf) -and @($p | Where-Object { -not (Select-String -Quiet -SimpleMatch -LiteralPath $f -Pattern $_) }).Count -eq 0 }; if (Tem 'python3' @('Gerado por hangar/install.ps1')) { Tira 'python3' }; $c = Join-Path $b 'python3.cmd'; if (Test-Path -LiteralPath $c -PathType Leaf) { $l = @(Get-Content -LiteralPath $c); if ($l.Count -eq 2 -and $l[0] -eq '@echo off' -and $l[1] -match '^\x22[^\x22]+\.exe\x22( -3)? \x25\*$') { Tira 'python3.cmd' } }; if (Tem 'hangar-python.cmd' @('.hangar\current', '\python\python.exe', '\backend\.venv\Scripts\python.exe')) { Tira 'hangar-python.cmd' }; if (Tem 'hangar-python' @('Gerado pelo instalador do hangar', 'scripts/hangar-python')) { Tira 'hangar-python' }; $m = Join-Path $HOME '.hangar\passos'; New-Item -ItemType Directory -Force $m | Out-Null; Set-Content -LiteralPath (Join-Path $m '2026-11-17-atalhos-python-removidos') -Value ('sobrou: ' + ($s -join ' '))"
prova: ~/.hangar/passos/2026-11-17-atalhos-python-removidos
destrutivo: true
tipo: maquina
---

Os comandos do Hangar no terminal (`hangar-send`, `hangar-conta`, `hangar-open` e os do Codex)
passaram a rodar pelo próprio servidor do Hangar, sem Python. Os atalhos `hangar-python` e o
`python3` que o instalador do Hangar tinha criado saem da máquina; um Python instalado por você
não é tocado.
