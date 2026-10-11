// @vitest-environment happy-dom
// Kimi "sem id" (pre-1o-prompt): a sessao Kimi so cria id + wire.jsonl no PRIMEIRO envio — bloquear
// o clique como no Claude manual fechava um ciclo sem saida (sem chat -> sem 1o prompt -> sem id).
// O card abre o chat pra kimi untracked, mas segue barrando claude untracked e fora do broadcast.
import { describe, it, expect, vi } from 'vitest';
import { mount, unmount, tick } from 'svelte';
import SessionCard from './SessionCard.svelte';
import type { AggSession, SessionInfo } from '@hangar/core';
import { fecharNav, marcarNavAberto } from '../lib/navegadorPanel.svelte';
import * as m from '../paraglide/messages';

function sessao(over: Partial<SessionInfo>): SessionInfo {
  return { name: 's1', state: 'idle', ...over } as SessionInfo;
}

function montar(session: SessionInfo, extra: Record<string, unknown> = {}) {
  const el = document.createElement('div');
  document.body.appendChild(el);
  const onClick = vi.fn();
  const onToggleSelect = vi.fn();
  const comp = mount(SessionCard, {
    target: el,
    props: { session, onClick, onDelete: vi.fn(), onToggleSelect, ...extra },
  });
  return { el, comp, onClick, onToggleSelect };
}

async function clicaNaRow(el: HTMLElement) {
  el.querySelector<HTMLElement>('.session-row')!.click();
  await tick();
}

describe('SessionCard: clique em sessão "sem id"', () => {
  it.each([0, 2])('perguntas pendentes (%i) preservam a sessão trabalhando e o clique', async (pending_questions) => {
    const { el, comp, onClick } = montar(sessao({
      provider: 'codex', state: 'working', pending_questions,
      question: 'Qual servidor?', label: 'Continuando o trabalho',
    }));
    const badge = el.querySelector('.pending-questions');
    expect(badge?.textContent ?? null).toBe(pending_questions ? '? 2' : null);
    expect(el.querySelector('.work-mark')).not.toBeNull();
    expect(el.querySelector('.status-sub')?.textContent).toBe(pending_questions ? 'Qual servidor?' : 'Continuando o trabalho');
    await clicaNaRow(el);
    expect(onClick).toHaveBeenCalledTimes(1);
    unmount(comp);
  });

  it('kimi sem id ABRE o chat (é o pré-1º-prompt por design, não um erro)', async () => {
    const { el, comp, onClick } = montar(sessao({ tracked: false, provider: 'kimi' }));
    await clicaNaRow(el);
    expect(onClick).toHaveBeenCalledTimes(1);
    // O aviso visual continua: badge "⚠ sem id" + hint no lugar do botão Retomar.
    expect(el.querySelector('.untracked-badge')).not.toBeNull();
    expect(el.querySelector('.untracked-hint')).not.toBeNull();
    unmount(comp);
  });

  it('claude sem id segue bloqueado (o transcript seria um chute de mtime)', async () => {
    const { el, comp, onClick } = montar(sessao({ tracked: false, provider: 'claude' }));
    await clicaNaRow(el);
    expect(onClick).not.toHaveBeenCalled();
    unmount(comp);
  });

  it('pi sem id segue bloqueado (o ticket dele vem da extensão, não do chat)', async () => {
    const { el, comp, onClick } = montar(sessao({ tracked: false, provider: 'pi' }));
    await clicaNaRow(el);
    expect(onClick).not.toHaveBeenCalled();
    unmount(comp);
  });

  it('kimi sem id NÃO entra no broadcast (selectMode continua bloqueado)', async () => {
    const { el, comp, onClick, onToggleSelect } = montar(
      sessao({ tracked: false, provider: 'kimi' }), { selectMode: true });
    await clicaNaRow(el);
    expect(onToggleSelect).not.toHaveBeenCalled();
    expect(onClick).not.toHaveBeenCalled();
    unmount(comp);
  });

  it('sessão rastreada abre normalmente (caminho feliz intacto)', async () => {
    const { el, comp, onClick } = montar(sessao({ tracked: true, provider: 'kimi' }));
    await clicaNaRow(el);
    expect(onClick).toHaveBeenCalledTimes(1);
    unmount(comp);
  });
});

describe('SessionCard: diff stats e tempo (referência super.engineering)', () => {
  it('mostra "+a −d" ao lado da branch quando o backend manda os números', () => {
    const { el, comp } = montar(sessao({ branch: 'PM-1', git_added: 128, git_removed: 24 }));
    const stats = el.querySelector('.diff-stats');
    expect(stats?.textContent).toContain('+128');
    expect(stats?.textContent).toContain('−24');
    unmount(comp);
  });

  it('sem repo (campos null) a meta-line não ganha badge nenhum', () => {
    const { el, comp } = montar(sessao({ branch: 'PM-1', git_added: null, git_removed: null }));
    expect(el.querySelector('.diff-stats')).toBeNull();
    unmount(comp);
  });

  it('tempo da última resposta fica na linha do nome, só com a sessão parada', () => {
    const at = Date.now() / 1000 - 45 * 60;
    const { el, comp } = montar(sessao({ last_activity: at, last_reply_at: at }));
    expect(el.querySelector('.name-row .ago')?.textContent).toContain('45');
    const { el: el2, comp: comp2 } = montar(sessao({ state: 'working', last_reply_at: at }));
    expect(el2.querySelector('.ago')).toBeNull();
    unmount(comp);
    unmount(comp2);
  });

  it('sessão parada mostra a última resposta e o tempo na linha secundária', () => {
    const { el, comp } = montar(sessao({
      last_reply: 'Os testes focados passaram.',
      last_reply_at: Date.now() / 1000 - 8 * 60,
    }));
    expect(el.querySelector('.status-sub.reply')?.textContent).toContain('Os testes focados passaram.');
    expect(el.querySelector('.ago')?.textContent).toContain('8');
    unmount(comp);
  });

  it('navegador aberto e sem terminal aparecem junto ao nome', () => {
    marcarNavAberto('srv-a::s1');
    const session = { ...sessao({ headless: true }), serverId: 'srv-a' } as AggSession;
    const { el, comp } = montar(session);
    expect(el.querySelector('.session-signal--browser')).not.toBeNull();
    expect(el.querySelector('.session-signal--headless')).not.toBeNull();
    fecharNav('srv-a::s1');
    unmount(comp);
  });

  it('mostra o nome do harness antes das demais informações', () => {
    const { el, comp } = montar(sessao({ provider: 'kimi' }));
    expect(el.querySelector('.meta-line .prov-chip')?.textContent).toContain('Kimi');
    expect(el.querySelector('.meta-line .prov-chip .pg svg')).not.toBeNull();
    const { el: el2, comp: comp2 } = montar(sessao({ provider: 'claude' }));
    expect(el2.querySelector('.meta-line .prov-chip')?.textContent).toContain('Claude');
    unmount(comp);
    unmount(comp2);
  });
});

describe('SessionCard: orquestrador sem LLM', () => {
  it('mostra o selo e não oferece Excluir (nem na trilha, nem pelo teclado)', () => {
    const { el, comp } = montar(sessao({ provider: 'orq', orq_arbiter: 'arb' }));
    expect(el.querySelector('.orq-badge')?.textContent?.trim()).toBe(m.orq_row_badge());
    expect(el.querySelector('.del')).toBeNull();
    unmount(comp);
  });

  it('sem glifo de provider mesmo quando a lista mistura agentes', () => {
    const { el, comp } = montar(sessao({ provider: 'orq', orq_arbiter: 'arb' }));
    expect(el.querySelector('.prov-chip')).toBeNull();
    unmount(comp);
  });

  it('sessão comum continua com Excluir e sem selo', () => {
    const { el, comp } = montar(sessao({ provider: 'claude' }));
    expect(el.querySelector('.orq-badge')).toBeNull();
    expect(el.querySelector('.del')).not.toBeNull();
    unmount(comp);
  });
});

describe('SessionCard: chip da worktree', () => {
  const wt = (serverId: string) =>
    ({ ...sessao({ cwd: '/r/app-x', worktree: true, worktree_path: '/r/app-x' }), serverId }) as AggSession;

  it('servidor próprio vira botão; convite fica só texto (o backend recusa worktrees ao convidado)', () => {
    localStorage.setItem('cp_servers', JSON.stringify([
      { id: 'meu', label: 'PC', baseUrl: 'http://pc:8765', token: 't' },
      { id: 'conv', label: 'Fora', baseUrl: 'https://x.ts.net:8443', token: 'g', invite: true },
    ]));
    const proprio = montar(wt('meu'));
    expect(proprio.el.querySelector('.cwd--botao')).not.toBeNull();
    unmount(proprio.comp);
    const convite = montar(wt('conv'));
    expect(convite.el.querySelector('.cwd--botao')).toBeNull();
    expect(convite.el.querySelector('.cwd')).not.toBeNull();
    unmount(convite.comp);
    localStorage.removeItem('cp_servers');
  });
});

describe('SessionCard: linha de meta do nativo', () => {
  it('branch principal não aparece; a de trabalho sim', () => {
    const { el, comp } = montar(sessao({ branch: 'main' }));
    expect(el.querySelector('.branch')).toBeNull();
    const { el: el2, comp: comp2 } = montar(sessao({ branch: 'PM-1' }));
    expect(el2.querySelector('.branch')?.textContent).toContain('PM-1');
    unmount(comp);
    unmount(comp2);
  });

  it('sessão de motor mostra o motor no lugar do agente', () => {
    const { el, comp } = montar(sessao({ provider: 'claude', engine: 'kimi-k3' }));
    expect(el.querySelector('.meta-line .prov-chip')?.textContent).toContain('kimi-k3');
    expect(el.querySelector('.meta-line .prov-chip')?.textContent).not.toContain('Claude');
    unmount(comp);
  });

  it('rótulo de trabalho corta no " ("', () => {
    const { el, comp } = montar(sessao({ state: 'working', label: 'Pensando (12s · esc para interromper)' }));
    expect(el.querySelector('.status-sub.working')?.textContent).toBe('Pensando');
    unmount(comp);
  });
});
