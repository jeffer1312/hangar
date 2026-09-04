// Única ponte shell→página: o seletor NATIVO de pasta do CreateSessionSheet. A página continua
// sendo o front normal servido por HTTP (inclusive remoto), então a superfície exposta é mínima
// e só de leitura do que o usuário escolheu no diálogo — nada de fs/exec.
const { contextBridge, ipcRenderer } = require('electron');
const path = require('path');
const { commitDoCheckout } = require('./versao.cjs');

contextBridge.exposeInMainWorld('hangar', {
  // Commit do checkout que ESTE shell carregou (lido uma vez, na abertura). A tela de atualização
  // compara com o commit atualizado: igual = a janela já é a nova, o "feche e abra" não vale mais.
  shellCommit: commitDoCheckout(path.join(__dirname, '..')),
  // Fecha e reabre o app (o restart do backend não alcança o main.cjs já carregado).
  relaunch: () => ipcRenderer.invoke('hangar:relaunch'),
  // Abre o diálogo nativo de diretório; resolve com o caminho absoluto ou null (cancelado).
  pickFolder: () => ipcRenderer.invoke('hangar:pick-folder'),
  // Navegador embutido (WebContentsView no main), UM POR SESSÃO: a chave é serverId::nome.
  // hide (não close) na troca de sessão — o agente segue dirigindo o view escondido via CDP.
  // `bounds` vai por send, não invoke: dispara a cada frame de resize e não precisa de resposta.
  // O view não tem preload — o site aberto nele NUNCA recebe esta ponte; só o cockpit tem.
  nav: {
    open: (chave, url, bounds) => ipcRenderer.invoke('hangar:nav-open', { chave, url, bounds }),
    hide: (chave) => ipcRenderer.send('hangar:nav-hide', { chave }),
    bounds: (chave, b) => ipcRenderer.send('hangar:nav-bounds', { chave, bounds: b }),
    reload: (chave) => ipcRenderer.send('hangar:nav-reload', { chave }),
    close: (chave) => ipcRenderer.send('hangar:nav-close', { chave }),
  },
});
