import { describe, it, expect } from 'vitest';

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

const { getBgScrim, getReadAlpha, getTextBoost, getFontPref, setFontPref, getSurfaceSolid, setSurfaceSolid, getMedidaTexto, setMedidaTexto } = await import('./background');

// Fonte: 'system' é o padrão e NÃO grava chave (mesma convenção do tema/painéis — só o desvio do
// padrão persiste). Lixo na chave cai em 'system' em vez de deixar o app numa fonte que não existe.
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
