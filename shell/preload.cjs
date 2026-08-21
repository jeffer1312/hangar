// Única ponte shell→página: o seletor NATIVO de pasta do CreateSessionSheet. A página continua
// sendo o front normal servido por HTTP (inclusive remoto), então a superfície exposta é mínima
// e só de leitura do que o usuário escolheu no diálogo — nada de fs/exec.
const { contextBridge, ipcRenderer } = require('electron');

contextBridge.exposeInMainWorld('hangar', {
  // Abre o diálogo nativo de diretório; resolve com o caminho absoluto ou null (cancelado).
  pickFolder: () => ipcRenderer.invoke('hangar:pick-folder'),
});
