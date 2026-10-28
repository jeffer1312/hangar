import { describe, expect, it } from 'vitest';
import { quotaRefusal, withQuotaRetry, type CreateSessionBody } from './api';
import { formataErro } from './errosApi';
import * as m from './paraglide/messages';

// O erro como o `ensureOk`/`apiFetchForServer` o lança: status, code e o detail cru.
const refused = (params: Record<string, unknown>, status = 409, code = 'erro_conta_sem_cota') =>
  Object.assign(new Error('x'), { status, code, envelope: { code, msg: 'x', params } });

describe('recusa da abertura por cota', () => {
  it('com sugerida Claude, devolve a conta, o uso e o config_dir a repetir', () => {
    const r = quotaRefusal(refused({ conta: 'claude-200-1', conta_id: '/c1', uso: '97', sugerida: 'claude-200-2',
      sugerida_id: '/c2', retry: { config_dir: '/c2' } }));
    expect(r).toEqual({ conta: 'claude-200-1', uso: 97, sugerida: 'claude-200-2', retry: { config_dir: '/c2' },
      text: m.open_quota_title({ conta: 'claude-200-1', uso: '97' }) });
  });

  it('com sugerida Codex, o retry é a codex_account', () => {
    const r = quotaRefusal(refused({ conta: 'a', uso: 100, sugerida: 'b', sugerida_id: 'acc-b', retry: { codex_account: 'acc-b' } }));
    expect(r?.retry).toEqual({ codex_account: 'acc-b' });
  });

  it('sem sugerida, não há retry nem conta a oferecer', () => {
    const r = quotaRefusal(refused({ conta: 'a', conta_id: '/a', uso: '100', sugerida: '', sugerida_id: '' }));
    expect(r).toEqual({ conta: 'a', uso: 100, sugerida: '', retry: null,
      text: `${m.open_quota_title({ conta: 'a', uso: '100' })} ${m.open_quota_none()}` });
  });

  it('outro 409 ou outro status com o mesmo código não é recusa por cota', () => {
    expect(quotaRefusal(refused({}, 409, 'erro_conversa_viva'))).toBeNull();
    expect(quotaRefusal(refused({ retry: { config_dir: '/c2' } }, 400))).toBeNull();
    expect(quotaRefusal(new Error('rede'))).toBeNull();
    expect(quotaRefusal(null)).toBeNull();
  });

  it('o retry troca só a conta: nome, pasta e o resto do corpo ficam', () => {
    const body: CreateSessionBody = { name: 'proj', cwd: '/p', provider: 'claude', config_dir: '/c1', model: 'opus', prewarm: 't1' };
    const r = quotaRefusal(refused({ conta: 'a', uso: 99, sugerida: 'b', retry: { config_dir: '/c2' } }))!;
    expect(withQuotaRetry(body, r)).toEqual({ ...body, config_dir: '/c2' });
  });

  it('a frase do erro diz o motivo quando não há sugerida', () => {
    const sem = formataErro({ code: 'erro_conta_sem_cota', params: { conta: 'a', uso: '100' }, msg: 'x' });
    expect(sem).toBe(`${m.open_quota_title({ conta: 'a', uso: '100' })} ${m.open_quota_none()}`);
    const com = formataErro({ code: 'erro_conta_sem_cota', params: { conta: 'a', uso: 97, retry: { config_dir: '/c2' } }, msg: 'x' });
    expect(com).toBe(m.open_quota_title({ conta: 'a', uso: '97' }));
  });
});
