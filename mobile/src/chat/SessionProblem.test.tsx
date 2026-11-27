// @vitest-environment happy-dom
import { act, createElement } from 'react';
import { createRoot } from 'react-dom/client';
import { describe, expect, it } from 'vitest';
import * as m from '../paraglide/messages';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

import { autoContinueCappedLabel, SessionProblem, showsSessionProblem } from './SessionProblem';

async function texto(problem: string, detail: string | null = null) {
  const container = document.createElement('div');
  const root = createRoot(container);
  await act(async () => root.render(createElement(SessionProblem, { problem, detail })));
  const out = container.textContent;
  act(() => root.unmount());
  return out;
}

describe('SessionProblem', () => {
  it('traduz os códigos do estado do Rust', async () => {
    expect(await texto('state_facts_unavailable')).toBe(m.problema_state_facts_unavailable());
    expect(await texto('permission_observe_failed')).toBe(m.problema_permission_observe_failed());
    expect(await texto('limit_switch_no_room')).toBe(m.problema_limit_switch_no_room());
    expect(await texto('limit_switch_failed')).toBe(m.problema_limit_switch_failed());
    expect(await texto('limit_switch_no_target')).toBe(m.problema_limit_switch_no_target());
    expect(await texto('limit_switch_login_lost')).toBe(m.problema_limit_switch_login_lost());
  });

  it('troca de conta barrada pelo teto do continuar sozinho diz o teto, nunca o código cru', async () => {
    expect(await texto('limit_switch_failed', 'auto_continue_capped')).toBe(m.problema_auto_continue_capped());
    expect(await texto('limit_switch_failed', 'auto_continue_capped:hourly')).toBe(m.problema_auto_continue_capped_hourly());
    expect(await texto('limit_switch_failed', 'auto_continue_capped:once')).toBe(m.problema_auto_continue_capped_once());
    // Teto sem frase própria fica com a geral.
    expect(await texto('limit_switch_failed', 'auto_continue_capped:server_5xx')).toBe(m.problema_auto_continue_capped());
    // Outra falha da troca segue com a frase da troca.
    expect(await texto('limit_switch_failed', 'reopen_failed')).toBe(m.problema_limit_switch_failed());
    expect(m.problema_auto_continue_capped()).toBe('Continuei sozinho 2 vezes seguidas sem resposta e parei para não virar laço. Mande uma mensagem para seguir.');
    expect(autoContinueCappedLabel('auto_continue_cappedx')).toBeNull();
    expect(autoContinueCappedLabel('erro_envio_falhou')).toBeNull();
  });

  it('sessão de terminal derrubada pelo sistema tem frase própria', async () => {
    expect(await texto('terminal_out_of_memory')).toBe(m.problema_terminal_out_of_memory());
    expect(await texto('terminal_died')).toBe(m.problema_terminal_died());
  });

  it('mensagem presa sem entrega tem frase própria, não a de código desconhecido', async () => {
    const out = await texto('headless_delivery_stalled');
    expect(out).toBe(m.problema_headless_delivery_stalled());
    expect(out).not.toBe(m.session_problem_unknown());
  });

  it('com o painel "Na fila" à mostra, a entrega parada fica só nele; os outros problemas continuam aqui', () => {
    expect(showsSessionProblem('headless_delivery_stalled', true)).toBe(false);
    expect(showsSessionProblem('headless_delivery_stalled', false)).toBe(true);
    expect(showsSessionProblem('headless_turno_erro', true)).toBe(true);
    expect(showsSessionProblem(null, false)).toBe(false);
  });
});
