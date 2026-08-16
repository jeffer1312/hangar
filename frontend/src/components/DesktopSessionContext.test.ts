// @vitest-environment happy-dom
// Follow-up visual: com toggle externo (barra/rail), o DesktopSessionContext NÃO
// pode ter botão duplicado (.ctx-fold) nem aba vertical central quando recolhido — o painel
// simplesmente some. Sem toggle externo (sidebar expandida), a porta acessível do painel continua.
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { mount, unmount, tick } from 'svelte';
import DesktopSessionContext from './DesktopSessionContext.svelte';
import { ctxPanel } from '../lib/ctxPanel.svelte';
import { overwriteGetLocale } from '../paraglide/runtime';
import { listFiles } from '../lib/api';

// Stubs dos componentes internos pesados (PlanRing/PlanPanel renderizam SVG/estado de plano).
vi.mock('./PlanRing.svelte', () => ({ default: class { $destroy() {} } }));
vi.mock('./PlanPanel.svelte', () => ({ default: class { $destroy() {} } }));

// A aba Arquivos (FilesPanel) fala com a rede no mount — sem o mock o teste montaria o
// componente com fetch de verdade.
vi.mock('../lib/api', () => ({
  listFiles: vi.fn().mockResolvedValue({ entries: [], truncated: false }),
  readFile: vi.fn(),
  searchFiles: vi.fn(),
  pathDiff: vi.fn(),
}));

function montar(toggleExterno: boolean) {
  const el = document.createElement('div');
  document.body.appendChild(el);
  const comp = mount(DesktopSessionContext, {
    target: el,
    props: {
      state: 'idle',
      sessionName: 'sess-1',
      toggleExterno,
    },
  });
  return { el, comp: comp as never };
}

beforeEach(() => {
  overwriteGetLocale(() => 'pt');   // textos dos painéis são mensagens agora
  ctxPanel.recolhido = false;
  ctxPanel.aba = 'contexto';        // a aba vive no modulo — reset entre testes
  document.body.innerHTML = '';
});

describe('DesktopSessionContext — toggle na barra (follow-up visual)', () => {
  it('toggleExterno: NENHUM .ctx-fold (nem no aberto) — sem botão duplicado', async () => {
    const t = montar(true);
    await tick();
    expect(document.querySelector('.ctx-fold')).toBeNull();
    unmount(t.comp);
  });

  it('toggleExterno + recolhido: painel some (sem aba vertical central)', async () => {
    const t = montar(true);
    await tick();
    ctxPanel.recolhido = true;
    await tick();
    const aside = document.querySelector<HTMLElement>('.session-context');
    expect(aside?.classList.contains('recolhido')).toBe(true);
    expect(aside?.classList.contains('toggle-externo')).toBe(true);
    // nenhuma aba vertical: não existe .ctx-fold (o display:none da regra recolhido+toggle-externo
    // é validado no browser — happy-dom não injeta o CSS escopado)
    expect(document.querySelector('.ctx-fold')).toBeNull();
    unmount(t.comp);
  });

  it('sem toggleExterno (sidebar expandida): porta acessível no TOPO preservada — sem aba flutuante isolada', async () => {
    const t = montar(false);
    await tick();
    const fold = document.querySelector<HTMLButtonElement>('.ctx-fold');
    expect(fold).not.toBeNull();
    expect(fold?.getAttribute('aria-label')).toBe('Recolher contexto');
    ctxPanel.recolhido = true;
    await tick();
    // A porta continua (botão do header, não aba 26×64 top:50%): o CSS da aba isolada morreu —
    // o ctx-fold fica na posição do header (estático no topo), não flutuando no meio da borda.
    expect(document.querySelector('.ctx-fold')).not.toBeNull();
    const aside = document.querySelector<HTMLElement>('.session-context')!;
    expect(aside.classList.contains('recolhido')).toBe(true);
    expect(aside.classList.contains('toggle-externo')).toBe(false);
    unmount(t.comp);
  });

  it('a barra de abas tem Contexto e Arquivos, com a primeira ativa', async () => {
    const t = montar(false);
    await tick();
    const abas = [...document.querySelectorAll('.aba')];
    expect(abas.map((a) => a.textContent?.trim())).toEqual(['Contexto', 'Arquivos']);
    expect(abas[0].getAttribute('aria-selected')).toBe('true');
    expect(abas[1].getAttribute('aria-selected')).toBe('false');
    unmount(t.comp);
  });

  it('aba Arquivos monta o FilesPanel e lista a sessao', async () => {
    const t = montar(false);
    await tick();
    const arq = [...document.querySelectorAll('.aba')][1] as HTMLButtonElement;
    arq.click();
    await tick();
    await tick();   // o onMount do FilesPanel -> recarregar -> listFiles
    expect(document.querySelector('.files-panel')).not.toBeNull();
    expect(listFiles).toHaveBeenCalledWith('sess-1', undefined, true);
    unmount(t.comp);
  });
});
