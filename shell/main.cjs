// Janela nativa do hangar. Ver docs/superpowers/specs/2026-08-05-shell-electron-design.md.
const { app, BrowserWindow, WebContentsView, dialog, ipcMain, screen, shell } = require('electron');
const path = require('path');
const fs = require('fs');
const os = require('os');
const { ler, gravar } = require('./settings.cjs');
const { uaDeChrome, normalizaBounds, urlNavegavel, nomeSidecar } = require('./navegador.cjs');

// O navegador embutido (WebContentsView, handlers hangar:nav-*) é dirigível por CDP na 9223 — o
// agent-browser conecta nela como conecta no Chrome do usuário (9222), sem backend no meio. A
// porta expõe TODOS os webContents, inclusive o cockpit (com o token no localStorage), a qualquer
// processo local — mesmo risco do Chrome com remote-debugging, assumido de propósito.
app.commandLine.appendSwitch('remote-debugging-port', '9223');

// MEDIDO 05/08/2026 (Hyprland/Wayland, Electron 43.3.0): o switch `enable-transparent-visuals`
// do spike NAO e necessario — a janela continua transparente sem ele. Linha removida.

const PADRAO = 'http://127.0.0.1:8765';

// Transparência por plataforma. No Windows `transparent: true` só funciona sem moldura (doc da
// própria opção) e a gente mantém a moldura nativa -> lá o caminho é backgroundMaterial, que o
// sistema desenha inclusive atrás da barra de título. No Windows 10, sem material, janela opaca.
function opcoesDeFundo() {
  if (process.platform === 'darwin') {
    return { transparente: true, extra: { vibrancy: 'under-window', visualEffectState: 'followWindow' } };
  }
  if (process.platform === 'win32') {
    return { transparente: false, extra: { backgroundMaterial: 'acrylic' } };
  }
  return { transparente: true, extra: {} };   // linux
}

async function paginaResponde(url) {
  // Critério é `GET /` devolver 200 text/html — NÃO um probe na API: o backend pode estar vivo
  // sem o dist (o mount é condicional, backend/app/api.py:3151-3153) e o usuário veria 404.
  try {
    const res = await fetch(url, { signal: AbortSignal.timeout(3000) });
    return res.ok && (res.headers.get('content-type') || '').includes('text/html');
  } catch {
    return false;
  }
}

// `dialog.showMessageBox` NÃO tem campo de texto — só botões. Pedir URL por ele devolveria sempre
// a mesma URL, e o atalho de recuperação recarregaria o mesmo endereço pra sempre. A tela é uma
// página `data:` carregada na própria janela: tem `<input>`, vem preenchida com a URL atual, e o
// submit navega. Sem preload, sem IPC, sem dependência.
function telaDeUrl(atual) {
  // `atual` vira valor de atributo HTML — escapa o mínimo que já basta pra não quebrar fora do
  // `value="..."` (uma URL com `"` fecharia o atributo cedo).
  const segura = String(atual).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/"/g, '&quot;');
  const html = `<!doctype html><meta charset="utf-8">
<style>body{background:#1a181d;color:#e8e6e3;font:14px system-ui;display:grid;place-items:center;height:100vh;margin:0}
form{display:flex;gap:8px;flex-direction:column;width:min(90vw,420px)}
input{padding:10px;border-radius:8px;border:1px solid #3a373f;background:#232028;color:inherit}
button{padding:10px;border-radius:8px;border:0;background:#6b5bd6;color:#fff;cursor:pointer}
p{opacity:.7;margin:0 0 4px}</style>
<form onsubmit="location.href=this.u.value;return false">
  <p>Não consegui carregar a interface. Qual o endereço do seu cockpit?</p>
  <input name="u" value="${segura}" autofocus>
  <button>Abrir</button>
</form>`;
  return 'data:text/html;charset=utf-8,' + encodeURIComponent(html);
}

function geometriaValida(j) {
  if (!j) return null;
  // Reencaixa na tela mais próxima: sem isto a janela renasce invisível num monitor desligado.
  const tela = screen.getDisplayMatching(j);
  const a = tela.workArea;
  return {
    x: Math.min(Math.max(j.x, a.x), a.x + a.width - 200),
    y: Math.min(Math.max(j.y, a.y), a.y + a.height - 100),
    width: Math.min(j.width, a.width),
    height: Math.min(j.height, a.height),
  };
}

// UMA instância, VÁRIAS janelas. Duas instâncias sobre o MESMO userData não funcionam: o
// armazenamento do Chromium (localStorage, onde moram o token e a aparência) só abre num
// processo. Medido em 18/08/2026, com duas janelas abertas sobre uma cópia do perfil real: a
// segunda subiu com `Failed to delete the database: Database IO error` e caiu na tela de login,
// sem papel de parede — parecia "o app não pegou as configs", e era o perfil trancado.
// Com a trava, o segundo lançamento não vira processo: ele avisa esta instância, que abre mais
// uma janela no mesmo perfil (mesmo token, mesma aparência, mesmas sessões).
async function criarJanela() {
  const dir = app.getPath('userData');
  const cfg = ler(dir);
  // Precedência: variável de ambiente > escolha salva > padrão.
  const url = process.env.COCKPIT_URL || cfg.url || PADRAO;
  // Ultimo endereco que CARREGOU de verdade (did-navigate abaixo mantem isto atualizado). `url`
  // acima fica congelada no valor do boot; sem esta variavel a tela de recuperacao reoferecia
  // 127.0.0.1:8765 depois que o usuario ja tinha corrigido pra outro endereco e ele caiu de novo.
  //
  // Comeca no PADRAO, NAO em `url`: `url` pode vir de COCKPIT_URL, que ninguem verificou. Medido em
  // 05/08/2026 — abrir com COCKPIT_URL apontando pra uma porta morta e fechar na tela de
  // recuperacao gravava a porta morta como "endereco bom", e TODA abertura seguinte caia na tela de
  // erro. Endereco que nunca carregou nao pode ser o ultimo bom.
  let urlBoa = PADRAO;

  const fundo = opcoesDeFundo();
  const geo = geometriaValida(cfg.janela) || { width: 1280, height: 800 };
  // Janela nova em cima de janela aberta nasce EXATAMENTE sobre a anterior (a geometria salva é
  // uma só) e some — parece que o clique não fez nada. Desloca em cascata por janela já aberta e
  // reencaixa na área de trabalho, pela mesma régua do `geometriaValida`.
  const jaAbertas = BrowserWindow.getAllWindows().length;
  if (jaAbertas > 0) {
    const desloc = geometriaValida({ ...geo, x: (geo.x ?? 0) + 32 * jaAbertas, y: (geo.y ?? 0) + 32 * jaAbertas });
    if (desloc) { geo.x = desloc.x; geo.y = desloc.y; }
  }

  // A marca que o front lê (frontend/src/lib/background.ts, isShell) SÓ é injetada quando a janela
  // vai mesmo ser transparente. Sem esta condição, numa plataforma opaca (Windows 10, GNOME sem
  // extensão) a tela de Aparência ofereceria "Desktop" e escolher isso zeraria o fundo de uma
  // janela sólida — o app nasceria sem fundo nenhum. Vai no user agent, e não na URL, porque
  // Login.svelte:111 apaga a query quando ela traz ?token= (pareamento).
  // `+=` uma vez só: com várias janelas, appendar a cada abertura empilharia a marca no user
  // agent ("… hangar-shell hangar-shell").
  if (fundo.transparente && !app.userAgentFallback.includes(' hangar-shell')) app.userAgentFallback += ' hangar-shell';

  const win = new BrowserWindow({
    ...geo,
    transparent: fundo.transparente,
    // Fallback opaco: onde a transparência não vale, a janela precisa de cor própria, senão
    // aparece preta. O front também não recebe a marca de fundo nesse caso (ver abaixo).
    backgroundColor: fundo.transparente ? '#00000000' : '#1a181d',
    ...fundo.extra,
    // O preload expõe SÓ window.hangar.pickFolder (seletor nativo de pasta) — ver preload.cjs.
    webPreferences: { contextIsolation: true, preload: path.join(__dirname, 'preload.cjs') },
  });
  win.removeMenu();

  // Título FIXO: é ele que a windowrulev2 do Hyprland casa (a classe é sempre 'electron' e não
  // muda — medido com app.setName e --class em 05/08/2026). preventDefault impede que a navegação
  // do SPA troque o título.
  win.setTitle('hangar');
  win.on('page-title-updated', (e) => e.preventDefault());

  // Link do chat (`target="_blank"`) sem isto abre uma BrowserWindow NOVA do proprio Electron — sem
  // barra de endereco, sem abas, sem as sessoes/extensoes do usuario. Parece "outro navegador".
  // `openExternal` manda pro navegador padrao do sistema (aqui, o Chrome).
  win.webContents.setWindowOpenHandler(({ url: alvo }) => {
    if (/^https?:/i.test(alvo)) shell.openExternal(alvo);
    return { action: 'deny' };
  });
  // Mesmo destino pra link que navega na PROPRIA janela (sem `target`): sem isto o cockpit vira um
  // navegador de uma aba so e o usuario fica preso no site externo, sem botao de voltar.
  win.webContents.on('will-navigate', (e, alvo) => {
    if (alvo.startsWith('data:')) return;                 // tela de recuperacao navega sozinha
    const atual = win.webContents.getURL();
    if (atual.startsWith('data:')) return;                // o submit da tela de recuperacao
    try {
      if (new URL(alvo).origin === new URL(atual).origin) return;   // navegacao interna do app
    } catch { return; }
    e.preventDefault();
    if (/^https?:/i.test(alvo)) shell.openExternal(alvo);
  });

  // A tela de recuperação não é endereço de cockpit: se o fechamento pegar a janela parada nela
  // (backend fora, usuário fechou em vez de responder), gravar essa URL faria a próxima abertura
  // tentar carregar a própria tela de erro. Nesse caso mantém a URL que já estava salva.
  win.on('close', () => {
    // Janela morrendo leva TODOS os views de navegador dela — sem isto o Map guardaria
    // referência de webContents mortos.
    const views = navegadores.get(win);
    if (views) {
      navegadores.delete(win);
      for (const v of views.values()) { try { v.webContents.close(); } catch { /* já morreu */ } }
      // ...e os sidecars das chaves dela, senão o CLI lista "MORTO" acumulando lixo a cada quit.
      for (const chave of views.keys()) {
        try { fs.rmSync(path.join(NAV_SIDECARS, `${nomeSidecar(chave)}.json`), { force: true }); } catch { /* sem sidecar */ }
      }
    }
    const u = win.webContents.getURL();
    // Na tela de recuperacao (data:) nao ha endereco de cockpit pra salvar — usa o ULTIMO que
    // carregou de verdade (urlBoa), nao o `cfg.url` do boot, que fica pra tras assim que o
    // usuario troca de endereco em tela.
    gravar(dir, { url: u.startsWith('data:') ? urlBoa : u,
                  janela: win.getBounds() });
  });

  // A URL que vale é a que a janela REALMENTE carregou — inclusive a digitada na tela de
  // recuperação, que navega por conta própria.
  win.webContents.on('did-navigate', (_e, u) => {
    if (u.startsWith('data:')) return;
    urlBoa = u;
    gravar(dir, { url: u, janela: win.getBounds() });
  });
  // Carga que falha (backend caiu no meio, URL errada) traz a tela de volta. Mas só se for a
  // página principal: o app mantém SSE e faz XHR de sessão o tempo todo, e um recurso solto que
  // falhou no meio de uma página que carregou bem também dispara este evento — sem o filtro, a
  // janela seria jogada pra tela de erro no meio do uso.
  win.webContents.on('did-fail-load', (_e, errorCode, _desc, validatedURL, isMainFrame) => {
    if (!isMainFrame) return;
    // -3 é ERR_ABORTED: uma navegação mais nova venceu a corrida (ex.: o duplo disparo do
    // Ctrl+Shift+U antes do guard de keyDown abaixo) — não é falha nenhuma, e recarregar a tela
    // de erro aqui é o próprio bug que o guard de baixo existe pra evitar.
    if (errorCode === -3) return;
    // Prefil com o endereço que ACABOU de falhar (não o último bom, urlBoa): é o que está
    // relevante pro que quebrou agora — inclusive pra corrigir um erro de digitação recém-feito
    // na própria tela de recuperação.
    win.loadURL(telaDeUrl(validatedURL || url));
  });

  // Atalho de recuperação. NÃO usar globalShortcut: ele registra no sistema inteiro enquanto o app
  // viver, e no Linux com IBus o Ctrl+Shift+U é a entrada de unicode. before-input-event vale só
  // com a janela em foco e funciona mesmo com removeMenu().
  win.webContents.on('before-input-event', (e, input) => {
    // before-input-event dispara pra keyDown E keyUp da MESMA tecla — sem este filtro, um
    // Ctrl+Shift+U soltava dois loadURL: o segundo aborta o primeiro (ERR_ABORTED), que sem o
    // guard acima reabria a tela de erro sozinho.
    if (input.type !== 'keyDown') return;
    if (input.control && input.shift && input.key.toLowerCase() === 'u') {
      e.preventDefault();
      win.loadURL(telaDeUrl(win.webContents.getURL() || url));
    }
    // Ctrl+Shift+R — recarregar de verdade. O `removeMenu()` acima tira o menu padrao e, com ele,
    // TODOS os aceleradores que vinham de graca: no Electron o reload nasce do role do menu, entao
    // sem menu o Ctrl+R e o Ctrl+Shift+R simplesmente nao existem (medido 10/08/2026 — o usuario
    // apertava e nada acontecia).
    //
    // E `reloadIgnoringCache()` sozinho NAO bastaria: a interface e um PWA com service worker
    // fazendo precache dos assets. O SW intercepta o fetch ANTES do cache HTTP, entao ignorar o
    // cache do Chromium ainda entrega o bundle velho — foi exatamente isso que segurou uma versao
    // antiga da tela depois de um rebuild nesta mesma sessao. Por isso limpa `serviceworkers` +
    // `cachestorage` primeiro e so entao recarrega.
    //
    // Nao mexe em cookies nem localStorage de proposito: ali moram o token de autenticacao e as
    // preferencias de aparencia — um "recarregar" que desloga o usuario e uma armadilha.
    if (input.control && input.shift && input.key.toLowerCase() === 'r') {
      e.preventDefault();
      // Desregistra o SW ANTES de limpar o armazenamento dele. `clearStorageData` apaga os bytes,
      // mas o registro vivo continua na página: ele volta a se instalar no reload e pode reservir
      // o bundle antigo. Sem isto o atalho "funcionava" e a tela continuava velha, que foi o que o
      // usuário viu — e como a promessa era engolida, nada aparecia dizendo o que houve.
      win.webContents
        .executeJavaScript(`navigator.serviceWorker?.getRegistrations?.()
            .then(rs => Promise.all(rs.map(r => r.unregister())))
            .then(rs => rs.length).catch(() => -1)`)
        .then((n) => console.log(`[recarregar] service workers desregistrados: ${n}`))
        .catch((err) => console.error('[recarregar] desregistro falhou:', err))
        .then(() => win.webContents.session.clearStorageData({
          storages: ['serviceworkers', 'cachestorage'],
        }))
        .then(() => console.log('[recarregar] cache do service worker limpo'))
        .catch((err) => console.error('[recarregar] limpeza falhou:', err))
        .finally(() => win.webContents.reloadIgnoringCache());
    }
    // Ctrl+Shift+I — DevTools. Mesmo motivo do R: o `removeMenu()` acima leva junto TODOS os
    // aceleradores padrão, e o DevTools é um deles. Sem menu, sem atalho — e sem DevTools não há
    // como conferir qual bundle a janela carregou, que é justamente o que se precisa quando a tela
    // não acompanha o build.
    if (input.control && input.shift && input.key.toLowerCase() === 'i') {
      e.preventDefault();
      win.webContents.toggleDevTools();
    }
  });

  // Endereco salvo fora do ar NAO significa perguntar: o caso comum e o backend local, que esta
  // bem ali no PADRAO. So depois que os dois falham e que faz sentido pedir ajuda pro usuario.
  let alvo = null;
  if (await paginaResponde(url)) alvo = url;
  else if (url !== PADRAO && (await paginaResponde(PADRAO))) alvo = PADRAO;
  win.loadURL(alvo ? alvo : telaDeUrl(url));
}

// A trava vem ANTES do whenReady: quem não a conseguiu não pode chegar a criar janela nenhuma —
// é justamente a janela quebrada que este bloco existe pra evitar. O processo que perdeu a trava
// sai na hora, e o `second-instance` dispara no processo que já está de pé.
// `criarJanela` e async: chamar sem catch transforma qualquer excecao dentro dela (geometria
// salva estranha, BrowserWindow que nao nasce) numa unhandled rejection. No app empacotado isso
// nao aparece em lugar nenhum — o clique pra abrir a segunda janela simplesmente nao faz nada —
// e, dependendo da politica do Node, derruba o processo inteiro, fechando as janelas que ja
// estavam abertas no meio do uso. Falha de abrir UMA janela nao pode custar as outras.
function abrirJanela(origem) {
  criarJanela().catch((err) => {
    console.error(`[janela] ${origem} falhou:`, err);
    // Segunda janela e um pedido EXPLICITO do usuario: silenciar ali e o bug. No arranque, se a
    // primeira janela nao nasce, tambem nao ha nada na tela pra explicar o que houve.
    dialog.showErrorBox('Hangar', `Não consegui abrir a janela.\n\n${err && err.message ? err.message : err}`);
  });
}

// ---------------------------------------------------------------------------
// Navegador embutido. UM WebContentsView POR SESSÃO (chave serverId::nome), pendurado no
// contentView da janela — navegação top-level, então X-Frame-Options não se aplica (diferente de
// iframe). Trocar de sessão ESCONDE o view (nav-hide), não fecha: o agente segue dirigindo ele
// via CDP em background (backgroundThrottling: false abaixo é por isso). Fechar de verdade é só
// pelo × do painel (nav-close). A POSIÇÃO é medida pelo front (div âncora no NavegadorPane) e
// chega por IPC: o view não é DOM, flutua POR CIMA da página — o front esconde com bounds zero
// quando um overlay DOM abre, e o layout do Chat reserva a faixa pra nada cobrir texto/composer.
const navegadores = new Map();   // BrowserWindow -> Map<chave, WebContentsView>

function viewsDa(win) {
  let m = navegadores.get(win);
  if (!m) { m = new Map(); navegadores.set(win, m); }
  return m;
}

function fecharNavegador(win, chave) {
  const m = navegadores.get(win);
  const view = m && m.get(chave);
  if (!view) return;
  m.delete(chave);
  if (m.size === 0) navegadores.delete(win);
  try { win.contentView.removeChildView(view); } catch { /* janela já destruída */ }
  try { view.webContents.close(); } catch { /* idem */ }
  try { fs.rmSync(path.join(NAV_SIDECARS, `${nomeSidecar(chave)}.json`), { force: true }); } catch { /* sem sidecar */ }
}

// Sidecar por sessão em ~/.hangar/nav/<chave>.json — é o que o `hangar-preview` lê pra achar o
// target CDP DESTA sessão sem adivinhar por URL (duas sessões no mesmo localhost:3000 teriam a
// mesma). O targetId é descoberto por diff do /json/list antes/depois do view nascer: opens são
// raros e seriais, então o alvo novo é o view. Gravação é async e tmp+rename (o CLI pode estar
// lendo). Sem targetId (CDP fora do ar?), grava só chave+url e o CLI casa por URL.
const NAV_SIDECARS = path.join(os.homedir(), '.hangar', 'nav');

async function gravarSidecarNav(chave, urlInicial, idsAntes) {
  for (let t = 0; t < 6; t++) {
    try {
      const r = await fetch('http://127.0.0.1:9223/json/list');
      const targets = await r.json();
      // O diff por idsAntes só vale se a leitura de ANTES do view nascer foi confiável; com ela
      // falha (null), casar por "target novo" pode pegar o app do hangar (com o token no
      // localStorage) — então sem diff não grava targetId e o CLI casa por URL.
      const novo = idsAntes
        ? targets.find((x) => x.type === 'page' && !idsAntes.has(x.id) && x.url.startsWith('http'))
        : null;
      if (novo || t === 5) {
        fs.mkdirSync(NAV_SIDECARS, { recursive: true });
        const arq = path.join(NAV_SIDECARS, `${nomeSidecar(chave)}.json`);
        const tmp = path.join(NAV_SIDECARS, `.${nomeSidecar(chave)}.${process.pid}.tmp`);
        fs.writeFileSync(tmp, JSON.stringify({ chave, url: novo ? novo.url : urlInicial, targetId: novo ? novo.id : null, ts: Date.now() }));
        fs.renameSync(tmp, arq);
        return;
      }
    } catch { /* CDP ainda subindo */ }
    await new Promise((r) => setTimeout(r, 400));
  }
}

// null em falha (NÃO Set vazio): um conjunto vazio por erro de leitura faria o diff achar que
// qualquer target é "novo" — inclusive o app principal do hangar.
async function idsDeTargets() {
  try {
    const r = await fetch('http://127.0.0.1:9223/json/list');
    return new Set((await r.json()).map((x) => x.id));
  } catch {
    return null;
  }
}

function viewDe(ev, chave) {
  return navegadores.get(BrowserWindow.fromWebContents(ev.sender))?.get(chave);
}

ipcMain.handle('hangar:nav-open', async (ev, { chave, url, bounds } = {}) => {
  const win = BrowserWindow.fromWebContents(ev.sender);
  if (!win || !chave) return { ok: false };
  const views = viewsDa(win);
  let view = views.get(chave);
  // O webContents pode ter morrido por fora (fechado via CDP Target.closeTarget, crash do
  // renderer): sem esta checagem o view volta invisível e nunca mais pinta — a área fica preta.
  if (view && view.webContents.isDestroyed()) {
    views.delete(chave);
    try { win.contentView.removeChildView(view); } catch { /* já saiu */ }
    view = undefined;
  }
  if (!view) {
    // View novo SÓ nasce com URL; o reexibir (troca de sessão, reload do front) chama open sem
    // url e recebe ok:false se o shell já não tiver o view — aí o front repete com a url salva.
    const destino = urlNavegavel(url);
    if (!destino) return { ok: false };
    // persist: cookies/localStorage no disco. COMPARTILHADA entre sessões de propósito — o uso é
    // cada sessão com suas URLs, não isolamento de conta; se um dia precisar, vira por-sessão.
    view = new WebContentsView({
      webPreferences: { partition: 'persist:nav', backgroundThrottling: false },
    });
    view.webContents.setUserAgent(uaDeChrome(view.webContents.getUserAgent()));
    // target=_blank vai pro navegador do sistema, mesmo padrão do cockpit.
    view.webContents.setWindowOpenHandler(({ url: alvo }) => {
      if (/^https?:/i.test(alvo)) shell.openExternal(alvo);
      return { action: 'deny' };
    });
    win.contentView.addChildView(view);
    views.set(chave, view);
    // idsDeTargets ANTES de criar o view: o targetId sai por diff do /json/list. Se o fetch ao
    // CDP falha (Set vazio), o diff pegaria o PRIMEIRO target page — que pode ser o COCKPIT (com
    // o token no localStorage). Sem diff confiável, grava sem targetId e o CLI casa por URL.
    const antes = await idsDeTargets();
    view.webContents.loadURL(destino).catch((err) => console.error('[nav] loadURL falhou:', err?.message || err));
    gravarSidecarNav(chave, destino, antes);   // async, não bloqueia o IPC
  } else {
    // Reexibir NUNCA recarrega: a URL atual do view pode ter mudado por navegação interna (o
    // agente clicou em links) e o front só manda `url` quando o usuário digita uma nova.
    const destino = url ? urlNavegavel(url) : null;
    if (destino && view.webContents.getURL() !== destino) view.webContents.loadURL(destino);
  }
  view.setVisible(true);
  view.setBounds(normalizaBounds(bounds));
  return { ok: true };
});

ipcMain.on('hangar:nav-hide', (ev, { chave } = {}) => {
  const view = viewDe(ev, chave);
  if (view) view.setVisible(false);
});

ipcMain.on('hangar:nav-bounds', (ev, { chave, bounds } = {}) => {
  const view = viewDe(ev, chave);
  if (view) view.setBounds(normalizaBounds(bounds));
});

ipcMain.on('hangar:nav-reload', (ev, { chave } = {}) => {
  const view = viewDe(ev, chave);
  if (view) view.webContents.reload();
});

ipcMain.on('hangar:nav-close', (ev, { chave } = {}) => fecharNavegador(BrowserWindow.fromWebContents(ev.sender), chave));

// Seletor nativo de pasta (window.hangar.pickFolder, via preload.cjs). Registrado UMA vez, fora
// do criarJanela — handler duplicado por janela é erro do ipcMain. O diálogo ancora na janela que
// pediu, senão ele nasce solto e pode cair atrás do app.
ipcMain.handle('hangar:pick-folder', async (ev) => {
  const win = BrowserWindow.fromWebContents(ev.sender);
  const r = await (win ? dialog.showOpenDialog(win, { properties: ['openDirectory'] })
                       : dialog.showOpenDialog({ properties: ['openDirectory'] }));
  return r.canceled ? null : (r.filePaths[0] ?? null);
});

if (!app.requestSingleInstanceLock()) {
  app.quit();
} else {
  app.on('second-instance', () => abrirJanela('segunda janela'));
  app.whenReady().then(() => abrirJanela('arranque'));
}

app.on('window-all-closed', () => app.quit());
