// @vitest-environment happy-dom
import { act, createElement, type ReactNode } from 'react';
import { createRoot } from 'react-dom/client';
import { beforeEach, describe, expect, it, vi } from 'vitest';

const calls = vi.hoisted(() => ({
  list: vi.fn(),
  credentials: vi.fn(),
  login: null as { server: unknown; accountId?: string } | null,
}));

const server = { id: 'server-b', label: 'Servidor B', baseUrl: 'https://b.local', token: 'token-b' };

vi.mock('../../stores/servers', () => ({
  useServers: (selector: (state: { active: () => typeof server; servers: typeof server[]; activeId: string }) => unknown) =>
    selector({ active: () => server, servers: [server], activeId: server.id }),
}));
vi.mock('@hangar/core', async (original) => ({
  ...await original<typeof import('@hangar/core')>(),
  getCodexAccountsForServer: calls.list,
  getCredentialsForServer: calls.credentials,
}));
vi.mock('./CodexContaLogin', () => ({
  CodexContaLogin: (props: { server: unknown; accountId?: string; onComplete: () => void }) => {
    calls.login = { server: props.server, accountId: props.accountId };
    return createElement('div', { 'data-testid': 'codex-login' }, props.accountId ?? 'new');
  },
}));
vi.mock('./Pagina', () => ({ Pagina: ({ children }: { children: ReactNode }) => createElement('main', null, children) }));
vi.mock('./Linha', () => ({
  Linha: ({ titulo, descricao, onPress, children, direita }: { titulo: string; descricao?: string; onPress?: () => void; children?: ReactNode; direita?: ReactNode }) =>
    createElement('section', null,
      createElement('button', { onClick: onPress, disabled: !onPress }, titulo),
      descricao ? createElement('span', null, descricao) : null,
      direita,
      children,
    ),
}));
vi.mock('../../paraglide/messages', () => ({
  contas_titulo: () => 'Contas',
  contas_descricao: () => 'credenciais',
  contas_atualizar: () => 'Atualizar',
  contas_nova: () => '+ Nova conta',
  contas_entrar: () => 'Entrar',
  lista_remover: () => 'Remover',
  comum_apagar: () => 'Apagar',
  comum_cancelar: () => 'Cancelar',
  codex_account_delete_failed: () => 'Não foi possível apagar a conta Codex.',
  contas_tipo_chave: () => 'Chave de API',
  contas_secao_claude: () => 'Assinaturas',
  contas_secao_modelos: () => 'Modelos',
  contas_secao_outros: () => 'Outras chaves',
  comum_carregando: () => 'Carregando…',
  maquinas_titulo: () => 'Máquinas',
  maquinas_vazio: () => 'Nenhuma máquina',
  contas_email_plano: ({ email, max }: { email: string; max: string }) => `${email} · ${max}`,
  contas_nao_conectada: () => 'Não está conectada',
  codex_ui_unknown: () => 'Autenticação indisponível',
  codex_ui_account: () => 'Conta do Codex',
  novacred_codex_nome: () => 'Conta do ChatGPT (Codex)',
  login_fechar: () => 'Fechar',
  erro_desconhecido: () => 'Erro',
}));
vi.mock('react-native-unistyles', () => ({ StyleSheet: { create: (fn: any) => typeof fn === 'function' ? fn({ tokens: { bg: { surface: '#eee' }, text: { primary: '#000', secondary: '#666', muted: '#999' }, accent: { base: '#00f' }, status: { error: '#f00', success: '#0a0' } }, base: { space: [0, 4, 8, 12, 16], text: { xs: 12, sm: 14, base: 16, lg: 18, xl: 20 }, radius: { md: 8, lg: 12 }, fontMono: 'monospace' } }) : fn } }));

import Contas from '../../../app/config/contas';

describe('contas Codex', () => {
  beforeEach(() => {
    calls.list.mockReset();
    calls.credentials.mockReset();
    calls.login = null;
    calls.list.mockResolvedValue([
      { id: 'default', credential_id: 'codex:/default', name: 'Padrão', home: '/default', is_default: true, auth: { method: 'oauth', status: 'connected', email: 'a@example.com', plan: 'Plus' }, sync: { status: 'ready', trust_pending: false, issues: [] } },
      { id: 'work', credential_id: 'codex:/work', name: 'Trabalho', home: '/work', is_default: false, auth: { method: 'oauth', status: 'disconnected', email: null, plan: null }, sync: { status: 'ready', trust_pending: false, issues: [] } },
    ]);
    calls.credentials.mockResolvedValue([
      { id: 'claude:/main', tipo: 'claude', auth_method: 'oauth', nome: 'Claude', nome_natural: 'Claude', ativa: true, usos: [], login: { estado: 'ok', loggedIn: true, email: 'claude@example.com', plano: 'Max' } },
      { id: 'codex:/work', tipo: 'codex', auth_method: 'oauth', codex_account: 'work', nome: 'Codex OAuth', nome_natural: 'Codex OAuth', ativa: true, usos: [] },
      { id: 'key:other', tipo: 'chave', auth_method: 'api_key', nome: 'API key', nome_natural: 'API key', ativa: true, usos: [] },
      { id: 'unknown', tipo: 'chave', auth_method: 'unknown', nome: 'Unknown', nome_natural: 'Unknown', ativa: true, usos: [] },
    ]);
  });

  it('lê e autentica no servidor ativo, mantendo a conta escolhida', async () => {
    const container = document.createElement('div');
    const root = createRoot(container);
    await act(async () => root.render(createElement(Contas)));
    await act(async () => Promise.resolve());

    expect(calls.list).toHaveBeenCalledWith(server, expect.any(AbortSignal));
    expect(calls.credentials).toHaveBeenCalledWith(server, false, expect.any(AbortSignal));
    expect(container.textContent).toContain('Servidor B');
    expect((container.textContent?.match(/Trabalho/g) ?? []).length).toBe(1);
    expect(container.textContent).not.toContain('Codex OAuth');
    expect(container.textContent).toContain('Assinaturas');
    expect(container.textContent).toContain('Claude');
    expect(container.textContent).toContain('API key');
    expect(container.textContent).toContain('Unknown');
    expect((container.textContent?.match(/API key/g) ?? []).length).toBe(1);
    expect((container.textContent?.match(/Unknown/g) ?? []).length).toBe(1);
    const work = [...container.querySelectorAll('button')].find((button) => button.textContent === 'Entrar');
    expect(work).toBeTruthy();
    await act(async () => work!.click());
    expect(calls.login).toEqual({ server, accountId: 'work' });

    const novo = [...container.querySelectorAll('button')].find((button) => button.textContent === '+ Nova conta');
    await act(async () => novo!.click());
    expect(calls.login).toEqual({ server, accountId: undefined });
    root.unmount();
  });
});
