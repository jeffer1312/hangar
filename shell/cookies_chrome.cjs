// Cookies do Chrome REAL do usuário, via CDP, já decifrados — o arquivo `Cookies` em disco é
// cifrado com a chave do chaveiro e muda de formato a cada versão. Precisa do Chrome aberto com
// `--remote-debugging-port`; sem isso, erro `chrome_fechado` com a instrução.
const COMANDO = 'google-chrome-stable --remote-debugging-port=9222';
const SAME_SITE = { Strict: 'strict', Lax: 'lax', None: 'no_restriction' };

class ChromeFechado extends Error {
  constructor(porta) {
    super(`Chrome sem a porta de depuração ${porta} — abra com: ${COMANDO}`);
    this.code = 'chrome_fechado';
  }
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

async function urlDoWs(porta) {
  try {
    const r = await fetch(`http://127.0.0.1:${porta}/json/version`, { signal: AbortSignal.timeout(2000) });
    const j = await r.json();
    if (j.webSocketDebuggerUrl) return j.webSocketDebuggerUrl;
  } catch { /* cai no erro abaixo */ }
  throw new ChromeFechado(porta);
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

async function importarCookiesDoChrome({ porta = 9222, dominio }) {
  const ws = await urlDoWs(porta);
  // `Storage.getCookies` responde no target do browser; `Network.getAllCookies` só num de página.
  const { cookies } = await cdp(ws, 'Storage.getCookies');
  return cookies.filter((c) => casaDominio(c.domain, dominio)).map((c) => paraElectron(c, dominio));
}

module.exports = { importarCookiesDoChrome, casaDominio, paraElectron, ChromeFechado, COMANDO };
