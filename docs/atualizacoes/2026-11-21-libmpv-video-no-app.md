---
id: 2026-11-21-libmpv-video-no-app
titulo: Os vídeos da conversa passam a tocar dentro do app nativo
comando_posix: ./scripts/install-libmpv.sh
comando_windows: powershell -NoProfile -ExecutionPolicy Bypass -File scripts\install-libmpv.ps1
prova: ~/.hangar/native/libmpv-ok
destrutivo: false
tipo: maquina
---

O app nativo toca os vídeos da galeria da conversa na própria janela, pela libmpv. No Windows a
atualização baixa a libmpv e a põe ao lado do `Hangar.exe`; no macOS ela é instalada pelo Homebrew,
quando ele existe. No Linux ela vem do gerenciador de pacotes da distro (`mpv`, `libmpv2` ou
`mpv-libs`), e a atualização só consegue instalar quando o `sudo` não pede senha; senão o aviso diz
o comando para rodar no terminal. Sem a libmpv, o vídeo continua abrindo no player do sistema.

O passo conta como feito mesmo quando a libmpv não instala, para a atualização não voltar atrás
numa máquina sem `sudo` sem senha. O motivo fica em `~/.hangar/native/libmpv-ok` e no log da
instalação (`~/.hangar/logs/privado/install.log`; no Windows,
`%LOCALAPPDATA%\hangar\logs\privado\install.log`). Para tentar de novo, rode à mão
`scripts/install-libmpv.sh` (no Windows, `scripts\install-libmpv.ps1`) ou reinstale o Hangar.
No Windows, uma `libmpv-2.dll` diferente que já esteja ao lado do `Hangar.exe` é trocada pela
versão conferida.
