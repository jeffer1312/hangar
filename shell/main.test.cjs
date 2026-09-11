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
function criarControladorFalso(opcoes) {
  const c = { enfileirar: (fn) => fn(), fechado: false, opcoes, ocultos: [] };
  c.fechar = () => { c.fechado = true; };
  c.definirOculto = async (v) => { c.ocultos.push(v); };
  criadas.push(c);
  return c;
}
const previewCtlPath = require.resolve('./preview_ctl.cjs');
require.cache[previewCtlPath] = {
  id: previewCtlPath, filename: previewCtlPath, loaded: true,
  exports: { criarControlador: criarControladorFalso },
};

function criarWebContentsFalso() {
  // `url` começa vazia como no view recém-criado; dispararLoad() simula o did-finish-load.
  const estado = { url: '', ouvintesLoad: [] };
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
    setWindowOpenHandler: () => {}, loadURL: async () => {}, getURL: () => estado.url,
    capturePage: async () => ({ isEmpty: () => false, toPNG: () => Buffer.alloc(0) }),
    close: () => {}, isDestroyed: () => false,
    on: (ev, cb) => { if (ev === 'did-finish-load') estado.ouvintesLoad.push(cb); },
    once: (ev, cb) => { if (ev === 'did-finish-load') estado.ouvintesLoad.push(cb); },
    removeListener: () => {},
    dispararLoad: () => { estado.url = 'https://z.test/'; estado.ouvintesLoad.splice(0).forEach((cb) => cb()); },
    debugger: dbg,
  };
}
const viewsFalsos = [];
class WebContentsViewFalso {
  constructor() { this.webContents = criarWebContentsFalso(); this.visivel = null; this.bounds = null; viewsFalsos.push(this); }
  setVisible(v) { this.visivel = v; }
  getVisible() { return this.visivel === true; }
  setBounds(b) { this.bounds = b; }
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

function novaJanela({ focada = true } = {}) {
  const webContents = { focos: 0, focus() { this.focos++; } }; // identidade distinta: o remetente do IPC desta janela
  const children = [];
  const emFoco = [];
  const win = {
    contentView: {
      children,
      addChildView(v) { children.push(v); },
      removeChildView(v) { const i = children.indexOf(v); if (i >= 0) children.splice(i, 1); },
    },
    webContents, isDestroyed: () => false,
    isFocused: () => focada,
    once(ev, fn) { if (ev === 'focus') emFoco.push(fn); },
    pendencias: () => emFoco.length,
    // O usuário volta pro app: a janela ganha foco e os pendentes disparam uma vez só.
    ganharFoco() { focada = true; const fns = emFoco.splice(0); for (const fn of fns) fn(); },
  };
  winMap.set(webContents, win);
  return { ev: { sender: webContents }, win };
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
  assert.deepEqual(r1, { ok: true, oculto: true }, 'ecoa `oculto`: é a prova que o front exige antes de confirmar');
  assert.equal(criadas.length, antes + 1, 'controlador criado mesmo escondido (o agente dirige via CDP)');
  const view = viewsFalsos.at(-1);
  assert.equal(view.visivel, false, 'nasce escondido');
  // É o primeiro load de um view JÁ na janela que leva o teclado (medido: anexar depois não leva).
  // Escondido, ele carrega solto e só entra na janela com a página pronta.
  assert.ok(!a.win.contentView.children.includes(view), 'escondido carrega FORA da janela');
  assert.equal(a.win.webContents.focos, 1, 'o foco volta pro front logo que o view escondido nasce');
  view.webContents.dispararLoad();
  assert.ok(a.win.contentView.children.includes(view), 'com a página pronta entra na janela (sem isso não há print)');
  assert.equal(a.win.webContents.focos, 2, 'e o foco volta de novo no load (navegação cross-site ainda rouba)');

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

test('com o app em segundo plano o view escondido nao puxa a tela: o teclado volta quando a janela e focada', async () => {
  const a = novaJanela({ focada: false });
  const abrir = handlers.get('hangar:nav-open');

  await abrir(a.ev, { chave: 'srv::longe', url: 'https://z.test', bounds: {}, oculto: true });
  const view = viewsFalsos.at(-1);
  view.webContents.dispararLoad();
  // `focus()` numa janela de fundo é pedido de ativação: com focus_on_activate o compositor
  // traz o app pra frente e rouba a tela de quem está noutro aplicativo.
  assert.equal(a.win.webContents.focos, 0, 'janela em segundo plano nao e ativada');
  // Nascimento e load pediram o teclado; o view dirigido por CDP navega muitas vezes antes de o
  // usuário voltar, e uma pendência por pedido empilharia listeners.
  assert.equal(a.win.pendencias(), 1, 'uma pendencia por janela, nao uma por pedido');

  a.win.ganharFoco();
  assert.equal(a.win.webContents.focos, 1, 'o teclado volta pro front quando o usuario volta');

  a.win.ganharFoco();
  assert.equal(a.win.webContents.focos, 1, 'pendencia so dispara uma vez');
});

test('pendencia nao arranca o teclado do painel que o usuario abriu enquanto esteve fora', async () => {
  const a = novaJanela({ focada: false });
  const abrir = handlers.get('hangar:nav-open');
  const chave = 'srv::abriu-depois';

  await abrir(a.ev, { chave, url: 'https://w.test', bounds: {}, oculto: true });
  const view = viewsFalsos.at(-1);
  // O painel dessa sessão é montado com a janela ainda em segundo plano (o IPC vem do renderer,
  // não exige foco): o view vira visível com a pendência já agendada.
  await abrir(a.ev, { chave, url: undefined, bounds: { x: 0, y: 0, width: 10, height: 10 } });
  assert.equal(view.getVisible(), true, 'painel montado');

  a.win.ganharFoco();
  assert.equal(a.win.webContents.focos, 0, 'o teclado fica com o navegador que esta na tela');
});

test('o controlador so e avisado do view escondido DEPOIS de a pagina carregar (senao, SIGSEGV)', async () => {
  const a = novaJanela();
  const abrir = handlers.get('hangar:nav-open');
  await abrir(a.ev, { chave: 'srv::viewport', url: 'https://z.test', bounds: {}, oculto: true });
  const view = viewsFalsos.at(-1);
  const ctl = criadas.at(-1);
  assert.deepEqual(ctl.ocultos, [], 'view recem-criado esta em about:blank — avisar aqui derruba o processo');

  view.webContents.dispararLoad();
  assert.deepEqual(ctl.ocultos, [true], 'carregou, agora sim');

  // O usuário abre a sessão: o painel monta e a emulação sai na hora (a página já carregou).
  await abrir(a.ev, { chave: 'srv::viewport', bounds: { x: 0, y: 0, width: 800, height: 600 } });
  assert.deepEqual(ctl.ocultos, [true, false]);

  // Trocar de sessão esconde o painel: o agente que continuar dirigindo precisa da emulação de volta.
  handlers.get('hangar:nav-hide')(a.ev, { chave: 'srv::viewport' });
  assert.deepEqual(ctl.ocultos, [true, false, true]);
});
