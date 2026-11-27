import { expect, test } from 'vitest';
import * as m from '../paraglide/messages';
import { sessionAccountLabel } from './sessionAccountLabel';

test.each([
  { kind: 'Claude nomeado', session: { conta: 'claude:/home/user/.claude-claude-200-6' }, expected: '200-6' },
  { kind: 'Claude padrão', session: { conta: 'claude:/home/user/.claude' }, expected: m.conta_padrao() },
  { kind: 'Claude no Windows com barras finais', session: { conta: 'claude:C:\\Users\\user\\.claude-claude-200-6\\\\' }, expected: '200-6' },
  { kind: 'Codex preserva claude no nome', session: { conta: 'codex:/home/user/.codex-claude-200-2' }, expected: 'claude-200-2' },
  { kind: 'Codex padrão com barras finais', session: { conta: 'codex:/home/user/.codex///' }, expected: m.conta_padrao() },
  { kind: 'Codex remove seu prefixo repetido', session: { conta: 'codex:/home/user/.codex-codex-foo' }, expected: 'foo' },
  { kind: 'Codex no Windows com barra final', session: { conta: 'codex:C:\\Users\\user\\.codex-claude-200-2\\' }, expected: 'claude-200-2' },
  { kind: 'ID Claude vence config_dir', session: { provider: 'claude', conta: 'claude:/home/user/.claude-claude-200-6', config_dir: '/home/user/.claude' }, expected: '200-6' },
  { kind: 'ID Codex vence os campos de reserva', session: { provider: 'codex', conta: 'codex:/home/user/.codex-codex-foo', codex_home: '/home/user/.codex', codex_account: 'other' }, expected: 'foo' },
  { kind: 'Claude usa config_dir sem ID', session: { provider: 'claude', config_dir: '/home/user/.claude-claude-200-6/' }, expected: '200-6' },
  { kind: 'Claude usa config_dir padrão no Windows', session: { provider: 'claude', config_dir: 'C:\\Users\\user\\.claude\\' }, expected: m.conta_padrao() },
  { kind: 'Codex usa codex_home antes de codex_account', session: { provider: 'codex', codex_home: '/home/user/.codex-claude-200-2', codex_account: 'other' }, expected: 'claude-200-2' },
  { kind: 'Codex usa codex_home padrão', session: { provider: 'codex', codex_home: '/home/user/.codex' }, expected: m.conta_padrao() },
  { kind: 'Codex usa nome de codex_account', session: { provider: 'codex', codex_account: 'claude-200-2' }, expected: 'claude-200-2' },
  { kind: 'Codex remove prefixo repetido de codex_account', session: { provider: 'codex', codex_account: 'codex-foo' }, expected: 'foo' },
  { kind: 'motor usa ID Codex antes de engine_account e config_dir', session: { provider: 'claude', engine: 'gpt', conta: 'codex:/home/user/.codex-claude-200-2', engine_account: 'other', config_dir: '/home/user/.claude' }, expected: 'gpt · claude-200-2' },
  { kind: 'motor usa ID Claude resolvido', session: { provider: 'claude', engine: 'custom', conta: 'claude:/home/user/.claude-claude-200-6', engine_account: 'other' }, expected: 'custom · 200-6' },
  { kind: 'motor usa engine_account com conta chave', session: { provider: 'claude', engine: 'gpt', conta: 'chave:gpt', engine_account: 'claude-200-2', config_dir: '/home/user/.claude' }, expected: 'gpt · claude-200-2' },
  { kind: 'motor usa engine_account sem conta resolvida', session: { provider: 'claude', engine: 'gpt', engine_account: 'claude-200-2' }, expected: 'gpt · claude-200-2' },
  { kind: 'motor sem conta não finge assinatura Anthropic', session: { provider: 'claude', engine: 'foo', conta: 'chave:foo', config_dir: '/home/user/.claude' }, expected: 'foo' },
  { kind: 'motor sem conta nem ID mostra só motor', session: { provider: 'claude', engine: 'foo', config_dir: '/home/user/.claude' }, expected: 'foo' },
  { kind: 'conta chave identifica motor mesmo sem campo engine', session: { provider: 'claude', conta: 'chave:foo', config_dir: '/home/user/.claude' }, expected: 'foo' },
  { kind: 'Antigravity padrão', session: { provider: 'antigravity', conta: 'antigravity:/home/user/.gemini' }, expected: m.mobile_antigravity_default_account() },
  { kind: 'Antigravity adicional usa o nome da pasta', session: { provider: 'antigravity', conta: 'antigravity:/home/user/.antigravity-trabalho/' }, expected: 'trabalho' },
  { kind: 'sem campos de identidade', session: {}, expected: null },
  { kind: 'Claude sem identidade', session: { provider: 'claude', conta: null, config_dir: null }, expected: null },
  { kind: 'Codex sem identidade', session: { provider: 'codex', conta: null, codex_home: null, codex_account: null }, expected: null },
  { kind: 'campos vazios não viram conta padrão', session: { provider: 'claude', conta: 'claude:', config_dir: '' }, expected: null },
  { kind: 'caminho sem nome não vira conta padrão', session: { provider: 'codex', conta: 'codex:////' }, expected: null },
  { kind: 'outro provider não usa config_dir nem codex_home', session: { provider: 'pi', config_dir: '/home/user/.claude', codex_home: '/home/user/.codex' }, expected: null },
  { kind: 'identidade não reconhecida não vira conta', session: { provider: 'kimi', conta: 'kimi:apikey' }, expected: null },
  { kind: 'engine_account sem motor não vira conta Claude', session: { provider: 'claude', engine_account: 'claude-200-2' }, expected: null },
] satisfies { kind: string; session: Parameters<typeof sessionAccountLabel>[0]; expected: string | null }[])(
  'sessionAccountLabel: $kind', ({ session, expected }) => {
    expect(sessionAccountLabel(session)).toBe(expected);
  },
);
