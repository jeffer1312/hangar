---
id: 2026-11-12-atalhos-windows-current
titulo: Atalhos do Hangar no Windows passam a seguir a versão instalada
comando_windows: powershell -ExecutionPolicy Bypass -File install.ps1 -Update && findstr /l /c:"current" "%USERPROFILE%\.local\bin\hangar-send.cmd" >nul && findstr /l /c:"current" "%USERPROFILE%\.local\bin\hangar-send" >nul && findstr /l /c:"current" "%USERPROFILE%\.local\bin\hangar-preview.cmd" >nul
prova: ~/.local/bin/hangar-send ~/.local/bin/hangar-send.cmd ~/.local/bin/hangar-preview.cmd
tipo: maquina
destrutivo: false
---

No Windows, o `hangar-send`, o `hangar-preview` e os lançadores do Codex continuavam presos à versão
em que o Hangar foi instalado e paravam de funcionar quando essa versão era removida. Agora eles
seguem a versão instalada.
