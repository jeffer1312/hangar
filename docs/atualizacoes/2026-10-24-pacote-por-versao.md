---
id: 2026-10-24-pacote-por-versao
titulo: O Hangar passa a se atualizar por pacote, sem git, com volta para a versão anterior
comando_posix: bash scripts/migrar-pacote.sh
comando_windows: powershell -NoProfile -ExecutionPolicy Bypass -File scripts\migrar-pacote.ps1
prova: ~/.hangar/migration.json
destrutivo: true
tipo: maquina
---

Depois desta atualização o Hangar roda de uma versão pronta em `~/.hangar/current`, sem `git pull`,
e volta sozinho para a versão anterior se a nova não subir. A senha do celular, as máquinas
pareadas e os projetos passam para `~/.hangar/config`.

O seu checkout do Hangar continua no disco, mas deixa de ser o que roda. Para editar o código ou
abrir PR, ligue **Modo desenvolvedor** nas configurações deste servidor: o Hangar volta a rodar do
seu checkout. Com mudanças locais no checkout, a máquina já entra direto no modo desenvolvedor.
Para desfazer: `hangar-server migrate-package --undo`.
