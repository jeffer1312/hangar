// Janela nativa do claude-cockpit. Ver docs/superpowers/specs/2026-08-05-shell-electron-design.md.
const { app, BrowserWindow, screen } = require('electron');
const { ler, gravar } = require('./settings.cjs');

// Legado X11; no Wayland o spike não isolou se faz diferença. MEDIR: abrir sem ele e ver se a
// transparência continua. Se continuar, apagar esta linha.
app.commandLine.appendSwitch('enable-transparent-visuals');

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

app.whenReady().then(async () => {
  const dir = app.getPath('userData');
  const cfg = ler(dir);
  // Precedência: variável de ambiente > escolha salva > padrão.
  const url = process.env.COCKPIT_URL || cfg.url || PADRAO;
  // Ultimo endereco que CARREGOU de verdade (did-navigate abaixo mantem isto atualizado). `url`
  // acima fica congelada no valor do boot; sem esta variavel a tela de recuperacao reoferecia
  // 127.0.0.1:8765 depois que o usuario ja tinha corrigido pra outro endereco e ele caiu de novo.
  let urlBoa = url;

  const fundo = opcoesDeFundo();
  const geo = geometriaValida(cfg.janela) || { width: 1280, height: 800 };

  // A marca que o front lê (frontend/src/lib/background.ts, isShell) SÓ é injetada quando a janela
  // vai mesmo ser transparente. Sem esta condição, numa plataforma opaca (Windows 10, GNOME sem
  // extensão) a tela de Aparência ofereceria "Desktop" e escolher isso zeraria o fundo de uma
  // janela sólida — o app nasceria sem fundo nenhum. Vai no user agent, e não na URL, porque
  // Login.svelte:111 apaga a query quando ela traz ?token= (pareamento).
  if (fundo.transparente) app.userAgentFallback += ' claude-cockpit-shell';

  const win = new BrowserWindow({
    ...geo,
    transparent: fundo.transparente,
    // Fallback opaco: onde a transparência não vale, a janela precisa de cor própria, senão
    // aparece preta. O front também não recebe a marca de fundo nesse caso (ver abaixo).
    backgroundColor: fundo.transparente ? '#00000000' : '#1a181d',
    ...fundo.extra,
    webPreferences: { contextIsolation: true },
  });
  win.removeMenu();

  // Título FIXO: é ele que a windowrulev2 do Hyprland casa (a classe é sempre 'electron' e não
  // muda — medido com app.setName e --class em 05/08/2026). preventDefault impede que a navegação
  // do SPA troque o título.
  win.setTitle('claude-cockpit');
  win.on('page-title-updated', (e) => e.preventDefault());

  // A tela de recuperação não é endereço de cockpit: se o fechamento pegar a janela parada nela
  // (backend fora, usuário fechou em vez de responder), gravar essa URL faria a próxima abertura
  // tentar carregar a própria tela de erro. Nesse caso mantém a URL que já estava salva.
  win.on('close', () => {
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
  });

  win.loadURL((await paginaResponde(url)) ? url : telaDeUrl(url));
});

app.on('window-all-closed', () => app.quit());
