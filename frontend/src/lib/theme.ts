// Tema claro/escuro. 'system' segue o prefers-color-scheme do SO; 'light'/'dark' forcam.
// A escolha persiste em localStorage; o tema resolvido vira data-theme no <html> (a CSS so tem
// :root [dark] + [data-theme="light"], sem duplicar valores em media query).

export type ThemePref = 'system' | 'light' | 'dark' | 'desktop';

const KEY = 'cp_theme';
const mq = typeof window !== 'undefined' ? window.matchMedia('(prefers-color-scheme: light)') : null;

export function getThemePref(): ThemePref {
  const v = typeof localStorage !== 'undefined' ? localStorage.getItem(KEY) : null;
  return v === 'light' || v === 'dark' || v === 'desktop' ? v : 'system';
}

function resolve(pref: ThemePref): 'light' | 'dark' {
  // 'desktop' e resolvido DE NOVO por aplicarPaleta(), com o $darkmode do arquivo. Aqui so evita o
  // flash: entre o boot e a resposta do backend, vale o que o SO diz.
  if (pref === 'system' || pref === 'desktop') return mq?.matches ? 'light' : 'dark';
  return pref;
}

export function applyTheme(pref: ThemePref = getThemePref()): void {
  if (typeof document === 'undefined') return;
  document.documentElement.dataset.theme = resolve(pref);
  // Sincroniza o <meta theme-color> com o tema: a toolbar do Safari iOS (a "barra de baixo") e a
  // status bar sao tingidas por ele. Fixo em escuro, ela ficava preta no tema CLARO (a barra que
  // limitava embaixo). Le o --bg-base ja resolvido (fonte unica).
  const bg = getComputedStyle(document.documentElement).getPropertyValue('--bg-base').trim();
  if (bg) {
    let meta = document.querySelector('meta[name="theme-color"]') as HTMLMetaElement | null;
    if (!meta) {
      meta = document.createElement('meta');
      meta.name = 'theme-color';
      document.head.appendChild(meta);
    }
    meta.content = bg;
  }
}

export function setThemePref(pref: ThemePref): void {
  if (typeof localStorage !== 'undefined') {
    if (pref === 'system') localStorage.removeItem(KEY);
    else localStorage.setItem(KEY, pref);
  }
  applyTheme(pref);
}

// Em 'system', reage a troca de tema do SO ao vivo.
mq?.addEventListener('change', () => {
  if (getThemePref() === 'system') applyTheme('system');
});

const KEY_TEXTO = 'cp_texto_desktop';

// Cor do texto no tema do desktop. Padrao: seguir o desktop (escolha do usuario). So o DESVIO
// persiste — mesma convencao da fonte e do desfoque.
export function getTextoDoDesktop(): boolean {
  return typeof localStorage !== 'undefined' ? localStorage.getItem(KEY_TEXTO) !== 'app' : true;
}

export function setTextoDoDesktop(v: boolean): void {
  if (typeof localStorage === 'undefined') return;
  if (v) localStorage.removeItem(KEY_TEXTO);
  else localStorage.setItem(KEY_TEXTO, 'app');
}
