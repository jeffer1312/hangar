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
async function candidatosWs({ porta, perfis = raizesDePerfil() }) {
  const out = perfis.map(endpointDoPerfil).filter(Boolean);
  const daPorta = porta ? await urlDaPorta(porta) : null;
  if (daPorta) out.push(daPorta);
  return out;
}

// Cliente WebSocket mínimo em cima de `net`, e não o `WebSocket` global do Node: contra o servidor
// que o toggle sobe, o global fica pendurado no handshake (medido no Chrome 150: `curl` e este
// cliente recebem 101 e a resposta; o global nunca dispara `open` nem `error`). Só o que o CDP
// usa: handshake, um frame de texto mascarado pra fora, frames de texto sem máscara pra dentro.
const net = require('net');
const crypto = require('crypto');

function frameTexto(txt) {
  const p = Buffer.from(txt);
  const mask = crypto.randomBytes(4);
  const cab = [0x81];
  if (p.length < 126) cab.push(0x80 | p.length);
  else if (p.length < 65536) cab.push(0x80 | 126, p.length >> 8, p.length & 255);
  else { cab.push(0x80 | 127, 0, 0, 0, 0, (p.length >>> 24) & 255, (p.length >>> 16) & 255, (p.length >>> 8) & 255, p.length & 255); }
  return Buffer.concat([Buffer.from(cab), mask, Buffer.from(p.map((b, i) => b ^ mask[i % 4]))]);
}

// Abre a conexão; `aoAbrir` recebe (socket, enviar). Resolve com a primeira mensagem de texto
// completa (mensagens grandes chegam em vários frames — junta até o FIN).
function conectarWs(wsUrl, { timeoutMs = 5000, soHandshake = false, metodo, params } = {}) {
  return new Promise((resolve, reject) => {
    const u = new URL(wsUrl);
    const s = net.connect(+u.port || 80, u.hostname);
    let buf = Buffer.alloc(0); let hs = false; let texto = '';
    const t = setTimeout(() => { s.destroy(); reject(new Error('CDP sem resposta')); }, timeoutMs);
    const fim = (fn) => { clearTimeout(t); s.destroy(); fn(); };
    s.on('error', () => fim(() => reject(new ChromeFechado(u.port))));
    s.on('connect', () => s.write(`GET ${u.pathname}${u.search} HTTP/1.1\r\nHost: ${u.host}\r\nUpgrade: websocket\r\n`
      + `Connection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: ${crypto.randomBytes(16).toString('base64')}\r\n\r\n`));
    s.on('data', (d) => {
      buf = Buffer.concat([buf, d]);
      if (!hs) {
        const i = buf.indexOf('\r\n\r\n');
        if (i < 0) return;
        if (!/^HTTP\/1\.1 101/.test(buf.toString('latin1', 0, 12))) return fim(() => reject(new ChromeFechado(u.port)));
        hs = true; buf = buf.subarray(i + 4);
        if (soHandshake) return fim(() => resolve(true));
        s.write(frameTexto(JSON.stringify({ id: 1, method: metodo, params: params || {} })));
      }
      for (;;) {
        if (buf.length < 2) return;
        const fin = (buf[0] & 0x80) !== 0; const op = buf[0] & 0x0f;
        let len = buf[1] & 127; let off = 2;
        if (len === 126) { if (buf.length < 4) return; len = buf.readUInt16BE(2); off = 4; }
        else if (len === 127) { if (buf.length < 10) return; len = Number(buf.readBigUInt64BE(2)); off = 10; }
        if (buf.length < off + len) return;
        const corpo = buf.subarray(off, off + len); buf = buf.subarray(off + len);
        if (op === 8) return fim(() => reject(new Error('CDP fechou a conexão')));
        if (op === 1 || op === 0) texto += corpo.toString();
        if (fin && (op === 1 || op === 0)) {
          const m = JSON.parse(texto); texto = '';
          if (m.id !== 1) continue;
          return fim(() => (m.error ? reject(new Error(m.error.message)) : resolve(m.result)));
        }
      }
    });
  });
}

// 15s: medido, a primeira conexão depois de um tempo parado leva de 2 a 6s pra responder.
const cdp = (wsUrl, metodo, params = {}) => conectarWs(wsUrl, { metodo, params, timeoutMs: 15000 });

async function importarCookiesDoChrome({ porta, dominio, perfis }) {
  // Uma conexão só por candidato (o handshake deste servidor leva ~2s — sondar antes e conectar
  // de novo dobrava o tempo). `DevToolsActivePort` pode sobrar de um Chrome já fechado: aí a
  // conexão falha e passa pro próximo. `Storage.getCookies` responde no target do browser;
  // `Network.getAllCookies` só num de página.
  let ultimo = null;
  for (const ws of await candidatosWs({ porta, perfis })) {
    // Duas tentativas: medido, a PRIMEIRA conexão ao servidor do toggle depois de um tempo
    // parado cai (as seguintes respondem em <1s) — uma só virava "Chrome sem depuração" falso.
    for (let tentativa = 0; tentativa < 2; tentativa++) {
      try {
        const { cookies } = await cdp(ws, 'Storage.getCookies');
        // `*` = tudo de uma vez: cada conexão ao CDP faz o Chrome perguntar "Permitir a depuração
        // remota?" na tela da pessoa, então uma importação por domínio era um diálogo por site.
        const escolhidos = dominio === '*' ? cookies : cookies.filter((c) => casaDominio(c.domain, dominio));
        return escolhidos.map((c) => paraElectron(c, dominio));
      } catch (e) {
        ultimo = e;
        console.warn(`cookies_chrome: ${ws} tentativa ${tentativa + 1} falhou: ${e.message}`);
        await new Promise((r) => setTimeout(r, 400));
      }
    }
  }
  throw ultimo instanceof ChromeFechado ? ultimo : new ChromeFechado(porta);
}

module.exports = { importarCookiesDoChrome, casaDominio, paraElectron, ChromeFechado, endpointDoPerfil, raizesDePerfil, PAGINA_ATIVAR };
