// Única ponte shell→página: o seletor NATIVO de pasta do CreateSessionSheet. A página continua
// sendo o front normal servido por HTTP (inclusive remoto), então a superfície exposta é mínima
// e só de leitura do que o usuário escolheu no diálogo — nada de fs/exec.
const { contextBridge, ipcRenderer } = require('electron');

contextBridge.exposeInMainWorld('hangar', {
  // Abre o diálogo nativo de diretório; resolve com o caminho absoluto ou null (cancelado).
  pickFolder: () => ipcRenderer.invoke('hangar:pick-folder'),
  // Navegador embutido (WebContentsView no main). `bounds` vai por send, não invoke: dispara a
  // cada frame de resize e não precisa de resposta. O view não tem preload — o site aberto nele
  // NUNCA recebe esta ponte; só o cockpit tem.
  nav: {
    open: (url, bounds) => ipcRenderer.invoke('hangar:nav-open', { url, bounds }),
    bounds: (b) => ipcRenderer.send('hangar:nav-bounds', b),
    reload: () => ipcRenderer.send('hangar:nav-reload'),
    close: () => ipcRenderer.send('hangar:nav-close'),
  },
});
