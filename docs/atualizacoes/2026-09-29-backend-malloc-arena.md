---
id: 2026-09-29-backend-malloc-arena
titulo: No Linux, o backend deixa de acumular memória ao longo do dia
comando_posix: ! test -f ~/.config/systemd/user/hangar-backend.service || ./install.sh --update
prova: scripts/services-setup.sh
destrutivo: true
tipo: maquina
---

O serviço do Hangar no Linux crescia centenas de megabytes em poucas horas e empurrava a máquina
para o swap. Agora ele segura bem menos memória parada. Vale para quem roda o backend como
serviço do systemd; quem roda na mão não é tocado. A atualização reescreve o serviço e reinicia o
backend: sessões sem terminal abertas nesse momento são interrompidas, e as conexões do celular
caem por alguns segundos.
