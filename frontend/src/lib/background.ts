// Fundo da tela: chapado (o de sempre), texturado ou texturado com uma luz fria no canto. Mesmo
// mecanismo do tema (lib/theme.ts): a escolha persiste em localStorage e vira `data-bg` no <html>,
// com a CSS toda em app.css. 'flat' é o default e não escreve nada — quem nunca abrir esta opção
// continua com a tela idêntica à de hoje.
//
// Por que existe: com fundo perfeitamente uniforme o vidro dos painéis não tem o que borrar, então
// eles leem como retângulo escuro em vez de material. E o grão mata o banding que gradiente escuro
// produz em tela de 8 bits.

export type BgPref = 'flat' | 'texture' | 'aurora' | 'image';

const KEY = 'cp_bg';
const IMG_KEY = 'cp_bg_image';   // data URL da imagem escolhida (já encolhida)
const SCRIM_KEY = 'cp_bg_scrim';  // 0..100 — quanto da imagem passa (100 = imagem crua)
const SOLID_KEY = 'cp_surface_solid';  // 0..100 — quanto as caixas de dentro do painel sao mais opacas que ele

// Transparência do papel de parede. O terminal faz isso pelo compositor; aqui o equivalente é o
// quanto do scrim escuro fica entre a foto e o texto. Guardado por dispositivo, como a imagem.
// 0 = fundo praticamente opaco (a imagem some) · 100 = imagem crua (texto compete com ela).
// `Number(null)` é 0, não NaN: ler uma chave que NUNCA foi escrita devolvia 0 e passava direto pelo
// teste de faixa, então o padrão documentado (11 / 92 / 10) nunca valia — o app abria com o slider
// no chão e o modo Texto inerte até alguém arrastar. Ausência tem que virar NaN antes da conversão.
function lerNumero(chave: string, padrao: number): number {
  const cru = typeof localStorage !== 'undefined' ? localStorage.getItem(chave) : null;
  const v = cru === null || cru === '' ? NaN : Number(cru);
  return Number.isFinite(v) && v >= 0 && v <= 100 ? v : padrao;
}

// 11 = véu 0,901, o mesmo `background_opacity 0.90` do kitty. O padrão anterior (45) deixava a foto
// atravessando o texto e obrigava a compensar nos outros dois sliders.
export function getBgScrim(): number {
  return lerNumero(SCRIM_KEY, 11);
}

export function setBgScrim(v: number): void {
  const n = Math.max(0, Math.min(100, Math.round(v)));
  try { localStorage.setItem(SCRIM_KEY, String(n)); } catch { /* modo privado */ }
  aplicarScrim(n);
}

// Solidez das CAIXAS de dentro dos painéis (chip, campo, card, bloco de saída) — quanto elas ficam
// mais opacas que o painel que as hospeda. 0 = a caixa some no vidro, indistinguível do painel;
// 100 = caixa sólida por cima dele. Existe como controle, e não como constante no CSS, porque o
// ponto certo depende da FOTO de quem usa: sobre uma imagem clara e cheia de detalhe as caixas
// precisam de mais corpo pra o texto se ler; sobre uma escura e lisa, menos.
export function getSurfaceSolid(): number {
  return lerNumero(SOLID_KEY, 12);
}

export function setSurfaceSolid(v: number): void {
  const n = Math.max(0, Math.min(100, Math.round(v)));
  try { localStorage.setItem(SOLID_KEY, String(n)); } catch { /* modo privado */ }
  aplicarScrim();
}

function aplicarScrim(t = getBgScrim()): void {
  if (typeof document === 'undefined') return;
  // t alto = mais imagem = menos véu. UNIFORME e chegando a tapar: é o que o compositor do terminal
  // faz (kitty background_opacity), e era o que faltava — parando em 0,88 sobrava foto atravessando o
  // texto no slider no fim, e o gradiente topo/base fazia a mesma linha ler diferente em cima e
  // embaixo. O degrade existia pra proteger o composer da parte clara da foto; ele tem vidro próprio
  // (--glass-bg-solid), então não depende disso.
  const base = 1 - (t / 100) * 0.90;
  const topo = base;
  // Os PAINÉIS acompanham: com wallpaper, vidro a 0,86 vira parede opaca do lado direito e mata a
  // sensação que a foto dá. Anda junto do slider, mas numa faixa mais conservadora — é sobre eles
  // que o texto de leitura fica.
  const painel = 0.92 - (t / 100) * 0.50;
  // As caixas DE DENTRO dos painéis (tokens --surface-* do app.css) partem da alfa do painel e
  // ganham o degrau do slider Solidez. Sem degrau nenhum elas somem no vidro e a tela vira uma
  // superfície só; com degrau demais voltam a ser recorte chapado — é o mesmo dilema do véu, uma
  // camada acima, e por isso tem controle próprio em vez de um número fixo aqui.
  const caixa = Math.min(1, painel + getSurfaceSolid() / 100);
  const raiz = document.documentElement;
  raiz.style.setProperty('--cp-scrim-topo', topo.toFixed(3));
  raiz.style.setProperty('--cp-scrim-base', base.toFixed(3));
  raiz.style.setProperty('--cp-panel-alpha', painel.toFixed(3));
  raiz.style.setProperty('--cp-surface-alpha', caixa.toFixed(3));
}

// Papel de parede: o navegador não enxerga o wallpaper do sistema (é o que o terminal faz por ser
// translúcido), então o usuário escolhe um arquivo e ele fica guardado AQUI, no proprio dispositivo.
// Guardado como data URL depois de passar pelo mesmo encolhedor dos anexos (lib/imagePrep): uma foto
// de 4K crua estoura a cota do localStorage; a 1568px de lado maior ela cabe com folga.
export function getBgImage(): string | null {
  try { return localStorage.getItem(IMG_KEY); } catch { return null; }
}

// Teto de armazenamento. localStorage costuma dar ~5 MB por origem e guarda TEXTO: a data URL é
// base64, ~33% maior que os bytes. 2,5 MB de string é folga confortável dentro disso.
const TETO_CHARS = 2_500_000;
// Degraus de reencode. O imagePrep mira o que o MODELO enxerga (1568px) e, em qualquer falha,
// devolve o arquivo ORIGINAL de propósito — regra certa pra anexo, errada pra cá: um wallpaper 4K
// passava direto e estourava a cota. Aqui o alvo é caber, então reencodamos com o nosso critério.
const DEGRAUS = [
  { lado: 1920, q: 0.82 },
  { lado: 1600, q: 0.75 },
  { lado: 1280, q: 0.68 },
  { lado: 1024, q: 0.60 },
];

async function paraDataUrl(file: File, lado: number, q: number): Promise<string> {
  const bitmap = await createImageBitmap(file);
  const escala = Math.min(1, lado / Math.max(bitmap.width, bitmap.height));
  const w = Math.round(bitmap.width * escala);
  const h = Math.round(bitmap.height * escala);
  const cv = document.createElement('canvas');
  cv.width = w;
  cv.height = h;
  const ctx = cv.getContext('2d');
  if (!ctx) throw new Error('sem canvas 2d');
  ctx.drawImage(bitmap, 0, 0, w, h);
  bitmap.close?.();
  return cv.toDataURL('image/jpeg', q);   // JPEG sempre: PNG de foto não encolhe
}

export async function setBgImage(file: File): Promise<void> {
  // HEIC do iPhone não é decodificável direto em muitos navegadores; o imagePrep já converte via
  // canvas. Falhou lá, seguimos com o arquivo cru — os degraus abaixo ainda podem dar conta.
  let base = file;
  try {
    const { prepareImage } = await import('./imagePrep');
    base = await prepareImage(file);
  } catch { /* segue com o original */ }

  let dataUrl = '';
  for (const d of DEGRAUS) {
    dataUrl = await paraDataUrl(base, d.lado, d.q);
    if (dataUrl.length <= TETO_CHARS) break;
  }
  if (dataUrl.length > TETO_CHARS) {
    throw new Error('imagem grande demais mesmo depois de reduzir');
  }
  localStorage.setItem(IMG_KEY, dataUrl);
  setBgPref('image');
}

export function clearBgImage(): void {
  try { localStorage.removeItem(IMG_KEY); } catch { /* modo privado */ }
  if (getBgPref() === 'image') setBgPref('flat');
}

export function getBgPref(): BgPref {
  const v = typeof localStorage !== 'undefined' ? localStorage.getItem(KEY) : null;
  // 'image' sem imagem guardada (limpou o storage, trocou de dispositivo) cai pro chapado em vez de
  // deixar a tela num estado que nao existe.
  if (v === 'image') return getBgImage() ? 'image' : 'flat';
  return v === 'texture' || v === 'aurora' ? v : 'flat';
}

export function applyBg(pref: BgPref = getBgPref()): void {
  if (typeof document === 'undefined') return;
  const raiz = document.documentElement;
  if (pref === 'flat') delete raiz.dataset.bg;
  else raiz.dataset.bg = pref;
  // A imagem entra por variavel CSS: assim a folha de estilo desenha a camada (cobertura, scrim,
  // grao) e o TS so entrega o arquivo.
  const img = pref === 'image' ? getBgImage() : null;
  if (img) {
    raiz.style.setProperty('--cp-wallpaper', `url("${img}")`);
    aplicarScrim();
  } else {
    raiz.style.removeProperty('--cp-wallpaper');
    raiz.style.removeProperty('--cp-scrim-topo');
    raiz.style.removeProperty('--cp-scrim-base');
    raiz.style.removeProperty('--cp-panel-alpha');
    raiz.style.removeProperty('--cp-surface-alpha');
  }
}

// ── Duas escolhas de aparência que nasceram da comparação com wallpaper (mock em
// .claude/mock-fundo-imagem-completo.html). Nenhuma das duas é decisão minha: ficam no painel.

// LEITURA: onde o texto da conversa fica. 'glass' = como sempre foi (o vidro do resto vale também
// pra conversa). 'solid' = a conversa vira uma folha quase opaca e a foto aparece no cromo/margens.
// 'auto' escolhe 'solid' só quando há imagem de fundo — sem imagem não há o que resolver.
// 'text' NÃO desenha superfície nenhuma: sobe o contraste do texto e põe uma sombra curta atrás
// dele, então a foto continua aparecendo entre as mensagens. É o mais leve — a conversa é a
// PÁGINA, não um painel, e virar um retângulo do tamanho da tela é o que lê como "site estranho".
export type ReadMode = 'auto' | 'glass' | 'text' | 'solid';
const READ_KEY = 'cp_read';

export function getReadMode(): ReadMode {
  const v = typeof localStorage !== 'undefined' ? localStorage.getItem(READ_KEY) : null;
  return v === 'glass' || v === 'solid' || v === 'text' ? v : 'auto';
}

export function setReadMode(m: ReadMode): void {
  try {
    if (m === 'auto') localStorage.removeItem(READ_KEY);
    else localStorage.setItem(READ_KEY, m);
  } catch { /* modo privado */ }
  aplicarLeitura();
}

// Quanto a folha da conversa é opaca (0..100). Existe porque "sólida" no talo vira um bloco escuro
// por cima da foto: quem decide o ponto é quem está olhando, como já é no scrim do fundo.
const READ_ALPHA_KEY = 'cp_read_alpha';

export function getReadAlpha(): number {
  return lerNumero(READ_ALPHA_KEY, 92);
}

export function setReadAlpha(v: number): void {
  const n = Math.max(0, Math.min(100, Math.round(v)));
  try { localStorage.setItem(READ_ALPHA_KEY, String(n)); } catch { /* modo privado */ }
  aplicarLeitura();
}

// Quanto o texto da conversa volta pro branco no modo Texto (0..100). Separado da força da sombra:
// um resolve fundo claro na foto, o outro resolve contorno. 60 é o meio-termo medido.
const TEXT_BOOST_KEY = 'cp_text_boost';

// 10 leva #d2cbcd (12,0:1) a ~12,6:1 — o contraste medido do terminal do usuário (#E8D3DE sobre
// #171B20 = 12,7:1). 60 era chute; com o véu do fundo no ponto certo, quase nada de branco é preciso.
export function getTextBoost(): number {
  return lerNumero(TEXT_BOOST_KEY, 10);
}

export function setTextBoost(v: number): void {
  const n = Math.max(0, Math.min(100, Math.round(v)));
  try { localStorage.setItem(TEXT_BOOST_KEY, String(n)); } catch { /* modo privado */ }
  aplicarLeitura();
}

function aplicarLeitura(): void {
  if (typeof document === 'undefined') return;
  const m = getReadMode();
  // 'auto' liga o modo TEXTO (o mais leve) e só quando há foto — sem imagem não há o que resolver.
  const efetivo = m === 'auto' ? (getBgPref() === 'image' ? 'text' : null) : (m === 'glass' ? null : m);
  const raiz = document.documentElement;
  if (efetivo) {
    raiz.dataset.read = efetivo;
    raiz.style.setProperty('--cp-read-alpha', String(getReadAlpha()));
    raiz.style.setProperty('--cp-text-boost', String(getTextBoost()));
  } else {
    delete raiz.dataset.read;
    raiz.style.removeProperty('--cp-read-alpha');
    raiz.style.removeProperty('--cp-text-boost');
  }
}

// PAINÉIS: 'card' = caixa solta (folga, cantos redondos, sombra) · 'edge' = colado na borda, como
// era antes. Vale pro painel de contexto e pros painéis não-modais.
export type PanelStyle = 'card' | 'edge';
const PANEL_KEY = 'cp_panels';

export function getPanelStyle(): PanelStyle {
  const v = typeof localStorage !== 'undefined' ? localStorage.getItem(PANEL_KEY) : null;
  return v === 'edge' ? 'edge' : 'card';
}

export function setPanelStyle(s: PanelStyle): void {
  try {
    if (s === 'card') localStorage.removeItem(PANEL_KEY);
    else localStorage.setItem(PANEL_KEY, s);
  } catch { /* modo privado */ }
  aplicarPaineis();
}

function aplicarPaineis(): void {
  if (typeof document === 'undefined') return;
  if (getPanelStyle() === 'edge') document.documentElement.dataset.panels = 'edge';
  else delete document.documentElement.dataset.panels;
}

// FONTE da conversa. 'system' = a sans do sistema (`system-ui`, o de sempre) · 'mono' = a
// monoespaçada, que é o que faz o chat ler como o terminal. Não há como o navegador LER a fonte do
// terminal (nenhuma API expõe config do kitty, e `fc-match monospace` responde outra coisa), então a
// escolha é do usuário: `--font-mono` nomeia a família e cai no ui-monospace de quem não a tiver.
export type FontPref = 'system' | 'mono';
const FONT_KEY = 'cp_font';

export function getFontPref(): FontPref {
  const v = typeof localStorage !== 'undefined' ? localStorage.getItem(FONT_KEY) : null;
  return v === 'mono' ? 'mono' : 'system';
}

export function setFontPref(f: FontPref): void {
  try {
    if (f === 'system') localStorage.removeItem(FONT_KEY);
    else localStorage.setItem(FONT_KEY, f);
  } catch { /* modo privado */ }
  aplicarFonte();
}

function aplicarFonte(): void {
  if (typeof document === 'undefined') return;
  if (getFontPref() === 'mono') document.documentElement.dataset.font = 'mono';
  else delete document.documentElement.dataset.font;
}

// ── As tres medidas do texto da conversa ─────────────────────────────────────
// Tamanho, entrelinha e largura da coluna. Guardadas como ESCALA (100 = o de hoje), nao como valor
// absoluto: o app ja usa medidas diferentes por tela — 16px/1.6 no celular, 17px/1.7 e coluna de
// 920px no desktop, cada uma com um porque registrado no CSS. Um valor absoluto obrigaria a escolher
// UM que serve pros dois e apagaria essa diferenca; a escala respeita o padrao de cada tela e move
// os dois juntos.
//
// A largura so tem efeito no desktop: no celular a coluna ja e a tela inteira, e a linha e curta por
// falta de espaco, nao por escolha.
const TEXTO_KEYS = {
  size: 'cp_text_size',
  lh: 'cp_text_lh',
  width: 'cp_text_width',
} as const;

export type MedidaTexto = keyof typeof TEXTO_KEYS;

// 50..150 em vez de 0..100: aqui 100 e o MEIO (o padrao), nao o maximo — o valor tanto encolhe
// quanto aumenta. lerNumero limita a 0..100, entao esta faixa tem leitura propria.
function lerEscala(chave: string): number {
  const cru = typeof localStorage !== 'undefined' ? localStorage.getItem(chave) : null;
  const v = cru === null || cru === '' ? NaN : Number(cru);
  return Number.isFinite(v) && v >= 50 && v <= 150 ? v : 100;
}

export function getMedidaTexto(m: MedidaTexto): number {
  return lerEscala(TEXTO_KEYS[m]);
}

export function setMedidaTexto(m: MedidaTexto, v: number): void {
  const n = Math.max(50, Math.min(150, Math.round(v)));
  try {
    // 100 e o padrao e NAO grava chave — mesma convencao do tema, da fonte e dos paineis: so o
    // desvio persiste, entao quem nunca abriu isto continua exatamente como estava.
    if (n === 100) localStorage.removeItem(TEXTO_KEYS[m]);
    else localStorage.setItem(TEXTO_KEYS[m], String(n));
  } catch { /* modo privado */ }
  aplicarTexto();
}

function aplicarTexto(): void {
  if (typeof document === 'undefined') return;
  const raiz = document.documentElement;
  raiz.style.setProperty('--cp-text-scale', (getMedidaTexto('size') / 100).toFixed(3));
  raiz.style.setProperty('--cp-lh-scale', (getMedidaTexto('lh') / 100).toFixed(3));
  raiz.style.setProperty('--cp-width-scale', (getMedidaTexto('width') / 100).toFixed(3));
}

// Chamado no boot junto com applyBg: sem isto as escolhas só valeriam depois de mexer nelas.
export function applyAppearance(): void {
  aplicarLeitura();
  aplicarPaineis();
  aplicarFonte();
  aplicarTexto();
}

export function setBgPref(pref: BgPref): void {
  if (typeof localStorage !== 'undefined') {
    if (pref === 'flat') localStorage.removeItem(KEY);
    else localStorage.setItem(KEY, pref);
  }
  applyBg(pref);
  // A leitura em 'auto' depende do fundo: entrar/sair de Imagem tem que reavaliar na hora, senão a
  // folha sólida só apareceria no próximo boot.
  aplicarLeitura();
}
