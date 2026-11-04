export const INSTALL = {
  unix: 'curl -fsSL https://raw.githubusercontent.com/jeffer1312/hangar/main/bootstrap.sh | bash',
  win: 'irm https://raw.githubusercontent.com/jeffer1312/hangar/main/bootstrap.ps1 | iex',
};

export function detectPlatform(ua = '', platform = '', touchPoints = 0) {
  if (/Android|iPhone|iPod|iPad|Mobile/i.test(ua)) return 'phone';
  if (/Mac/i.test(platform) && touchPoints > 1) return 'phone';
  // /Win/ sozinho casa "Darwin" no userAgent do macOS
  if (/Windows|Win32|Win64/i.test(`${ua} ${platform}`)) return 'windows';
  if (/Mac/i.test(`${ua} ${platform}`)) return 'macos';
  return 'linux';
}

export const deviceFor = (platform, ua = '') => {
  if (platform !== 'phone') return platform;
  return /Android/i.test(ua) ? 'android' : 'iphone';
};
export const installFor = (p) => (p === 'windows' ? 'win' : 'unix');
export const nextIndex = (i, n) => (i + 1) % n;
export const prevIndex = (i, n) => (i - 1 + n) % n;

const reducedMotion = () => globalThis.matchMedia?.('(prefers-reduced-motion: reduce)').matches ?? false;

function initShowcase(root) {
  const tabs = [...root.querySelectorAll('[role=tab]')];
  const video = root.querySelector('video');
  const caption = root.querySelector('.sc-caption');
  let i = 0;
  let visible = true;
  const play = () => {
    if (reducedMotion()) { video.controls = true; return; }
    if (!visible) return;
    // autoplay negado: sem controles o vídeo ficaria preso na capa e as abas não andariam.
    // AbortError é só troca de src ou pause no meio do play, não negação.
    video.play().catch((e) => { if (e.name !== 'AbortError') video.controls = true; });
  };
  const show = (k, focus = false) => {
    i = k;
    tabs.forEach((t, j) => {
      t.setAttribute('aria-selected', String(j === k));
      t.tabIndex = j === k ? 0 : -1;
      t.querySelector('.sc-bar').style.width = '0%';
    });
    const t = tabs[k];
    video.poster = t.dataset.poster;
    video.src = t.dataset.src;
    video.setAttribute('aria-label', t.dataset.alt);
    caption.textContent = t.dataset.caption;
    if (focus) t.focus();
    play();
  };
  tabs.forEach((t, k) => t.addEventListener('click', () => show(k)));
  root.querySelector('[role=tablist]').addEventListener('keydown', (e) => {
    if (e.key !== 'ArrowRight' && e.key !== 'ArrowLeft') return;
    e.preventDefault();
    // a aba em foco pode não ser a que toca: o avanço automático muda i sem mover o foco
    const f = tabs.indexOf(e.target.closest('[role=tab]'));
    const k = f < 0 ? i : f;
    show(e.key === 'ArrowRight' ? nextIndex(k, tabs.length) : prevIndex(k, tabs.length), true);
  });
  video.addEventListener('timeupdate', () => {
    if (video.duration) tabs[i].querySelector('.sc-bar').style.width = `${(video.currentTime / video.duration) * 100}%`;
  });
  video.addEventListener('ended', () => show(nextIndex(i, tabs.length)));
  // sem 'ended' as abas param; com controles o visitante ainda vê o que der
  video.addEventListener('error', () => {
    video.controls = true;
    console.error('showcase: vídeo não carregou', video.currentSrc || video.src, video.error);
  });
  // fora da tela pausa, então 'ended' não dispara e as abas não andam escondidas
  if (!reducedMotion() && typeof IntersectionObserver !== 'undefined') {
    new IntersectionObserver(([e]) => {
      visible = e.isIntersecting;
      if (visible) play(); else video.pause();
    }, { threshold: 0.25 }).observe(video);
  }
  show(0);
}

function initTerminal(el) {
  const lines = [...el.querySelectorAll('[data-at]')];
  if (reducedMotion()) { lines.forEach((l) => { l.hidden = false; }); return; }
  let s = 0;
  const paint = () => lines.forEach((l) => { l.hidden = s < Number(l.dataset.at); });
  paint();
  // esconder e mostrar de novo reinicia a animação CSS de digitação de cada linha
  setInterval(() => { s = nextIndex(s, 32); paint(); }, 380);
}

function initInstall(root, platform) {
  const tabs = [...root.querySelectorAll('[data-os]')];
  const code = root.querySelector('[data-cmd]');
  const prompt = root.querySelector('[data-prompt]');
  const btn = root.querySelector('[data-copy]');
  let os = installFor(platform);
  let timer;
  const label = (text, state) => {
    btn.setAttribute('aria-label', text);
    btn.dataset.state = state;
    btn.querySelector('.copy-text').textContent = state === 'idle' ? '' : text;
    clearTimeout(timer);
    if (state !== 'idle') timer = setTimeout(() => label(btn.dataset.labelCopy, 'idle'), 1800);
  };
  const render = () => {
    tabs.forEach((t) => {
      t.setAttribute('aria-selected', String(t.dataset.os === os));
      t.tabIndex = t.dataset.os === os ? 0 : -1;
    });
    code.textContent = INSTALL[os];
    prompt.textContent = os === 'win' ? 'PS>' : '$';
  };
  const pick = (t) => { os = t.dataset.os; render(); label(btn.dataset.labelCopy, 'idle'); };
  tabs.forEach((t) => t.addEventListener('click', () => pick(t)));
  root.querySelector('[role=tablist]').addEventListener('keydown', (e) => {
    if (e.key !== 'ArrowRight' && e.key !== 'ArrowLeft') return;
    e.preventDefault();
    const f = tabs.indexOf(e.target.closest('[role=tab]'));
    const k = f < 0 ? tabs.findIndex((t) => t.dataset.os === os) : f;
    const t = tabs[e.key === 'ArrowRight' ? nextIndex(k, tabs.length) : prevIndex(k, tabs.length)];
    pick(t);
    t.focus();
  });
  btn.addEventListener('click', async () => {
    try {
      await navigator.clipboard.writeText(INSTALL[os]);
      label(btn.dataset.labelCopied, 'copied');
    } catch {
      // sem permissão de área de transferência: deixa o comando selecionado para Ctrl+C
      getSelection().selectAllChildren(code);
      label(btn.dataset.labelSelected, 'selected');
    }
  });
  render();
}

function initPlatform(platform, device) {
  const key = `label${platform[0].toUpperCase()}${platform.slice(1)}`;
  document.querySelectorAll('[data-cta-os]').forEach((a) => {
    a.querySelector('.cta-text').textContent = a.dataset[key];
    if (platform === 'phone') a.setAttribute('href', '#celular');
  });
  const mine = [...document.querySelectorAll('[data-platform]')].filter((c) => c.dataset.platform === device);
  mine.forEach((c) => c.classList.add('is-mine'));
  // o navegador não diz se o Mac é Intel ou Apple Silicon: com dois cartões, o selo seria palpite
  if (mine.length === 1) mine[0].querySelector('.badge-mine').hidden = false;
  // mover no DOM, não por CSS: Tab e leitor de tela seguem a mesma ordem da tela
  mine[0]?.parentElement.prepend(...mine);
}

function initLoopVideos() {
  const vids = [...document.querySelectorAll('video.loop')];
  if (reducedMotion() || typeof IntersectionObserver === 'undefined') { vids.forEach((v) => { v.controls = true; }); return; }
  const io = new IntersectionObserver((entries) => entries.forEach((e) => {
    if (e.isIntersecting) e.target.play().catch((err) => { if (err.name !== 'AbortError') e.target.controls = true; });
    else e.target.pause();
  }), { threshold: 0.25 });
  vids.forEach((v) => io.observe(v));
}

if (typeof document !== 'undefined') {
  document.addEventListener('DOMContentLoaded', () => {
    const platform = detectPlatform(navigator.userAgent, navigator.platform, navigator.maxTouchPoints);
    // uma parte quebrada não pode levar as outras junto
    const safe = (name, fn) => { try { fn(); } catch (e) { console.error(`${name} falhou`, e); } };
    safe('showcase', () => document.querySelectorAll('[data-showcase]').forEach(initShowcase));
    safe('terminal', () => document.querySelectorAll('[data-terminal]').forEach(initTerminal));
    safe('install', () => document.querySelectorAll('[data-install]').forEach((el) => initInstall(el, platform)));
    safe('platform', () => initPlatform(platform, deviceFor(platform, navigator.userAgent)));
    safe('loopVideos', initLoopVideos);
  });
}
