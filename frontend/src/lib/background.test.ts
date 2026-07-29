import { describe, it, expect } from 'vitest';

// Mesmo stub do auth.test.ts: background.ts lê localStorage no load. env=node não tem.
const store = new Map<string, string>();
(globalThis as any).localStorage = {
  getItem: (k: string) => (store.has(k) ? store.get(k)! : null),
  setItem: (k: string, v: string) => store.set(k, String(v)),
  removeItem: (k: string) => store.delete(k),
};

const { getBgScrim, getReadAlpha, getTextBoost, getFontPref, setFontPref } = await import('./background');

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
