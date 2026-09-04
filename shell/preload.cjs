// Única ponte shell→página: o seletor NATIVO de pasta do CreateSessionSheet. A página continua
// sendo o front normal servido por HTTP (inclusive remoto), então a superfície exposta é mínima
// e só de leitura do que o usuário escolheu no diálogo — nada de fs/exec.
const { contextBridge, ipcRenderer } = require('electron');

// Commit que o main.cjs carregou, vindo por argv (ver `additionalArguments` no main). Não é lido
// do .git aqui de propósito: este arquivo roda de novo a cada reload da página e veria o checkout
// já atualizado com o main ainda velho. Main sem o flag (mais velho que este preload) = null.
const shellCommit = (() => {
  const arg = process.argv.find((a) => a.startsWith('--hangar-shell-commit='));
  const v = arg ? arg.slice('--hangar-shell-commit='.length) : '';
  return /^[0-9a-f]{40}$/.test(v) ? v : null;
})();

contextBridge.exposeInMainWorld('hangar', {
  // A tela de atualização compara com o commit atualizado: igual = a janela já é a nova, o
  // "feche e abra o Hangar" não vale mais.
  shellCommit,
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
    // Cookies do Chrome real (CDP) -> partição do view. Resolve sempre; erro vem no objeto.
    importCookies: (chave, host, porta) => ipcRenderer.invoke('hangar:nav-import-cookies', { chave, host, porta }),
  },
});
