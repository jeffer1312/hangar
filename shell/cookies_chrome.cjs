// Cookies do Chrome REAL do usuário, via CDP, já decifrados — o arquivo `Cookies` em disco é
// cifrado com a chave do chaveiro e muda de formato a cada versão.
//
// Como chegar no CDP do Chrome do usuário: o Chrome (136+) NÃO aceita mais `--remote-debugging-port`
// no perfil padrão — o flag entra na linha de comando e a porta nunca sobe. O caminho que ele
// aceita é o toggle em `chrome://inspect/#remote-debugging` ("Allow remote debugging for this
// browser"): ligado, o Chrome grava `DevToolsActivePort` na raiz do perfil (linha 1 = porta,
// linha 2 = caminho do WebSocket do browser) e é por esse par que a gente entra. A porta fixa
// (`porta`) fica como plano B pra Chromium/Brave antigos ou perfil alternativo.
const fs = require('fs');
const os = require('os');
const path = require('path');

const PAGINA_ATIVAR = 'chrome://inspect/#remote-debugging';
const SAME_SITE = { Strict: 'strict', Lax: 'lax', None: 'no_restriction' };

class ChromeFechado extends Error {
  constructor(porta, motivo = 'fechado') {
    super(motivo === 'headless'
      ? `a porta ${porta} está com um Chrome de automação (headless), não o seu`
      : `o Chrome não está com a depuração remota ligada (${PAGINA_ATIVAR})`);
    this.code = 'chrome_fechado';
    this.motivo = motivo;
  }
}

// Raízes de perfil onde o Chrome grava `DevToolsActivePort`, na ordem em que vale procurar.
function raizesDePerfil() {
  const home = os.homedir();
  if (process.platform === 'win32') {
    const local = process.env.LOCALAPPDATA || path.join(home, 'AppData', 'Local');
    return [path.join(local, 'Google', 'Chrome', 'User Data'), path.join(local, 'Microsoft', 'Edge', 'User Data')];
  }
  if (process.platform === 'darwin') {
    const s = path.join(home, 'Library', 'Application Support');
    return [path.join(s, 'Google', 'Chrome'), path.join(s, 'Chromium'), path.join(s, 'BraveSoftware', 'Brave-Browser')];
  }
  const cfg = process.env.XDG_CONFIG_HOME || path.join(home, '.config');
  return [path.join(cfg, 'google-chrome'), path.join(cfg, 'chromium'), path.join(cfg, 'BraveSoftware', 'Brave-Browser')];
}

// `DevToolsActivePort` de um perfil -> URL do WebSocket do browser, ou null.
function endpointDoPerfil(raiz) {
  try {
    const [porta, caminho] = fs.readFileSync(path.join(raiz, 'DevToolsActivePort'), 'utf8').split('\n').map((l) => l.trim());
    if (!/^\d+$/.test(porta) || !caminho || !caminho.startsWith('/')) return null;
    return `ws://127.0.0.1:${porta}${caminho}`;
  } catch { return null; }
}

function casaDominio(cookieDomain, host) {
  const d = String(cookieDomain || '').replace(/^\./, '').toLowerCase();
  const h = host.toLowerCase();
  return !!d && (d === h || h.endsWith('.' + d));
}

function paraElectron(c, host) {
  return {
    url: `${c.secure ? 'https' : 'http'}://${String(c.domain || host).replace(/^\./, '')}${c.path || '/'}`,
    name: c.name, value: c.value, domain: c.domain, path: c.path || '/',
    secure: !!c.secure, httpOnly: !!c.httpOnly,
    ...(c.expires > 0 ? { expirationDate: c.expires } : {}),
    sameSite: SAME_SITE[c.sameSite] || 'unspecified',
  };
}

async function urlDaPorta(porta) {
  try {
    const r = await fetch(`http://127.0.0.1:${porta}/json/version`, { signal: AbortSignal.timeout(2000) });
    const j = await r.json();
    // A porta pode estar com um Chrome HEADLESS de automação (agent-browser ocupa a 9222): sem
    // cookies de ninguém, e trazer 0 parecia "não tinha login".
    if (/Headless/i.test(j['User-Agent'] || '')) throw new ChromeFechado(porta, 'headless');
    if (j.webSocketDebuggerUrl) return j.webSocketDebuggerUrl;
  } catch (e) { if (e instanceof ChromeFechado) throw e; }
  return null;
}

// Endpoint do browser: o arquivo do perfil primeiro (o jeito que o Chrome atual aceita), a porta
// fixa depois. O arquivo pode sobrar de um Chrome já fechado — por isso quem lê testa a conexão.
async function urlDoWs({ porta, perfis = raizesDePerfil() }) {
  for (const raiz of perfis) {
    const ws = endpointDoPerfil(raiz);
    if (ws && (await responde(ws))) return ws;
  }
  const daPorta = porta ? await urlDaPorta(porta) : null;
  if (daPorta) return daPorta;
  throw new ChromeFechado(porta);
}

function responde(wsUrl) {
  return new Promise((resolve) => {
    let ws;
    try { ws = new WebSocket(wsUrl); } catch { return resolve(false); }
    const t = setTimeout(() => { ws.close(); resolve(false); }, 1500);
    ws.onerror = () => { clearTimeout(t); resolve(false); };
    ws.onopen = () => { clearTimeout(t); ws.close(); resolve(true); };
  });
}

function cdp(wsUrl, metodo, params = {}) {
  return new Promise((resolve, reject) => {
    const ws = new WebSocket(wsUrl);
    const t = setTimeout(() => { ws.close(); reject(new Error('CDP sem resposta')); }, 5000);
    ws.onerror = () => { clearTimeout(t); reject(new ChromeFechado(new URL(wsUrl).port)); };
    ws.onopen = () => ws.send(JSON.stringify({ id: 1, method: metodo, params }));
    ws.onmessage = (ev) => {
      const m = JSON.parse(ev.data);
      if (m.id !== 1) return;
      clearTimeout(t); ws.close();
      m.error ? reject(new Error(m.error.message)) : resolve(m.result);
    };
  });
}

async function importarCookiesDoChrome({ porta, dominio, perfis }) {
  const ws = await urlDoWs({ porta, perfis });
  // `Storage.getCookies` responde no target do browser; `Network.getAllCookies` só num de página.
  const { cookies } = await cdp(ws, 'Storage.getCookies');
  return cookies.filter((c) => casaDominio(c.domain, dominio)).map((c) => paraElectron(c, dominio));
}

module.exports = { importarCookiesDoChrome, casaDominio, paraElectron, ChromeFechado, endpointDoPerfil, raizesDePerfil, PAGINA_ATIVAR };
