---
id: 2026-10-24-pacote-por-versao
titulo: O Hangar passa a se atualizar por pacote, com volta para a versão anterior
comando_posix: bash scripts/migrar-pacote.sh
comando_windows: powershell -NoProfile -ExecutionPolicy Bypass -File scripts\migrar-pacote.ps1
prova: ~/.hangar/migration-offered
destrutivo: true
tipo: maquina
---

O Hangar deixa de se atualizar por `git pull`. Cada versão chega pronta em `~/.hangar/current`, e se
a nova não subir ele volta sozinho para a anterior. A senha do celular, as máquinas pareadas e os
projetos passam para `~/.hangar/config`. A porta da tela continua a mesma.

**Pacote (recomendado):** a máquina roda a versão publicada, e o Atualizar troca de versão. O seu
checkout continua no disco como está, com as suas edições, mas deixa de ser o que roda.

**Modo desenvolvedor:** o servidor roda direto do seu checkout (o backend Python, os scripts, as
skills e o servidor em Rust que você compilou em `crates/target/release`), e o Atualizar passa a
fazer `git pull` nele. Só vale a pena se você mexe no código do servidor do Hangar e quer que esta
máquina rode o que você editou. Para testar o app nativo não precisa: abra o que você compilou. Dá
para trocar depois, a qualquer momento, em Configurações › Sobre.

Para desfazer tudo: `hangar-server migrate-package --undo`.
