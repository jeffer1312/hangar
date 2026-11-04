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
import * as core from '@hangar/core';

vi.mock('@hangar/core', async (original) => ({
  ...await original<typeof import('@hangar/core')>(), leavePair: vi.fn(), setPairAccess: vi.fn(), hideSession: vi.fn(),
}));

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

describe('SessionCard: par externo', () => {
  const PAR = { alias: 'casa', owner: 'Ana', session: 'api', address: 'https://ana.ts.net:8443', level: 'view' as const, pending: true };
  async function abrirMenu(el: HTMLElement) {
    vi.useFakeTimers();
    el.querySelector<HTMLElement>('.session-row')!.dispatchEvent(new PointerEvent('pointerdown', { clientX: 1, clientY: 1, bubbles: true }));
    vi.advanceTimersByTime(600);
    vi.useRealTimers();
    await tick();
  }
  const botao = (t: string) => [...document.querySelectorAll('button')].find((b) => b.textContent?.trim() === t);

  it('sessão de fora: marca do dono, sem Fechar, e sair do pareamento pede confirmação', async () => {
    vi.mocked(core.leavePair).mockResolvedValue({ ok: true });
    const fora = { ...sessao({ name: 'api', guest_kind: 'pair' }), serverId: 'pair:x', serverInvite: true,
      outside: { alias: 'casa', owner: 'Ana', localServerId: 'srv-a', localSession: 'hangar' } } as AggSession;
    const { el, comp } = montar(fora);
    expect(el.textContent).toContain(m.pair_from_owner({ dono: 'Ana' }));
    expect(el.querySelector('.act.del')).toBeNull();
    await abrirMenu(el);
    expect(botao(m.convite_parar())).toBeUndefined();
    botao(m.pair_leave_outside({ dono: 'Ana' }))!.click();
    await tick();
    expect(document.body.textContent).toContain(m.pair_leave_outside_body({ dono: 'Ana' }));
    expect(core.leavePair).not.toHaveBeenCalled();
    botao(m.pair_leave_outside({ dono: 'Ana' }))!.click();
    await tick();
    expect(core.leavePair).toHaveBeenCalledWith('casa', 'api', expect.objectContaining({ id: 'srv-a' }));
    unmount(comp);
    document.body.innerHTML = '';
  });

  it('sessão própria com par pendente: aviso, Liberar com confirmação e Desfazer par', async () => {
    vi.mocked(core.setPairAccess).mockResolvedValue({ ok: true });
    vi.mocked(core.leavePair).mockResolvedValue({ ok: true });
    const propria = { ...sessao({ name: 'hangar', pair_externals: [PAR] }), serverId: 'srv-a' } as AggSession;
    const { el, comp } = montar(propria);
    expect(el.textContent).toContain(m.pair_full_pending());
    await abrirMenu(el);
    botao(m.pair_release({ dono: 'Ana' }))!.click();
    await tick();
    expect(document.body.textContent).toContain(m.pair_full_confirm({ sessao: 'hangar', dono: 'Ana' }));
    botao(m.pair_release({ dono: 'Ana' }))!.click();
    await tick();
    expect(core.setPairAccess).toHaveBeenCalledWith('hangar', 'casa', 'full', expect.objectContaining({ id: 'srv-a' }));
    await new Promise((r) => setTimeout(r, 0));
    await tick();
    expect(document.body.textContent).not.toContain(m.pair_full_confirm({ sessao: 'hangar', dono: 'Ana' }));
    await abrirMenu(el);
    botao(m.pair_undo_own({ dono: 'Ana' }))!.click();
    await tick();
    expect(document.body.textContent).toContain(m.pair_undo_anchor_body({ dono: 'Ana' }));
    botao(m.pair_undo_own({ dono: 'Ana' }))!.click();
    await tick();
    expect(core.leavePair).toHaveBeenCalledWith('casa', 'api', expect.objectContaining({ id: 'srv-a' }));
    unmount(comp);
    document.body.innerHTML = '';
  });

  it('dois pares do mesmo dono: o Desfazer diz qual sessão', async () => {
    const pares = [{ ...PAR, pending: false }, { ...PAR, session: 'docs', pending: false }];
    const { el, comp } = montar({ ...sessao({ name: 'hangar', pair_externals: pares }), serverId: 'srv-a' } as AggSession);
    await abrirMenu(el);
    expect(botao(m.pair_undo_own({ dono: 'Ana · api' }))).toBeTruthy();
    expect(botao(m.pair_undo_own({ dono: 'Ana · docs' }))).toBeTruthy();
    unmount(comp);
    document.body.innerHTML = '';
  });
});

describe('SessionCard: placa da máquina e da conta, anel de contexto', () => {
  const linha = (over: Partial<AggSession>) => ({
    name: 's1', state: 'idle', serverId: 'casa', serverLabel: 'Casa', serverColor: '#f59e0b', ...over,
  }) as AggSession;

  it('etiqueta da conta vira a placa no lugar do chip antigo, com o nome do servidor', () => {
    const { el, comp } = montar(linha({
      conta: 'codex:/h/.codex-work', provider: 'codex',
      accountTag: { name: 'ana', icon: 'credit-card', color: 'blue' },
    }));
    const conta = el.querySelector('.plate-account');
    expect(conta?.textContent).toContain('ana');
    expect(conta?.getAttribute('title')).toBe(m.tags_plate_account({ name: 'ana' }));
    expect(el.querySelector('.conta-chip')).toBeNull();
    // A máquina só entra quando a lista pede.
    expect(el.querySelector('.plate-machine')).toBeNull();
    unmount(comp);
  });

  it('servidor antigo (sem etiqueta da conta) mantém o chip calculado do id', () => {
    const { el, comp } = montar(linha({ conta: 'claude:/h/.claude-work', accountTag: null }));
    expect(el.querySelector('.plate')).toBeNull();
    expect(el.querySelector('.conta-chip')?.textContent).toBe('work');
    unmount(comp);
  });

  it('sem conta conhecida, sem pedaço de conta', () => {
    const { el, comp } = montar(linha({ conta: null, accountTag: null }));
    expect(el.querySelector('.plate-account')).toBeNull();
    expect(el.querySelector('.conta-chip')).toBeNull();
    unmount(comp);
  });

  it('showMachine põe a máquina na placa com o nome da etiqueta', () => {
    const { el, comp } = montar(linha({
      machineTag: { name: 'Casa', icon: 'house', color: 'amber' },
      accountTag: { name: 'claude-200-2', icon: 'credit-card', color: null },
    }), { showMachine: true });
    const segs = [...el.querySelectorAll('.plate .plate-seg')];
    expect(segs.map((s) => s.classList.contains('plate-machine'))).toEqual([true, false]);
    expect(segs[0].textContent).toContain('Casa');
    expect(segs[1].textContent).toContain('claude-200-2');
    unmount(comp);
  });

  it('anel de contexto: âmbar a partir de 70, vermelho a partir de 90, ausente sem dado', () => {
    const ctx = (used: number): Partial<AggSession> => ({ context: { used, window: 100 } });
    const warn = montar(linha(ctx(75)));
    expect(warn.el.querySelector('.lead .ring')?.classList.contains('tone-warn')).toBe(true);
    const err = montar(linha(ctx(95)));
    expect(err.el.querySelector('.lead .ring')?.classList.contains('tone-error')).toBe(true);
    const ok = montar(linha(ctx(20)));
    expect(ok.el.querySelector('.lead .ring')?.classList.contains('tone-ok')).toBe(true);
    const none = montar(linha({}));
    expect(none.el.querySelector('.lead .ring')).toBeNull();
    // Com o anel, o ponto do estado continua dentro dele.
    expect(warn.el.querySelector('.lead .ring .state-dot')).not.toBeNull();
    for (const t of [warn, err, ok, none]) unmount(t.comp);
  });
});

describe('SessionCard: ocultar sessão em grupo', () => {
  async function abrirMenu(el: HTMLElement) {
    vi.useFakeTimers();
    el.querySelector<HTMLElement>('.session-row')!.dispatchEvent(new PointerEvent('pointerdown', { clientX: 1, clientY: 1, bubbles: true }));
    vi.advanceTimersByTime(600);
    vi.useRealTimers();
    await tick();
  }
  const botao = (t: string) => [...document.querySelectorAll('button')].find((b) => b.textContent?.trim() === t);
  const linha = (serverId: string, name: string, over: Partial<AggSession> = {}) =>
    ({ name, state: 'idle', serverId, ...over }) as AggSession;
  const flush = async () => { await new Promise((r) => setTimeout(r, 0)); await tick(); };

  function preparar() {
    localStorage.setItem('cp_servers', JSON.stringify([
      { id: 'pc', label: 'PC', baseUrl: 'http://pc:8765', token: 't' },
      { id: 'nb', label: 'NB', baseUrl: 'http://nb:8765', token: 't' },
    ]));
    vi.mocked(core.hideSession).mockReset().mockResolvedValue(undefined as never);
  }
  function limpar(comp: ReturnType<typeof mount>) {
    unmount(comp);
    document.body.innerHTML = '';
    localStorage.removeItem('cp_servers');
  }

  it('em grupo pergunta antes; "grupo inteiro" oculta cada membro no servidor dele', async () => {
    preparar();
    const me = linha('pc', 'a', { pair_gid: 'g' });
    const rows = [me, linha('pc', 'b', { pair_gid: 'g' }), linha('nb', 'c', { pair_gid: 'g' }), linha('pc', 'x')];
    const { el, comp } = montar(me, { peers: () => rows });
    await abrirMenu(el);
    botao(m.session_hide_from_list())!.click();
    await tick();
    expect(document.body.textContent).toContain(m.session_hide_group_title());
    expect(core.hideSession).not.toHaveBeenCalled();
    expect(botao(m.comum_cancelar())).toBeTruthy();
    botao(m.session_hide_whole_group({ n: 3 }))!.click();
    await flush();
    const calls = vi.mocked(core.hideSession).mock.calls.map(([s, n]) => `${(s as { id: string }).id}::${n}`);
    expect(calls).toEqual(['pc::a', 'pc::b', 'nb::c']);
    limpar(comp);
  });

  it('falha num membro vira um aviso só, com os nomes que ficaram', async () => {
    preparar();
    vi.mocked(core.hideSession).mockImplementation(async (_s, name) => { if (name === 'c') throw new Error('offline'); return undefined as never; });
    const me = linha('pc', 'a', { pair_gid: 'g' });
    const rows = [me, linha('nb', 'c', { pair_gid: 'g' })];
    const { el, comp } = montar(me, { peers: () => rows });
    await abrirMenu(el);
    botao(m.session_hide_from_list())!.click();
    await tick();
    botao(m.session_hide_whole_group({ n: 2 }))!.click();
    await flush();
    const note = document.querySelector('.card-menu-note.error');
    expect(note?.textContent).toContain(m.session_hide_group_skipped({ names: 'c' }));
    expect(note?.textContent).toContain('offline');
    limpar(comp);
  });

  it('fora de grupo oculta direto, sem pergunta', async () => {
    preparar();
    const me = linha('pc', 'a');
    const { el, comp } = montar(me, { peers: () => [me, linha('pc', 'b')] });
    await abrirMenu(el);
    botao(m.session_hide_from_list())!.click();
    await flush();
    expect(document.body.textContent).not.toContain(m.session_hide_group_title());
    expect(core.hideSession).toHaveBeenCalledTimes(1);
    expect(core.hideSession).toHaveBeenCalledWith(expect.objectContaining({ id: 'pc' }), 'a');
    limpar(comp);
  });
});
