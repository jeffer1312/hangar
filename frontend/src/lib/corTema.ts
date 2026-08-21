// Cor manual do tema — destaque (accent) e tinta de fundo, editáveis por modo (escuro/claro).
// Referência: o "Customize Theme" do super.engineering (canais Accent/Background separados, um
// valor por modo). Guarda só o DESVIO do padrão de fábrica em localStorage (`cp_cor_dark` /
// `cp_cor_light`); sem escolha gravada, NENHUM token é tocado e o app fica bit-a-bit como era.
//
// Convivência com o tema Desktop: lá quem escreve os tokens é a paleta Material You
// (desktopTheme.aplicarPaleta) — este módulo fica MUDO enquanto getThemePref() === 'desktop', e a
// UI da Aparência nem mostra a seção (mesmo gate da "Cor do texto", só que invertido). Os dois
// nunca escrevem ao mesmo tempo.

export type ModoCor = 'dark' | 'light';

export interface CorTema {
  /** Accent escolhido (hex). null = o do tema de fábrica. */
  destaque: string | null;
  /** Cor da tinta de fundo (hex). null = sem tinta (neutros de fábrica). */
  tinta: string | null;
  /** Força da tinta 0-100 (vira fração de mistura — ver TINTA_MAX). */
  forca: number;
}

const PADRAO: CorTema = { destaque: null, tinta: null, forca: 50 };
const KEYS: Record<ModoCor, string> = { dark: 'cp_cor_dark', light: 'cp_cor_light' };

// Teto da mistura: 100 no slider = 45% da cor escolhida na rampa de fundos. Acima disso o texto
// (clareado pra AAA sobre o neutro) começava a perder contraste medido no escuro — a tinta é
// ambiente, não parede pintada.
const TINTA_MAX = 0.45;

// Tokens escritos pelo destaque. `--accent-dim` deriva da MESMA cor com a alfa de fábrica do modo
// (0.18 escuro / 0.12 claro, app.css), e `--accent-press` escurece — um valor só escolhido, os
// três nunca saem de sincronia.
const CHAVES_DESTAQUE = ['--accent', '--accent-dim', '--accent-press'] as const;
// Tokens que recebem a tinta: a rampa de fundos + as triplas cruas do vidro e do véu. As bordas
// ficam (são branco-com-alfa, acompanham qualquer fundo); `--surface-*` são aliases da rampa e
// seguem sozinhos. Lista no MESMO espírito do CHAVES do desktopTheme: explícita pra `limpar` saber
// o que tirar mesmo num boot diferente do que aplicou.
const CHAVES_TINTA = [
  '--bg-base', '--bg-surface', '--bg-elevated', '--bg-hover',
  '--veu-rgb', '--veu-amostra-rgb', '--glass-panel-rgb', '--glass-rgb', '--glass-solid-rgb',
] as const;

export function getCorTema(modo: ModoCor): CorTema {
  if (typeof localStorage === 'undefined') return PADRAO;
  try {
    const raw = localStorage.getItem(KEYS[modo]);
    if (!raw) return PADRAO;
    const p = JSON.parse(raw) as Partial<CorTema>;
    return {
      destaque: typeof p.destaque === 'string' ? p.destaque : null,
      tinta: typeof p.tinta === 'string' ? p.tinta : null,
      forca: typeof p.forca === 'number' ? Math.min(100, Math.max(0, p.forca)) : PADRAO.forca,
    };
  } catch {
    return PADRAO;
  }
}

function gravar(modo: ModoCor, c: CorTema): void {
  if (typeof localStorage === 'undefined') return;
  if (!c.destaque && !c.tinta) localStorage.removeItem(KEYS[modo]);
  else localStorage.setItem(KEYS[modo], JSON.stringify(c));
  aplicarCorTema();
}

export function setDestaque(modo: ModoCor, hex: string | null): void {
  gravar(modo, { ...getCorTema(modo), destaque: hex });
}

export function setTinta(modo: ModoCor, hex: string | null, forca?: number): void {
  const atual = getCorTema(modo);
  gravar(modo, { ...atual, tinta: hex, forca: forca ?? atual.forca });
}

export function setForca(modo: ModoCor, forca: number): void {
  gravar(modo, { ...getCorTema(modo), forca });
}

// "Copiar do outro modo" (o "Copy from Dark" deles): copia o desvio GRAVADO do outro modo —
// se lá não há escolha, aqui volta ao padrão de fábrica (gravar sem nada = remover a chave).
export function copiarDoOutroModo(modo: ModoCor): void {
  const outro: ModoCor = modo === 'dark' ? 'light' : 'dark';
  gravar(modo, getCorTema(outro));
}

export function temCorTema(): boolean {
  if (typeof localStorage === 'undefined') return false;
  return localStorage.getItem(KEYS.dark) !== null || localStorage.getItem(KEYS.light) !== null;
}

// "Voltar ao padrão" da Aparência: apaga o desvio dos DOIS modos e tira os tokens inline (o
// aplicarCorTema rodado sem escolha nenhuma já remove tudo e só ressincroniza a meta).
export function limparCorTema(): void {
  if (typeof localStorage !== 'undefined') {
    localStorage.removeItem(KEYS.dark);
    localStorage.removeItem(KEYS.light);
  }
  aplicarCorTema();
}

// ── Aplicação ────────────────────────────────────────────────────────────────

type Rgb = [number, number, number];

function hexPraRgb(hex: string): Rgb {
  const n = parseInt(hex.slice(1), 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
}

// Computed style de custom property devolve o valor ESPECIFICADO: hex ("#100e11") pras cores da
// rampa, "rgb(16, 14, 17)" ou a tripla crua "16 14 17" pras triplas. Hex primeiro — sem isto a
// tinta nunca tocava --bg-* (o regex lia "8","6","2" de "#f8f6f2" e o claro ficava lamacento).
function parseCor(v: string): Rgb | null {
  const t = v.trim();
  if (/^#[0-9a-fA-F]{6}$/.test(t)) return hexPraRgb(t);
  const nums = t.match(/[\d.]+/g);
  if (!nums || nums.length < 3) return null;
  return [+nums[0], +nums[1], +nums[2]];
}

function misturar(base: Rgb, tinta: Rgb, frac: number): Rgb {
  return base.map((b, i) => Math.round(b * (1 - frac) + tinta[i] * frac)) as Rgb;
}

// Mesma regra do desktopTheme.escurecer (botão apertado precisa ser visivelmente diferente, e um
// accent quase preto não tem pra onde escurecer — clareia). Duplicada de propósito: os dois
// módulos são donos de tokens independentes e não se importam.
function escurecer([r, g, b]: Rgb, fator = 0.82): Rgb {
  if (r === 0 && g === 0 && b === 0) return [46, 46, 46];
  return [Math.round(r * fator), Math.round(g * fator), Math.round(b * fator)];
}

// Aplica o desvio do modo RESOLVIDO (data-theme já no <html>) por cima dos tokens de fábrica.
// Chamada pelo applyTheme (theme.ts) a cada troca de tema e pelos setters acima a cada toque da
// UI. No tema 'desktop' não escreve nada (a paleta é a dona) — mas TAMBÉM não deixa restos: quem
// limpa ao entrar/sair do desktop é o ThemeToggle (limparPaleta + esta chamada).
export function aplicarCorTema(): void {
  if (typeof document === 'undefined' || typeof getComputedStyle === 'undefined') return;
  const raiz = document.documentElement;
  const modo: ModoCor = raiz.dataset.theme === 'light' ? 'light' : 'dark';
  const temaPref = typeof localStorage !== 'undefined' ? localStorage.getItem('cp_theme') : null;
  const c = getCorTema(modo);

  // No tema Desktop NÃO remove nada: as chaves inline ali são da PALETA (desktopTheme), e tirá-las
  // sem reaplicar (o "Voltar ao padrão" chama limparCorTema com tema desktop ativo) quebrava o
  // tema até o próximo boot. Ao ENTRAR no desktop quem limpa é o ThemeToggle (limparPaleta).
  if (temaPref === 'desktop') return;

  // Lê os padrões ANTES de escrever: remove as nossas chaves, lê o computed (que volta a ser o
  // valor do CSS), mistura e regrava — tudo síncrono no mesmo frame, sem flash. Ler sem remover
  // traria o nosso próprio override e a tinta se acumularia a cada tick do slider.
  const chaves = [...CHAVES_DESTAQUE, ...CHAVES_TINTA];
  for (const k of chaves) raiz.style.removeProperty(k);
  if (!c.destaque && !c.tinta) { sincronizarMeta(); return; }

  if (c.destaque) {
    const rgb = hexPraRgb(c.destaque);
    const press = escurecer(rgb);
    raiz.style.setProperty('--accent', c.destaque);
    // Alfa por modo, como a fábrica: 0.18 no escuro, 0.12 no claro (app.css).
    raiz.style.setProperty('--accent-dim', `rgb(${rgb.join(' ')} / ${modo === 'light' ? 0.12 : 0.18})`);
    raiz.style.setProperty('--accent-press', `rgb(${press.join(' ')})`);
  }
  if (c.tinta) {
    const frac = (c.forca / 100) * TINTA_MAX;
    const t = hexPraRgb(c.tinta);
    const cs = getComputedStyle(raiz);
    for (const k of CHAVES_TINTA) {
      const base = parseCor(cs.getPropertyValue(k));
      if (!base) continue;
      const m = misturar(base, t, frac);
      // Triplas cruas voltam como "r g b" (o formato que entra em rgb(.../alfa) e color-mix);
      // as cores diretas voltam como rgb() resolvido.
      raiz.style.setProperty(k, k.endsWith('-rgb') ? m.join(' ') : `rgb(${m.join(' ')})`);
    }
  }
  sincronizarMeta();
}

// A toolbar do Safari/PWA é tingida pelo <meta theme-color>, sincronizado no applyTheme ANTES da
// gente — com tinta aplicada ele ficaria na base sem tinta. Mesmo remédio do desktopTheme.
function sincronizarMeta(): void {
  const bg = getComputedStyle(document.documentElement).getPropertyValue('--bg-base').trim();
  const meta = document.querySelector('meta[name="theme-color"]') as HTMLMetaElement | null;
  if (bg && meta) meta.content = bg;
}
