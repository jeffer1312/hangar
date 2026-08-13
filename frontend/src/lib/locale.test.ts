// @vitest-environment happy-dom
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { LOCALES, localeAtual, preferenciaSalva, aplicarPreferencia } from './locale';

// A chave real vem do Step 5 — trocar CHAVE pelo valor lido em src/paraglide/runtime.js.
const CHAVE = 'PARAGLIDE_LOCALE';

describe('preferencia de idioma', () => {
  beforeEach(() => { localStorage.clear(); });

  it('sem nada salvo, a preferencia e "sistema"', () => {
    expect(preferenciaSalva()).toBe('sistema');
  });

  it('com override salvo, devolve o idioma escolhido', () => {
    localStorage.setItem(CHAVE, 'en');
    expect(preferenciaSalva()).toBe('en');
  });

  it('valor invalido no storage nao vira idioma: cai em "sistema"', () => {
    localStorage.setItem(CHAVE, 'klingon');
    expect(preferenciaSalva()).toBe('sistema');
  });

  it('escolher "sistema" APAGA a chave — nao grava a string "sistema"', () => {
    localStorage.setItem(CHAVE, 'en');
    const recarregar = vi.fn();
    aplicarPreferencia('sistema', recarregar);
    expect(localStorage.getItem(CHAVE)).toBeNull();
    expect(recarregar).toHaveBeenCalledOnce();
  });

  it('so ha dois idiomas, e o ingles vem primeiro (e o baseLocale)', () => {
    expect([...LOCALES]).toEqual(['en', 'pt']);
  });

  // Degrau 2 da cadeia: sem escolha salva, manda o idioma do sistema.
  // O mock e ['pt-BR'] SOZINHO de proposito — e o que o Safari do iOS costuma reportar, e e o
  // aparelho alvo deste app. Se o matching do Paraglide for exato contra locales ['en','pt'],
  // 'pt-BR' nao casa, cai no baseLocale = INGLES, calado, em todo iPhone brasileiro. Um mock com
  // ['pt-BR','pt'] esconderia exatamente esse caso.
  it('sistema reportando so "pt-BR", sem escolha salva, abre em portugues', () => {
    vi.spyOn(navigator, 'languages', 'get').mockReturnValue(['pt-BR']);
    expect(localeAtual()).toBe('pt');
    expect(preferenciaSalva()).toBe('sistema');
  });

  // Degrau 3: sistema num idioma que o app nao tem. Sem esta garantia o usuario com o telefone em
  // espanhol veria a tela sem nenhuma opcao marcada e nao saberia em que idioma esta.
  it('sistema em idioma que o app nao tem cai no ingles, e a tela continua utilizavel', () => {
    vi.spyOn(navigator, 'languages', 'get').mockReturnValue(['es-AR', 'es']);
    expect(localeAtual()).toBe('en');
    expect(preferenciaSalva()).toBe('sistema');
  });
});
