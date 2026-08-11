// @vitest-environment happy-dom
// Kimi "sem id" (pre-1o-prompt): a sessao Kimi so cria id + wire.jsonl no PRIMEIRO envio — bloquear
// o clique como no Claude manual fechava um ciclo sem saida (sem chat -> sem 1o prompt -> sem id).
// O card abre o chat pra kimi untracked, mas segue barrando claude untracked e fora do broadcast.
import { describe, it, expect, vi } from 'vitest';
import { mount, unmount, tick } from 'svelte';
import SessionCard from './SessionCard.svelte';
import type { SessionInfo } from '../lib/types';

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
