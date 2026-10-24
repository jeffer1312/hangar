---
id: 2026-09-29-lancador-electron-renomeado
titulo: No menu de apps, a janela Electron passa a se chamar "Hangar (Electron)"
comando_posix: f="${XDG_DATA_HOME:-$HOME/.local/share}/applications/hangar.desktop"; [ ! -f "$f" ] || { sed -i 's/^Name=Hangar\r\?$/Name=Hangar (Electron)/' "$f" && ! tr -d '\r' < "$f" | grep -qx 'Name=Hangar'; } && mkdir -p ~/.hangar/native && touch ~/.hangar/native/lancador-electron-renomeado
prova: ~/.hangar/native/lancador-electron-renomeado
destrutivo: false
tipo: maquina
---

Com o app nativo instalado, o menu de apps mostrava dois itens chamados "Hangar". Agora "Hangar" é
o app nativo e a janela Electron aparece como "Hangar (Electron)".
