import { describe, it, expect, beforeEach } from 'vitest';

// Mesmo stub do auth.test.ts: background.ts lê localStorage no load. env=node não tem.
const store = new Map<string, string>();
// `falhaAoGravar` simula o Safari em modo privado / cota estourada: setItem levanta. Sem isto o
// stub nunca falha e o caminho do catch — o mais fácil de escrever errado — fica sem teste.
let falhaAoGravar = false;
(globalThis as any).localStorage = {
  getItem: (k: string) => (store.has(k) ? store.get(k)! : null),
  setItem: (k: string, v: string) => {
    if (falhaAoGravar) throw new DOMException('quota', 'QuotaExceededError');
    store.set(k, String(v));
  },
  removeItem: (k: string) => store.delete(k),
};

// O <html> só existe no jsdom; nos testes de node basta um objeto que guarde o que foi setado.
const varsCss = new Map<string, string>();
(globalThis as any).document = {
  documentElement: {
    style: {
      setProperty: (k: string, v: string) => varsCss.set(k, v),
      removeProperty: (k: string) => varsCss.delete(k),
    },
    dataset: {},
  },
};

const { getBgScrim, getReadAlpha, getTextBoost, getFontPref, setFontPref, getSurfaceSolid, setSurfaceSolid, setBgScrim, getMedidaTexto, setMedidaTexto, getBackdropBlur, setBackdropBlur, applyBg, applyAppearance, getBgPref, isShell } = await import('./background');

// Fonte: 'system' é o padrão e NÃO grava chave (mesma convenção do tema/painéis — só o desvio do
// padrão persiste). Lixo na chave cai em 'system' em vez de deixar o app numa fonte que não existe.

// Desfoque do fundo: mesma convenção da fonte/'off' default. 0 = não grava nada; 'light'/'strong'
// gravam; lixo volta pra 'off'. Efeito PRÁTICO vem do `--cp-backdrop-blur` que vai pro <html> — ele
// quem o wallpaper ::after depois lê pra aplicar blur(). Se um dia alguém mudar o conjunto de opções
// sem atualizar o picker da Aparência, este teste quebra ANTES do usuário; é pra isso que ele existe.
describe('desfoque do fundo', () => {
  it('padrão é off e não grava nada', () => {
    store.clear();
    expect(getBackdropBlur()).toBe('off');
    expect(store.has('cp_backdrop_blur')).toBe(false);
  });

  it('cada nível persiste e voltar pra off remove a chave', () => {
    store.clear();
    setBackdropBlur('light');
    expect(store.get('cp_backdrop_blur')).toBe('light');
    expect(getBackdropBlur()).toBe('light');
    setBackdropBlur('strong');
    expect(getBackdropBlur()).toBe('strong');
    setBackdropBlur('off');
    expect(store.has('cp_backdrop_blur')).toBe(false);
    expect(getBackdropBlur()).toBe('off');
  });

  it('lixo na chave volta pro padrão', () => {
    store.clear();
    store.set('cp_backdrop_blur', '99');
    expect(getBackdropBlur()).toBe('off');
    store.set('cp_backdrop_blur', 'blur');
    expect(getBackdropBlur()).toBe('off');
  });
});
describe('escolha de fonte', () => {
  it('padrão é system, mono persiste, lixo volta pro padrão', () => {
    store.clear();
    expect(getFontPref()).toBe('system');
    setFontPref('mono');
    expect(store.get('cp_font')).toBe('mono');
    expect(getFontPref()).toBe('mono');
    setFontPref('system');
    expect(store.has('cp_font')).toBe(false);
    store.set('cp_font', 'comic-sans');
    expect(getFontPref()).toBe('system');
  });
});

// O bug que este arquivo existe pra travar: `Number(null)` é 0, não NaN. Lendo uma chave que nunca
// foi escrita, o 0 passava pelo teste de faixa (0..100) e virava o valor "escolhido" — o app abria
// com o slider no chão e o modo Texto inerte, até alguém arrastar. Sem teste, passou.
describe('padrões quando a chave nunca foi escrita', () => {
  it('devolve o padrão, não 0', () => {
    store.clear();
    expect(getBgScrim()).toBe(11);
    expect(getReadAlpha()).toBe(92);
    expect(getTextBoost()).toBe(10);
  });

  it('0 gravado de propósito continua valendo 0', () => {
    store.clear();
    store.set('cp_bg_scrim', '0');
    store.set('cp_read_alpha', '0');
    store.set('cp_text_boost', '0');
    expect(getBgScrim()).toBe(0);
    expect(getReadAlpha()).toBe(0);
    expect(getTextBoost()).toBe(0);
  });

  it('lixo e valor fora da faixa caem no padrão', () => {
    store.clear();
    store.set('cp_bg_scrim', 'abc');
    store.set('cp_read_alpha', '140');
    store.set('cp_text_boost', '');
    expect(getBgScrim()).toBe(11);
    expect(getReadAlpha()).toBe(92);
    expect(getTextBoost()).toBe(10);
  });
});

// Solidez das caixas: mesmo contrato dos outros sliders (padrão quando a chave não existe, 0 vale 0,
// lixo cai no padrão) mais o clamp na escrita — o valor vira alfa de cor, e alfa fora de 0..1
// pintaria caixa nenhuma ou caixa opaca sem ninguém entender de onde veio.
describe('solidez das caixas', () => {
  it('padrão 12, 0 vale 0, lixo cai no padrão', () => {
    store.clear();
    expect(getSurfaceSolid()).toBe(12);
    store.set('cp_surface_solid', '0');
    expect(getSurfaceSolid()).toBe(0);
    store.set('cp_surface_solid', 'abc');
    expect(getSurfaceSolid()).toBe(12);
  });

  it('escrita prende na faixa 0..100 e arredonda', () => {
    store.clear();
    setSurfaceSolid(-30);
    expect(getSurfaceSolid()).toBe(0);
    setSurfaceSolid(999);
    expect(getSurfaceSolid()).toBe(100);
    setSurfaceSolid(41.6);
    expect(getSurfaceSolid()).toBe(42);
  });
});

// As tres medidas do texto são ESCALA, não valor: 100 é o meio da faixa (o padrão de cada tela), e o
// slider vai de 50 a 150. Os outros sliders do arquivo são 0..100, então a leitura destas é própria —
// um `lerNumero` compartilhado descartaria 150 como fora da faixa e o slider voltaria sozinho pro
// meio ao recarregar.
describe('medidas do texto da conversa', () => {
  it('padrão é 100 e não grava chave; só o desvio persiste', () => {
    store.clear();
    expect(getMedidaTexto('size')).toBe(100);
    setMedidaTexto('size', 100);
    expect(store.has('cp_text_size')).toBe(false);
    setMedidaTexto('size', 130);
    expect(store.get('cp_text_size')).toBe('130');
    expect(getMedidaTexto('size')).toBe(130);
  });

  it('aceita a faixa inteira 50..150 e prende fora dela', () => {
    store.clear();
    setMedidaTexto('lh', 150);
    expect(getMedidaTexto('lh')).toBe(150);
    setMedidaTexto('lh', 400);
    expect(getMedidaTexto('lh')).toBe(150);
    setMedidaTexto('width', 10);
    expect(getMedidaTexto('width')).toBe(50);
  });

  it('as três são independentes', () => {
    store.clear();
    setMedidaTexto('size', 120);
    setMedidaTexto('width', 80);
    expect(getMedidaTexto('size')).toBe(120);
    expect(getMedidaTexto('lh')).toBe(100);
    expect(getMedidaTexto('width')).toBe(80);
  });
});

// Gravacao que falha (modo privado) nao pode deixar a TELA no valor velho: o slider ja mostra o
// numero novo, entao a variavel CSS tem que acompanhar mesmo sem persistir. Sem isto o usuario
// arrasta, ve o numero mudar, nada acontece na tela e nenhum aviso aparece.
describe('preferência que não consegue persistir ainda vale nesta sessão', () => {
  it('solidez aplica a variável CSS mesmo com setItem falhando', () => {
    store.clear(); varsCss.clear();
    falhaAoGravar = true;
    try { setSurfaceSolid(60); } finally { falhaAoGravar = false; }
    expect(store.has('cp_surface_solid')).toBe(false);          // não persistiu
    expect(varsCss.get('--cp-surface-alpha')).toBeDefined();     // mas valeu agora
  });

  it('as medidas do texto aplicam a escala pedida mesmo com setItem falhando', () => {
    store.clear(); varsCss.clear();
    falhaAoGravar = true;
    try { setMedidaTexto('size', 130); } finally { falhaAoGravar = false; }
    expect(store.has('cp_text_size')).toBe(false);
    expect(varsCss.get('--cp-text-scale')).toBe('1.300');
  });
});

// A Solidez SOMAVA no alfa do painel e estourava no teto de 1: com Transparencia 38 o slider ja
// chegava em opaco na marca 27, e os dois tercos seguintes nao mudavam nada — o usuario percebeu
// como "nao funciona". Agora interpola, entao o curso inteiro tem efeito.
describe('solidez usa o curso inteiro do slider', () => {
  const alfa = () => Number(varsCss.get('--cp-surface-alpha'));

  it('0 = igual ao painel, 100 = opaco, e o meio fica entre os dois', () => {
    store.clear(); varsCss.clear();
    setBgScrim(38);                    // painel = 0.92 - 0.38*0.50 = 0.73
    setSurfaceSolid(0);
    const noChao = alfa();
    expect(noChao).toBeCloseTo(0.73, 3);
    setSurfaceSolid(100);
    expect(alfa()).toBe(1);
    setSurfaceSolid(50);
    const meio = alfa();
    expect(meio).toBeGreaterThan(noChao);
    expect(meio).toBeLessThan(1);
  });

  it('nenhuma marca abaixo de 100 satura — o que matava o slider antes', () => {
    store.clear(); varsCss.clear();
    setBgScrim(38);
    for (const v of [30, 60, 90, 99]) {
      setSurfaceSolid(v);
      expect(alfa()).toBeLessThan(1);
    }
  });
});

// `navigator` é global NATIVO no Node >= 21 e módulo ESM roda em strict mode: atribuir por cima
// levanta TypeError. defineProperty é o caminho que funciona nas duas versões.
function comUserAgent(ua: string) {
  Object.defineProperty(globalThis, 'navigator', {
    value: { userAgent: ua }, configurable: true, writable: true,
  });
}

describe("fundo 'desktop' (shell Electron)", () => {
  beforeEach(() => { varsCss.clear(); comUserAgent('Mozilla/5.0'); });

  it('sem a marca no user agent, isShell é falso e desktop cai pro chapado', () => {
    store.set('cp_bg', 'desktop');
    expect(isShell()).toBe(false);
    // O modo só existe dentro do shell: fora dele a preferência não pode deixar a tela
    // sem fundo nenhum.
    expect(getBgPref()).toBe('flat');
  });

  it('com a marca, desktop se mantém', () => {
    store.clear();
    comUserAgent('Mozilla/5.0 claude-cockpit-shell');
    store.set('cp_bg', 'desktop');
    expect(isShell()).toBe(true);
    expect(getBgPref()).toBe('desktop');
  });

  it('applyBg(desktop) marca o html e NÃO define wallpaper', () => {
    comUserAgent('Mozilla/5.0 claude-cockpit-shell');
    applyBg('desktop');
    const raiz = document.documentElement;
    expect(raiz.dataset.bg).toBe('desktop');
    // A foto é justamente o que o desktop substitui.
    expect(varsCss.get('--cp-wallpaper')).toBeUndefined();
    // Mas o véu roda: é ele que torna os tokens de superfície translúcidos.
    expect(varsCss.get('--cp-panel-alpha')).toBeDefined();
    expect(varsCss.get('--cp-surface-alpha')).toBeDefined();
  });

  it('no modo desktop o auto liga o reforço de texto', () => {
    comUserAgent('Mozilla/5.0 claude-cockpit-shell');
    store.clear();
    store.set('cp_bg', 'desktop');
    applyBg('desktop');
    applyAppearance();
    expect(document.documentElement.dataset.read).toBe('text');
  });

  it('sem shell e sem imagem, o auto não liga nada', () => {
    comUserAgent('Mozilla/5.0');
    store.clear();
    applyBg('flat');
    applyAppearance();
    expect(document.documentElement.dataset.read).toBeUndefined();
  });
});
