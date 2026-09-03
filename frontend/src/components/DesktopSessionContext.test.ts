// @vitest-environment happy-dom
// Follow-up visual: com toggle externo (barra/rail), o DesktopSessionContext NÃO
// pode ter botão duplicado (.ctx-fold) nem aba vertical central quando recolhido — o painel
// simplesmente some. Sem toggle externo (sidebar expandida), a porta acessível do painel continua.
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { mount, unmount, tick } from 'svelte';
import DesktopSessionContext from './DesktopSessionContext.svelte';
import { ctxPanel, LARGURA_MIN, LARGURA_ABERTO } from '../lib/ctxPanel.svelte';
import { overwriteGetLocale } from '../paraglide/runtime';
import { listFiles } from '../lib/api';

// Stubs dos componentes internos pesados (PlanRing/PlanPanel renderizam SVG/estado de plano).
vi.mock('./PlanRing.svelte', () => ({ default: class { $destroy() {} } }));
vi.mock('./PlanPanel.svelte', () => ({ default: class { $destroy() {} } }));

// A aba Arquivos (FilesPanel) fala com a rede no mount — sem o mock o teste montaria o
// componente com fetch de verdade.
vi.mock('../lib/api', () => ({
  getPermissionModes: vi.fn().mockResolvedValue({ current: 'plan', modes: ['plan', 'auto', 'manual', 'acceptEdits'] }),
  setPermissionMode: vi.fn().mockResolvedValue({ mode: 'plan', current: 'plan' }),
  isTimeoutError: vi.fn(() => false),
  isAbortError: vi.fn(() => false),
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
      serverId: 'srv-test',
      toggleExterno,
    },
  });
  return { el, comp: comp as never };
}

beforeEach(() => {
  overwriteGetLocale(() => 'pt');   // textos dos painéis são mensagens agora
  ctxPanel.recolhido = false;
  ctxPanel.aba = 'contexto';        // a aba vive no modulo — reset entre testes
  ctxPanel.largura = LARGURA_ABERTO; // idem: largura vive no modulo
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

  it('a barra de abas tem Contexto, Arquivos e Navegador, com a primeira ativa', async () => {
    const t = montar(false);
    await tick();
    const abas = [...document.querySelectorAll('.aba')];
    expect(abas.map((a) => a.textContent?.trim())).toEqual(['Contexto', 'Arquivos', 'Navegador']);
    expect(abas[0].getAttribute('aria-selected')).toBe('true');
    expect(abas[1].getAttribute('aria-selected')).toBe('false');
    expect(abas[2].getAttribute('aria-selected')).toBe('false');
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

// Task 17: divisória redimensionável. O handle existe só com o painel ABERTO (recolhido não é
// largura), e o arrasto segue o MESMO contrato da Sidebar: pointerdown captura, move atualiza o
// store, soltar persiste. O happy-dom não implementa setPointerCapture — stub no teste, o
// comportamento real é do navegador (mesma limitação do teste da Sidebar).
describe('DesktopSessionContext — divisória redimensionável (task 17)', () => {
  it('handle presente com painel aberto, ausente com painel recolhido', async () => {
    const t = montar(false);
    await tick();
    const handle = document.querySelector<HTMLElement>('.ctx-resize-handle');
    expect(handle).not.toBeNull();
    expect(handle?.getAttribute('role')).toBe('separator');
    expect(handle?.getAttribute('aria-label')).toBe('Redimensionar painel de contexto');
    ctxPanel.recolhido = true;
    await tick();
    expect(document.querySelector('.ctx-resize-handle')).toBeNull();
    unmount(t.comp);
  });

  it('arrastar atualiza a largura e soltar persiste', async () => {
    Object.defineProperty(window, 'innerWidth', { value: 1600, configurable: true });
    const t = montar(false);
    await tick();
    const origCapture = HTMLElement.prototype.setPointerCapture;
    HTMLElement.prototype.setPointerCapture = vi.fn();
    try {
      const handle = document.querySelector<HTMLElement>('.ctx-resize-handle')!;
      // janela 1600 -> teto 560; clientX 1200 -> 400, dentro da faixa clampsa
      handle.dispatchEvent(new PointerEvent('pointerdown', { pointerId: 1, clientX: 1200, bubbles: true }));
      handle.dispatchEvent(new PointerEvent('pointermove', { pointerId: 1, clientX: 1200, bubbles: true }));
      expect(ctxPanel.largura).toBe(window.innerWidth - 1200);
      handle.dispatchEvent(new PointerEvent('pointerup', { pointerId: 1, bubbles: true }));
      expect(localStorage.getItem('cp_ctx_w')).toBe(String(window.innerWidth - 1200));
    } finally {
      HTMLElement.prototype.setPointerCapture = origCapture;
    }
    unmount(t.comp);
  });

  it('arrastar além do mínimo clampa no piso', async () => {
    Object.defineProperty(window, 'innerWidth', { value: 1600, configurable: true });
    const t = montar(false);
    await tick();
    const origCapture = HTMLElement.prototype.setPointerCapture;
    HTMLElement.prototype.setPointerCapture = vi.fn();
    try {
      const handle = document.querySelector<HTMLElement>('.ctx-resize-handle')!;
      handle.dispatchEvent(new PointerEvent('pointerdown', { pointerId: 1, clientX: 5000, bubbles: true }));
      handle.dispatchEvent(new PointerEvent('pointermove', { pointerId: 1, clientX: 5000, bubbles: true }));
      expect(ctxPanel.largura).toBe(LARGURA_MIN);
    } finally {
      HTMLElement.prototype.setPointerCapture = origCapture;
    }
    unmount(t.comp);
  });

  // A alca pode sair do DOM no meio do arrasto (recolher, cruzar os 820px, trocar de sessao) — o
  // pointerup fica sem destino e o flag do store (singleton) ficaria preso, fazendo a divisoria
  // redimensionar so com o cursor por cima. Recolher NAO desmonta o componente (a alca some por
  // {#if} interno) — o zero vem do $effect reagindo ao recolhido; os outros caminhos desmontam o
  // componente inteiro e o cleanup cobre.
  it('desmontar no meio do arrasto zera o resizing', async () => {
    const t = montar(false);
    await tick();
    ctxPanel.resizing = true;   // arrasto em curso
    unmount(t.comp);            // alca sai do DOM sem pointerup
    expect(ctxPanel.resizing).toBe(false);
  });

  it('recolher no meio do arrasto zera o resizing (a alca some por {#if}, sem desmontar)', async () => {
    const t = montar(false);
    await tick();
    ctxPanel.resizing = true;   // arrasto em curso
    ctxPanel.recolhido = true;  // recolheu: a alca sai do DOM, o componente fica montado
    await tick();
    expect(ctxPanel.resizing).toBe(false);
    unmount(t.comp);
  });
});
