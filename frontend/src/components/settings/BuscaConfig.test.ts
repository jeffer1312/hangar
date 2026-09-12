// @vitest-environment happy-dom
import { describe, it, expect, vi, afterEach } from 'vitest';
import { mount, unmount, tick } from 'svelte';
import BuscaConfig, { ENTRADAS, TITULO_TELA, filtrar } from './BuscaConfig.svelte';
import { overwriteGetLocale } from '../../paraglide/runtime';
import * as m from '../../paraglide/messages';

// Os três termos da spec são PORTUGUESES ("cota", "reiniciar", "voz") e o baseLocale do projeto é
// 'en': sem fixar o locale, as mensagens resolvem em inglês e a busca não acharia nada — o teste
// passaria a medir o idioma, não a busca.
overwriteGetLocale(() => 'pt');

afterEach(() => { document.body.innerHTML = ''; });

function montar(semServidor = false) {
  const el = document.createElement('div');
  document.body.appendChild(el);
  const onIrPara = vi.fn();
  const comp = mount(BuscaConfig, { target: el, props: { onIrPara, semServidor } });
  const campo = el.querySelector('input') as HTMLInputElement;
  return { el, comp: comp as never, onIrPara, campo };
}

async function digitar(campo: HTMLInputElement, texto: string) {
  campo.value = texto;
  campo.dispatchEvent(new Event('input'));
  await tick();
}

describe('BuscaConfig — os três termos da spec abrem a tela certa', () => {
  // Um caso por termo, com o destino ESPERADO: trocar a tela de uma entrada (ou apontá-la para a
  // rota errada) derruba exatamente este teste.
  const CASOS: [string, string][] = [['cota', 'contas'], ['reiniciar', 'avancado'], ['voz', 'voz']];

  for (const [termo, tela] of CASOS) {
    it(`"${termo}" acha um controle de ${tela} e clicar navega pra lá`, async () => {
      const t = montar();
      await digitar(t.campo, termo);

      const achados = filtrar(termo);
      expect(achados.length).toBeGreaterThan(0);
      const alvo = achados.findIndex((a) => a.entrada.tela === tela);
      expect(alvo, `nenhum resultado de "${termo}" leva a ${tela}`).toBeGreaterThanOrEqual(0);

      const itens = [...t.el.querySelectorAll<HTMLButtonElement>('.bc-item')];
      expect(itens.length).toBe(achados.length);
      itens[alvo].click();
      expect(t.onIrPara).toHaveBeenCalledWith(tela);
      unmount(t.comp);
    });
  }

  it('cada resultado mostra "Tela › Controle"', async () => {
    const t = montar();
    await digitar(t.campo, 'voz');
    const primeiro = t.el.querySelector('.bc-item') as HTMLElement;
    expect(primeiro.querySelector('.bc-tela')?.textContent).toBeTruthy();
    expect(primeiro.querySelector('.bc-rot')?.textContent).toBeTruthy();
    unmount(t.comp);
  });

  it('campo vazio não lista nada; termo sem par diz que não achou', async () => {
    const t = montar();
    expect(t.el.querySelector('.bc-lista')).toBeNull();
    expect(t.el.querySelector('.bc-nada')).toBeNull();
    await digitar(t.campo, 'zzzznadaaqui');
    expect(t.el.querySelector('.bc-lista')).toBeNull();
    expect(t.el.querySelector('.bc-nada')).not.toBeNull();
    unmount(t.comp);
  });

  it('acento não separa: "voz" e "vóz" dão o mesmo resultado', () => {
    expect(filtrar('vóz').length).toBe(filtrar('voz').length);
  });

  it('abrir um resultado limpa o campo — voltar ao modal não cai na busca anterior', async () => {
    const t = montar();
    await digitar(t.campo, 'voz');
    (t.el.querySelector('.bc-item') as HTMLButtonElement).click();
    await tick();
    expect(t.el.querySelector('.bc-lista')).toBeNull();
    unmount(t.comp);
  });

  // A busca não pode ser porta lateral: sem servidor resolvido, a lista da raiz já apaga as telas
  // de servidor com o motivo — o resultado da busca faz o mesmo, em vez de sumir ou navegar.
  it('sem servidor, resultado de tela de servidor fica apagado com o motivo e não navega', async () => {
    const t = montar(true);
    await digitar(t.campo, 'cota');
    const itens = [...t.el.querySelectorAll<HTMLButtonElement>('.bc-item')];
    expect(itens.length).toBeGreaterThan(0);
    expect(itens.every((b) => b.disabled)).toBe(true);
    expect(t.el.querySelector('.bc-motivo')?.textContent).toBe(m.config_modal_escolha_servidor());
    itens[0].click();
    expect(t.onIrPara).not.toHaveBeenCalled();
    unmount(t.comp);
  });

  it('sem servidor, tela do aparelho continua clicável', async () => {
    const t = montar(true);
    await digitar(t.campo, 'tema');
    const item = t.el.querySelector<HTMLButtonElement>('.bc-item');
    expect(item?.disabled).toBe(false);
    item?.click();
    expect(t.onIrPara).toHaveBeenCalledWith('aparencia');
    unmount(t.comp);
  });

  it('o resultado se anuncia por inteiro: "Tela: Controle" no nome acessível', async () => {
    const t = montar();
    await digitar(t.campo, 'voz');
    const rotulo = t.el.querySelector('.bc-item')?.getAttribute('aria-label') ?? '';
    expect(rotulo).toContain(': ');
    expect(rotulo).not.toContain('›');
    unmount(t.comp);
  });

  // A região viva anuncia a CONTAGEM, não a lista: com os 72 rótulos lá dentro, cada tecla
  // digitada faz o leitor de tela reler a lista inteira.
  it('a região viva anuncia só quantos resultados há, e a lista fica fora dela', async () => {
    const t = montar();
    await digitar(t.campo, 'voz');
    const vivo = t.el.querySelector('[aria-live="polite"]') as HTMLElement;
    expect(vivo.querySelector('.bc-lista')).toBeNull();
    expect(vivo.textContent?.trim()).toBe(m.config_busca_resultados({ n: filtrar('voz').length }));
    unmount(t.comp);
  });

  it('sem resultado, a região viva diz que não achou', async () => {
    const t = montar();
    await digitar(t.campo, 'zzzznadaaqui');
    const vivo = t.el.querySelector('[aria-live="polite"]') as HTMLElement;
    expect(vivo.textContent?.trim()).toBe(m.busca_nenhum_resultado());
    unmount(t.comp);
  });

  // A lista aponta para FUNÇÕES de mensagem, nunca para texto: um rótulo escrito à mão aqui (ou
  // congelado no carregamento) prende a busca no idioma do primeiro acesso e some do i18n. Trocar
  // UMA entrada por `() => 'Tema'` derruba exatamente esta asserção, nomeando a entrada.
  it('toda entrada aponta para uma função de mensagem do Paraglide', () => {
    const doModulo = new Set<unknown>(Object.values(m));
    for (const e of ENTRADAS) {
      expect(doModulo.has(e.rotulo), `rótulo fora do i18n em ${e.tela}: ${e.rotulo()}`).toBe(true);
      if (e.descricao) {
        expect(doModulo.has(e.descricao), `descrição fora do i18n em ${e.tela}: ${e.rotulo()}`).toBe(true);
      }
    }
  });

  // E o efeito prático disso: a lista responde à troca de idioma.
  it('os rótulos seguem o idioma vigente, não o do carregamento', () => {
    expect(filtrar('tema').some((a) => a.entrada.tela === 'aparencia')).toBe(true);
    overwriteGetLocale(() => 'en');
    try {
      expect(filtrar('theme').some((a) => a.entrada.tela === 'aparencia')).toBe(true);
    } finally {
      overwriteGetLocale(() => 'pt');
    }
  });

  // Costura da spec: nenhuma tela do modal pode ficar inalcançável pela busca.
  it('toda tela do modal, fora a raiz, tem pelo menos uma entrada', () => {
    const telas = Object.keys(TITULO_TELA).filter((t) => t !== 'root');
    for (const tela of telas) {
      expect(
        ENTRADAS.some((e) => e.tela === tela),
        `a tela "${tela}" não tem entrada na busca`,
      ).toBe(true);
    }
  });

  it('nenhuma entrada aponta para a raiz nem para uma tela repetida com o mesmo rótulo', () => {
    const vistos = new Set<string>();
    for (const e of ENTRADAS) {
      const chave = `${e.tela}|${e.rotulo()}`;
      expect(vistos.has(chave), `entrada repetida: ${chave}`).toBe(false);
      vistos.add(chave);
    }
  });
});
