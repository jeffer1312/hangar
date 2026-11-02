import { expect, it } from 'vitest';
import type { AggSession, CotaContaResumo } from '@hangar/core';
import { accountQuotaLine, recentSessions } from './drawerMenu';

const row = (name: string, state: AggSession['state'], last_activity: number | null) =>
  ({ name, state, last_activity, serverId: 'casa' }) as AggSession;

it('recentes: as mais novas primeiro, sem as encerradas e no limite pedido', () => {
  const rows = [row('velha', 'idle', 10), row('morta', 'dead', 99), row('nova', 'working', 50), row('sem-data', 'idle', null), row('meio', 'awaiting_input', 30)];
  expect(recentSessions(rows, 3).map((r) => r.name)).toEqual(['nova', 'meio', 'velha']);
});

it('recentes: sem as ocultas nem a de fora presa a uma oculta', () => {
  const rows = [row('visivel', 'idle', 10), { ...row('oculta', 'idle', 30), hidden_from_list: true },
    { ...row('convidada', 'idle', 20), serverId: 'fora', outside: { localServerId: 'casa', localSession: 'oculta' } } as AggSession];
  expect(recentSessions(rows, 5).map((r) => r.name)).toEqual(['visivel']);
});

it('cota: conta ativa com as janelas lidas', () => {
  const configs = [{ path: '/a', label: 'Outra', active: false }, { path: '/b', label: 'Felizardo e Batista', active: true }];
  const cotas = [{ id: 'claude:/b', estado: 'lida', janelas: [{ rotulo: '5h', pct: 27.4 }, { rotulo: '7d', pct: 10 }] }] as CotaContaResumo[];
  expect(accountQuotaLine(configs, cotas)).toBe('Felizardo e Batista · 5h 27% · 7d 10%');
});

it('cota: sem leitura mostra só a conta; sem conta ativa, nada', () => {
  const configs = [{ path: '/b', label: 'Felizardo e Batista', active: true }];
  expect(accountQuotaLine(configs, [])).toBe('Felizardo e Batista');
  expect(accountQuotaLine([{ path: '/b', label: 'X', active: false }], [])).toBe('');
});
