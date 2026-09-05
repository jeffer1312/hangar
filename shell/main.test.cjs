// main.cjs requer 'electron' (que fora do binário do Electron não é um módulo utilizável — ver
// node_modules/electron/index.js) e sobe janela/servidor real no boot. Pra exercitar a guarda de
// identidade do item 1 sem nada disso: hijack do require.cache ANTES de exigir main.cjs, trocando
// 'electron' por um mock mínimo e o `criarControlador` de preview_ctl.cjs por uma fábrica que
// devolve controladores espiáveis (`fechado`). requestSingleInstanceLock() devolve false, então o
// branch de whenReady/criarJanela/subirServidor nunca roda — só os handlers de IPC (registrados
// ANTES da trava) importam aqui, que é onde soltarControlador vive.
const test = require('node:test');
const assert = require('node:assert/strict');
const path = require('node:path');
const os = require('node:os');
const fs = require('node:fs');

// gravarSidecarNav (fire-and-forget dentro do handler nav-open) escreve em ~/.hangar/nav — sem
// isolar o HOME antes do require, o teste sujaria o diretório real do usuário.
const homeOriginal = process.env.HOME;
process.env.HOME = fs.mkdtempSync(path.join(os.tmpdir(), 'hangar-main-test-'));
test.after(() => { process.env.HOME = homeOriginal; });

const criadas = [];
function criarControladorFalso() {
  const c = { enfileirar: (fn) => fn(), fechado: false };
  c.fechar = () => { c.fechado = true; };
  criadas.push(c);
  return c;
}
const previewCtlPath = require.resolve('./preview_ctl.cjs');
require.cache[previewCtlPath] = {
  id: previewCtlPath, filename: previewCtlPath, loaded: true,
  exports: { criarControlador: criarControladorFalso },
};

function criarWebContentsFalso() {
  const dbg = {
    attached: false,
    isAttached: () => dbg.attached,
    attach: () => { dbg.attached = true; },
    detach: () => { dbg.attached = false; },
    sendCommand: async () => ({}),
    on: () => {},
  };
  return {
    setUserAgent: () => {}, getUserAgent: () => 'UA',
    setWindowOpenHandler: () => {}, loadURL: async () => {}, getURL: () => '',
    capturePage: async () => ({ toPNG: () => Buffer.alloc(0) }),
    close: () => {}, isDestroyed: () => false,
    on: () => {}, once: () => {},
    debugger: dbg,
  };
}
const viewsFalsos = [];
class WebContentsViewFalso {
  constructor() { this.webContents = criarWebContentsFalso(); this.visivel = null; viewsFalsos.push(this); }
  setVisible(v) { this.visivel = v; }
  getVisible() { return this.visivel === true; }
  setBounds() {}
}

const winMap = new Map();
class BrowserWindowFalso {}
BrowserWindowFalso.fromWebContents = (wc) => winMap.get(wc) || null;
BrowserWindowFalso.getAllWindows = () => [];

const handlers = new Map();
const ipcMainFalso = {
  handle(nome, fn) { handlers.set(nome, fn); },
  on(nome, fn) { handlers.set(nome, fn); },
};

const electronPath = require.resolve('electron');
require.cache[electronPath] = {
  id: electronPath, filename: electronPath, loaded: true,
  exports: {
    app: {
      commandLine: { appendSwitch() {} },
      requestSingleInstanceLock: () => false,
      quit() {}, on() {}, getPath: () => '', userAgentFallback: '',
    },
    BrowserWindow: BrowserWindowFalso,
    WebContentsView: WebContentsViewFalso,
    dialog: {}, ipcMain: ipcMainFalso, screen: {}, shell: {},
  },
};

require('./main.cjs');

function novaJanela() {
  const win = { contentView: { addChildView() {}, removeChildView() {} } };
  const webContents = {}; // identidade distinta representando o remetente do IPC desta janela
  winMap.set(webContents, win);
  return { ev: { sender: webContents } };
}

test('fechar o painel de uma janela nao mata o controlador vivo da MESMA sessao aberta noutra janela', async () => {
  const a = novaJanela();
  const b = novaJanela();
  const chave = 'srv::mesma-sessao';
  const abrir = handlers.get('hangar:nav-open');
  const fechar = handlers.get('hangar:nav-close');
  assert.ok(abrir && fechar, 'handlers hangar:nav-open e hangar:nav-close registrados');

  // Duas janelas abrem a MESMA chave — a segunda sobrescreve a entrada da primeira no Map
  // global `controladores` (é a situação que o item 1 descreve).
  await abrir(a.ev, { chave, url: 'https://a.test', bounds: {} });
  await abrir(b.ev, { chave, url: 'https://b.test', bounds: {} });
  assert.equal(criadas.length, 2, 'um controlador por open');
  const ctlB = criadas[1];

  // Janela A fecha o SEU painel dessa sessão. Sem a guarda de identidade, soltarControlador
  // apagava incondicionalmente `controladores.get(chave)` — que agora é o controlador de B.
  fechar(a.ev, { chave });
  assert.equal(ctlB.fechado, false, 'controlador de B sobrevive ao close de A (bug do item 1)');

  // Fechar B de verdade ainda funciona — a guarda não deixa a entrada travada pra sempre.
  fechar(b.ev, { chave });
  assert.equal(ctlB.fechado, true, 'o proprio close de B fecha o controlador de B');
});

test('open oculto cria o view escondido e ja dirigivel; view visivel nao e tocado', async () => {
  const a = novaJanela();
  const chave = 'srv::fora-da-tela';
  const abrir = handlers.get('hangar:nav-open');
  const antes = criadas.length;

  // Sessão fora da tela: o pedido chega pela lista, o view nasce escondido, com controlador.
  const r1 = await abrir(a.ev, { chave, url: 'https://x.test', bounds: {}, oculto: true });
  assert.equal(r1.ok, true);
  assert.equal(criadas.length, antes + 1, 'controlador criado mesmo escondido (o agente dirige via CDP)');
  const view = viewsFalsos.at(-1);
  assert.equal(view.visivel, false, 'nasce escondido');

  // O usuário abre a sessão: o painel reexibe sem url, como sempre.
  const r2 = await abrir(a.ev, { chave, url: undefined, bounds: { x: 1, y: 1, width: 10, height: 10 } });
  assert.equal(r2.ok, true);
  assert.equal(view.visivel, true);

  // Outro pedido oculto com o painel montado não esconde nem recria.
  const r3 = await abrir(a.ev, { chave, url: 'https://y.test', bounds: {}, oculto: true });
  assert.equal(r3.ok, true);
  assert.equal(view.visivel, true, 'view visível fica como está');
  assert.equal(criadas.length, antes + 1, 'sem controlador novo');
});
