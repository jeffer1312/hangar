// @vitest-environment happy-dom
// Correção 3 (round 7): no Board/Canvas o `forced` da sidebarPin segura a sidebar recolhida; o
// botão expandir das abas não pode virar clique morto que grava a preferência por baixo — com
// override ativo ele fica desabilitado (tooltip explica) e a preferência só muda quando o
// usuário decide de verdade (sem override).
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { mount, unmount, tick } from 'svelte';
import SessionTabs from './SessionTabs.svelte';
import { sidebarPin } from '../lib/sidebarPin.svelte';
import { sidebarBridge } from '../lib/sidebarBridge';
import { ctxPanel } from '../lib/ctxPanel.svelte';
import { navMode } from '../lib/navMode.svelte';
import { overwriteGetLocale } from '../paraglide/runtime';

// Store REATIVO (fixture .svelte.ts): o $derived do SessionTabs re-computa quando o modelo muda —
// necessário pro teste do foco pós-rename esperar o reflexo do SSE. Mutar via fixtureByServer.
vi.mock('../lib/sessionsStore.svelte', async () => ({
  sessionsStore: (await import('./sessionTabs.test-store.svelte')).fixtureStore,
}));
import { fixtureByServer } from './sessionTabs.test-store.svelte';
vi.mock('../lib/format', () => ({
  stateColors: {}, rotuloEstado: () => '', sortSessions: (s: unknown[]) => s,
}));
vi.mock('../lib/plan', () => ({ planBadge: vi.fn() }));
import { planBadge } from '../lib/plan';
// A QuotaPill lê as cotas ao montar; sem isto cada teste deixava um fetch REAL escapar pro
// `localhost:3000` do happy-dom (19 ECONNREFUSED/AbortError por suíte, um por teste).
vi.mock('../lib/contaEstado', async (importOriginal) => {
  const real = await importOriginal<typeof import('../lib/contaEstado')>();
  return { ...real, listarCotas: vi.fn().mockResolvedValue([]) };
});

function montar(over: { ctxDisponivel?: boolean } = {}) {
  const el = document.createElement('div');
  document.body.appendChild(el);
  const comp = mount(SessionTabs, {
    target: el,
    props: {
      currentKey: 'srv-a::sess-1', onSelect: vi.fn(), onOpenConfig: vi.fn(), onIrParaContas: vi.fn(),
      ...(over.ctxDisponivel !== undefined ? { ctxDisponivel: over.ctxDisponivel } : {}),
    },
  });
  return { el, comp: comp as never };
}

function comSessao(nome: string) {
  fixtureByServer.length = 0;
  fixtureByServer.push({
    server: { id: 'srv-a', label: 'A' },
    sessions: [{ name: nome, serverId: 'srv-a', state: 'idle' }],
    error: null, loaded: true,
  });
}

beforeEach(() => {
  overwriteGetLocale(() => 'pt');   // rótulos/aria-labels da barra são mensagens agora
  sidebarPin.setForced(null);
  sidebarPin.setUser(false);
  vi.mocked(planBadge).mockReturnValue(null);   // sem plano por padrão (filete ausente)
  comSessao('sess-1');
  ctxPanel.recolhido = false;   // painel de contexto aberto por padrão
  // A tira de sessões passou a existir SÓ no modo abas (10/08/2026): a barra do topo virou
  // permanente e o modo decide onde as sessões moram. Estes testes são todos sobre a tira, então
  // precisam do modo em que ela existe — sem isto o componente monta a barra sem nenhuma aba.
  navMode.mode = 'tabs';
  // Teste que falha pula o unmount: o DOM vazava pro teste seguinte (abas fantasmas). Limpeza aqui.
  document.body.innerHTML = '';
});

describe('SessionTabs — a marca alterna Abas <-> Trilho', () => {
  // A marca ficou sem função quando a barra virou permanente ("expandir barra lateral" não quer
  // dizer nada no modo abas, onde a lateral está escondida de propósito), então virou o alternador
  // — pedido do usuário, 10/08/2026.
  it('clicar troca o modo nos dois sentidos e reflete em aria-pressed', async () => {
    navMode.mode = 'tabs';
    const t = montar();
    await tick();
    const marca = document.querySelector<HTMLButtonElement>('.tab-expand')!;
    expect(marca).not.toBeNull();
    expect(marca.getAttribute('aria-pressed')).toBe('true');
    marca.click();
    await tick();
    expect(navMode.mode).toBe('rail');
    unmount(t.comp);
  });

  it('sair das abas destrava a barra lateral (senão ela volta recolhida e parece sem efeito)', async () => {
    navMode.mode = 'tabs';
    sidebarPin.setUser(true);          // usuário tinha recolhido antes de ir pras abas
    const t = montar();
    await tick();
    document.querySelector<HTMLButtonElement>('.tab-expand')!.click();
    await tick();
    expect(navMode.mode).toBe('rail');
    expect(sidebarPin.preferred).toBe(false);
    unmount(t.comp);
  });
});

describe('SessionTabs — engrenagem e ponto do servidor (vieram do rodapé do trilho)', () => {
  // A engrenagem saiu do rodapé da Sidebar e passou a morar aqui (10/08/2026). O PONTO veio junto
  // de propósito: ele é a única coisa na tela que diz em qual servidor você está, na mesma cor que
  // agrupa as sessões. Sem este teste, uma limpeza futura tiraria o ponto sem ninguém notar — a
  // engrenagem continuaria funcionando e só a informação sumiria.
  it('a engrenagem existe, cita o servidor ativo e carrega o ponto colorido', async () => {
    const t = montar();
    await tick();
    const gear = document.querySelector<HTMLButtonElement>('.tab-config');
    expect(gear).not.toBeNull();
    expect(gear?.getAttribute('aria-label')).toContain('Configurações');
    expect(gear?.querySelector('.tab-srv-dot')).not.toBeNull();
    unmount(t.comp);
  });
});

describe('SessionTabs — expandir sob override do Board/Canvas (round 7)', () => {
  it('forced=true: botão desabilitado com tooltip e clique NÃO altera a preferência', async () => {
    sidebarPin.setUser(true);   // preferência do usuário: recolhida
    sidebarPin.setForced(true); // board/canvas segurando o recolhido
    const t = montar();
    await tick();
    const btn = document.querySelector<HTMLButtonElement>('.tab-expand')!;
    expect(btn.disabled).toBe(true);
    expect(btn.title).not.toBe('');   // explica o porquê do bloqueio
    btn.click();
    await tick();
    expect(sidebarPin.preferred).toBe(true);   // nada gravado por baixo
    unmount(t.comp);
  });

  it('forced=null com preferência recolhida: botão habilitado e clique expande de verdade', async () => {
    sidebarPin.setUser(true);   // recolhida por PREFERÊNCIA (não por override)
    const t = montar();
    await tick();
    const btn = document.querySelector<HTMLButtonElement>('.tab-expand')!;
    expect(btn.disabled).toBe(false);
    btn.click();
    await tick();
    expect(sidebarPin.preferred).toBe(false);   // expandiu: preferência mudou de propósito
    unmount(t.comp);
  });
});

describe('SessionTabs — filete de progresso do plano (round 2)', () => {
  function comPlano(pct: number, complete: boolean) {
    vi.mocked(planBadge).mockReturnValue({
      pct, complete, label: '', title: `Plano ${pct}%`,
    });
  }

  it('sem plano: NENHUM filete no DOM e aria-label sem menção a plano', async () => {
    const t = montar();   // beforeEach: planBadge null
    await tick();
    expect(document.querySelector('.tab-plan')).toBeNull();
    const tab = document.querySelector<HTMLButtonElement>('.tab');
    expect(tab?.getAttribute('aria-label')).not.toContain('plano');
    unmount(t.comp);
  });

  it('0%: filete presente com trilho cheio (--pct 0%) — distingue de "sem plano"', async () => {
    comPlano(0, false);
    const t = montar();
    await tick();
    const filete = document.querySelector<HTMLElement>('.tab-plan');
    expect(filete).not.toBeNull();
    expect(filete?.style.getPropertyValue('--pct')).toBe('0%');
    expect(filete?.classList.contains('done')).toBe(false);
    unmount(t.comp);
  });

  it('parcial: --pct proporcional ao progresso, sem marcação de concluído', async () => {
    comPlano(37, false);
    const t = montar();
    await tick();
    const filete = document.querySelector<HTMLElement>('.tab-plan');
    expect(filete?.style.getPropertyValue('--pct')).toBe('37%');
    expect(filete?.classList.contains('done')).toBe(false);
    unmount(t.comp);
  });

  it('concluído: --pct 100% e classe done (cor de sucesso no CSS)', async () => {
    comPlano(100, true);
    const t = montar();
    await tick();
    const filete = document.querySelector<HTMLElement>('.tab-plan');
    expect(filete?.style.getPropertyValue('--pct')).toBe('100%');
    expect(filete?.classList.contains('done')).toBe(true);
    unmount(t.comp);
  });

  it('aba ativa com plano parcial: filete presente DENTRO da aba ativa (sem sumir)', async () => {
    comPlano(37, false);
    const t = montar();   // currentKey 'srv-a::sess-1' = aba ativa
    await tick();
    const ativa = document.querySelector<HTMLButtonElement>('.tab.active')!;
    expect(ativa).not.toBeNull();
    const filete = ativa.querySelector<HTMLElement>('.tab-plan');
    expect(filete).not.toBeNull();
    expect(filete?.style.getPropertyValue('--pct')).toBe('37%');
    unmount(t.comp);
  });

  it('aria-label carrega o progresso do plano quando existe', async () => {
    comPlano(37, false);
    const t = montar();
    await tick();
    const tab = document.querySelector<HTMLButtonElement>('.tab');
    expect(tab?.getAttribute('aria-label')).toContain('plano 37%');
    unmount(t.comp);
  });
});

describe('SessionTabs — toggle do contexto na barra (follow-up visual)', () => {
  it('botão no EXTREMO DIREITO da barra (último, depois da engrenagem)', async () => {
    const t = montar();
    await tick();
    const botoes = [...document.querySelectorAll<HTMLButtonElement>('.tabs-bar button')];
    const ctx = botoes.find((b) => b.classList.contains('tab-ctx'))!;
    expect(ctx).toBeDefined();
    // Último botão da barra: o toggle de contexto fecha a fila de ações
    expect(botoes[botoes.length - 1]).toBe(ctx);
    unmount(t.comp);
  });

  it('alterna ctxPanel.recolhido nos dois sentidos e reflete aria-label', async () => {
    const t = montar();
    await tick();
    const ctx = document.querySelector<HTMLButtonElement>('.tab-ctx')!;
    expect(ctxPanel.recolhido).toBe(false);
    expect(ctx.getAttribute('aria-label')).toBe('Recolher painel de contexto');
    ctx.click();
    await tick();
    expect(ctxPanel.recolhido).toBe(true);
    expect(ctx.getAttribute('aria-label')).toBe('Expandir painel de contexto');
    ctx.click();
    await tick();
    expect(ctxPanel.recolhido).toBe(false);
    expect(ctx.getAttribute('aria-label')).toBe('Recolher painel de contexto');
    unmount(t.comp);
  });

  it('ctxDisponivel=false: desabilitado com tooltip explicando (decisão do usuário)', async () => {
    const t = montar({ ctxDisponivel: false });
    await tick();
    const ctx = document.querySelector<HTMLButtonElement>('.tab-ctx')!;
    expect(ctx.disabled).toBe(true);
    expect(ctx.title).toBe('Sem painel de contexto aberto');
    // clique é no-op (disabled) — a preferência não muda
    ctx.click();
    await tick();
    expect(ctxPanel.recolhido).toBe(false);
    unmount(t.comp);
  });

  it('tab-ctx desabilitado anuncia o motivo no aria-label', async () => {
    const t = montar({ ctxDisponivel: false });
    await tick();
    const ctx = document.querySelector<HTMLButtonElement>('.tab-ctx')!;
    expect(ctx.getAttribute('aria-label')).toBe('Sem painel de contexto aberto');
    unmount(t.comp);
  });

  it('tab-ctx desabilitado tem title igual ao aria-label', async () => {
    const t = montar({ ctxDisponivel: false });
    await tick();
    const ctx = document.querySelector<HTMLButtonElement>('.tab-ctx')!;
    expect(ctx.getAttribute('title')).toBe(ctx.getAttribute('aria-label'));
    expect(ctx.getAttribute('title')).toBe('Sem painel de contexto aberto');
    unmount(t.comp);
  });

  it('tab-ctx habilitado tem title igual ao aria-label', async () => {
    const t = montar();
    await tick();
    const ctx = document.querySelector<HTMLButtonElement>('.tab-ctx')!;
    expect(ctx.getAttribute('title')).toBe(ctx.getAttribute('aria-label'));
    expect(ctx.getAttribute('title')).toBe('Recolher painel de contexto');
    unmount(t.comp);
  });
});

describe('SessionTabs — foco pós-rename via bridge (round 2)', () => {
  const aba = () => document.querySelector<HTMLButtonElement>('.tab');

  it('focusTab com chave presente foca a aba imediatamente', async () => {
    const t = montar();
    await tick();
    sidebarBridge.focusTab('srv-a::sess-1');
    await tick();
    expect(document.activeElement).toBe(aba());
    unmount(t.comp);
  });

  it('focusTab com chave ainda ausente espera o modelo refletir e foca a aba recriada (conectada)', async () => {
    const t = montar();
    await tick();
    // O SSE ainda não refletiu o rename: a chave nova não existe no modelo
    sidebarBridge.focusTab('srv-a::sess-novo');
    await tick();
    expect(document.activeElement).not.toBe(aba());
    // Modelo reflete o novo nome: a aba antiga (keyed por nome) é substituída pela recriada
    comSessao('sess-novo');
    await tick(); await tick();
    const novaAba = aba();
    expect(novaAba?.isConnected).toBe(true);
    expect(document.activeElement).toBe(novaAba);
    unmount(t.comp);
  });
});
