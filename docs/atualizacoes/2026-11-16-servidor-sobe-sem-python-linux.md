---
id: 2026-11-16-servidor-sobe-sem-python-linux
titulo: No Linux, o servidor do Hangar sobe sem o Python e o serviço nunca desiste de religar
comando_posix: { ! test -f ~/.config/systemd/user/hangar-backend.service || { ./install.sh --update && grep -qx 'StartLimitIntervalSec=0' ~/.config/systemd/user/hangar-backend.service && grep -qx 'KillMode=control-group' ~/.config/systemd/user/hangar-backend.service && grep -qx 'TimeoutStopSec=120' ~/.config/systemd/user/hangar-backend.service; }; } && mkdir -p ~/.hangar/passos && touch ~/.hangar/passos/2026-11-16-servidor-sobe-sem-python-linux
prova: ~/.hangar/passos/2026-11-16-servidor-sobe-sem-python-linux
destrutivo: true
tipo: maquina
---

O serviço do Hangar passa a abrir o servidor direto, sem o Python no meio, e volta sozinho sempre
que cair, esperando cada vez mais entre as tentativas. Se o servidor travar sem cair, o sistema o
reinicia. A atualização reescreve o serviço e reinicia o servidor: as conexões do celular caem por
alguns segundos.
