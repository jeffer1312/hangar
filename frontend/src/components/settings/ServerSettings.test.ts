// @vitest-environment happy-dom
// "Pastas mapeadas" (Configurações → Avançado) e o seletor NATIVO de pasta. O mecanismo já existia
// no modal de "Nova sessão" e só não tinha sido levado pra cá — a tela obrigava a DIGITAR o caminho.
// Aqui escolher no diálogo ADICIONA direto (o clique no diálogo já é a resposta), e sem shell
// Electron a tela fica exatamente como era: campo de texto + Adicionar, que é o caminho de quem usa
// pelo navegador e pelo celular.
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { mount, unmount, tick } from 'svelte';
import ServerSettings from './ServerSettings.svelte';
import { criarProps } from './props-reativas.svelte';
import * as m from '../../paraglide/messages';
import type { ConfigServidorStore } from '../../lib/serverConfig.svelte';

vi.mock('../../lib/api', () => ({
  listarVozesTts: vi.fn(async () => []),
  saldoTts: vi.fn(async () => ({ usados: 0, limite: 0 })),
}));
vi.mock('../../lib/ttsPlayer.svelte', () => ({ ttsPlayer: { tocando: false, parar: vi.fn() } }));
vi.mock('../../lib/ouvir', () => ({ ouvirAmostra: vi.fn() }));

/** Store de mentira com UM campo reativo: o `scan_roots` é a string "a,b" (mesmo formato do
 *  CP_SCAN_ROOTS) e o `setRascunho` a reescreve, que é o que a tela faz de verdade. `criarProps`
 *  dá o $state — sem ele o `$derived` da lista não recalcularia depois do clique. */
function criarStore(inicial: string) {
  const estado = criarProps({ valor: inicial });
  const store = {
    get campos() { return { scan_roots: { valor: estado.valor, origem: 'env' } }; },
    get leitura() { return {}; },
    get carregando() { return false; },
    get salvando() { return false; },
    get erro() { return ''; },
    get salvo() { return false; },
    get temMudanca() { return false; },
    valorAtual: (k: string) => (k === 'scan_roots' ? estado.valor : ''),
    rascunhoDe: () => '',
    setRascunho: (k: string, v: unknown) => { if (k === 'scan_roots') estado.valor = String(v); },
    carregar: vi.fn(),
    salvar: vi.fn(),
    invalidar: vi.fn(),
  } as unknown as ConfigServidorStore;
  return { store, estado };
}

function montar(inicial = '/home/voce/projetos') {
  const { store, estado } = criarStore(inicial);
  const el = document.createElement('div');
  document.body.appendChild(el);
  const comp = mount(ServerSettings, { target: el, props: { store, secao: 'avancado' as const } });
  return { el, comp: comp as never, estado };
}

function botaoNativo(el: HTMLElement) {
  return [...el.querySelectorAll('button')].find((b) => b.textContent?.trim() === m.criar_pasta_computador());
}

beforeEach(() => { vi.clearAllMocks(); });
afterEach(() => { delete (window as unknown as { hangar?: unknown }).hangar; });

describe('ServerSettings — Pastas mapeadas e o seletor nativo', () => {
  it('sem window.hangar (navegador/celular): sem botão, e o campo de texto continua lá', async () => {
    const t = montar();
    await tick();
    expect(botaoNativo(t.el)).toBeUndefined();
    // O caminho manual NÃO pode sumir junto — é o único de quem não roda o shell.
    expect(t.el.querySelector<HTMLInputElement>('.raiz-add input')).not.toBeNull();
    unmount(t.comp);
  });

  it('com window.hangar.pickFolder: o botão aparece e escolher ADICIONA a pasta na lista', async () => {
    const pickFolder = vi.fn().mockResolvedValue('/home/jefferson/novo-projeto');
    (window as unknown as { hangar?: unknown }).hangar = { pickFolder };
    const t = montar('/home/voce/projetos');
    await tick();
    const btn = botaoNativo(t.el);
    expect(btn).toBeDefined();
    btn!.click();
    await tick(); await tick();
    expect(pickFolder).toHaveBeenCalledOnce();
    // Gravou no rascunho, no formato "a,b" — sem exigir um segundo clique em "Adicionar".
    expect(t.estado.valor).toBe('/home/voce/projetos,/home/jefferson/novo-projeto');
    const linhas = [...t.el.querySelectorAll('.raiz-caminho')].map((n) => n.textContent);
    expect(linhas).toContain('/home/jefferson/novo-projeto');
    unmount(t.comp);
  });

  it('cancelar o diálogo (null) não mexe na lista', async () => {
    (window as unknown as { hangar?: unknown }).hangar = { pickFolder: vi.fn().mockResolvedValue(null) };
    const t = montar('/home/voce/projetos');
    await tick();
    botaoNativo(t.el)!.click();
    await tick(); await tick();
    expect(t.estado.valor).toBe('/home/voce/projetos');
    unmount(t.comp);
  });

  it('pasta já mapeada não duplica', async () => {
    (window as unknown as { hangar?: unknown }).hangar = { pickFolder: vi.fn().mockResolvedValue('/home/voce/projetos') };
    const t = montar('/home/voce/projetos');
    await tick();
    botaoNativo(t.el)!.click();
    await tick(); await tick();
    expect(t.estado.valor).toBe('/home/voce/projetos');
    unmount(t.comp);
  });

  it('diálogo que falha vira erro na tela, não silêncio', async () => {
    (window as unknown as { hangar?: unknown }).hangar = {
      pickFolder: vi.fn().mockRejectedValue(new Error('dialog morreu')),
    };
    const t = montar();
    await tick();
    botaoNativo(t.el)!.click();
    await tick(); await tick();
    expect(t.el.querySelector('[role="alert"]')?.textContent).toContain('dialog morreu');
    unmount(t.comp);
  });

  it('clique duplo não abre dois diálogos concorrentes', async () => {
    // Dois abertos ao mesmo tempo resolvem fora de ordem e o último sobrescreveria o primeiro calado.
    let liberar: (v: string | null) => void = () => {};
    const pickFolder = vi.fn(() => new Promise<string | null>((res) => { liberar = res; }));
    (window as unknown as { hangar?: unknown }).hangar = { pickFolder };
    const t = montar();
    await tick();
    const btn = botaoNativo(t.el)!;
    btn.click();
    await tick();
    btn.click();
    await tick();
    expect(pickFolder).toHaveBeenCalledOnce();
    liberar(null);
    unmount(t.comp);
  });

  it('o campo de texto continua adicionando, e caminho repetido não apaga o que foi digitado', async () => {
    const t = montar('/home/voce/projetos');
    await tick();
    const form = t.el.querySelector<HTMLFormElement>('.raiz-add')!;
    const input = form.querySelector<HTMLInputElement>('input')!;
    input.value = '/home/voce/projetos';
    input.dispatchEvent(new Event('input', { bubbles: true }));
    await tick();
    form.dispatchEvent(new Event('submit', { bubbles: true, cancelable: true }));
    await tick();
    expect(t.estado.valor).toBe('/home/voce/projetos');
    expect(input.value).toBe('/home/voce/projetos');

    input.value = '/srv/outra';
    input.dispatchEvent(new Event('input', { bubbles: true }));
    await tick();
    form.dispatchEvent(new Event('submit', { bubbles: true, cancelable: true }));
    await tick();
    expect(t.estado.valor).toBe('/home/voce/projetos,/srv/outra');
    expect(input.value).toBe('');
    unmount(t.comp);
  });
});
