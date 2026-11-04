// @vitest-environment happy-dom
import { act, createElement, type ReactNode } from 'react';
import { createRoot } from 'react-dom/client';
import { beforeEach, describe, expect, it, vi } from 'vitest';

const calls = vi.hoisted(() => ({
  list: vi.fn(),
  credentials: vi.fn(),
  engines: vi.fn(),
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
  getEnginesForServer: calls.engines,
}));
const stored = vi.hoisted(() => new Map<string, string>());
vi.mock('../../stores/prefs', () => ({ prefs: {
  getString: (key: string) => stored.get(key), set: (key: string, value: string) => { stored.set(key, value); }, remove: (key: string) => { stored.delete(key); },
} }));
vi.mock('expo-linking', () => ({ openURL: vi.fn() }));
vi.mock('expo-clipboard', () => ({ setStringAsync: vi.fn() }));
// A folha da etiqueta puxa o teclado e a área segura nativos, que não carregam no teste.
vi.mock('../machines/parts', () => ({ MachineSheet: () => null, Notice: () => null }));
vi.mock('../../ui/Sheet', () => ({
  Sheet: ({ children, open }: { children: ReactNode; open?: boolean }) => open === false ? null : createElement('div', null, children),
}));
vi.mock('./CodexContaLogin', () => ({
  CodexContaLogin: (props: { server: unknown; accountId?: string; onComplete: () => void }) => {
    calls.login = { server: props.server, accountId: props.accountId };
    return createElement('div', { 'data-testid': 'codex-login' }, props.accountId ?? 'new');
  },
}));
vi.mock('./Pagina', () => ({ Pagina: ({ children }: { children: ReactNode }) => createElement('main', null, children) }));
vi.mock('./SettingsRow', () => ({
  SettingsRow: ({ title, description, children }: { title: string; description?: string; children?: ReactNode }) =>
    createElement('section', null, createElement('strong', null, title), description ? createElement('span', null, description) : null, children),
}));
vi.mock('./SectionCard', () => ({
  SectionCard: ({ title, subtitle, extra, children }: { title?: string; subtitle?: string; extra?: ReactNode; children?: ReactNode }) =>
    createElement('article', null, title, subtitle ? createElement('small', null, subtitle) : null, extra, children),
}));
vi.mock('./InfoNotice', () => ({ InfoNotice: ({ text }: { text: string }) => createElement('p', null, text) }));
vi.mock('./PageHeader', () => {
  const Pill = ({ label, onPress }: { label: string; onPress: () => void }) => createElement('button', { onClick: onPress }, label);
  return {
    Pill,
    PageHeader: ({ title, actions }: { title: string; actions?: Array<{ label: string; onPress: () => void }> }) =>
      createElement('header', null, title, ...(actions ?? []).map((a) => createElement(Pill, { key: a.label, ...a }))),
  };
});
vi.mock('../../paraglide/messages', async original => ({
  ...await original<typeof import('../../paraglide/messages')>(),
  contas_modelos_titulo: () => 'Contas e modelos',
  contas_descricao: () => 'credenciais',
  contas_secao_claude_leg: () => 'leg assinaturas',
  contas_secao_modelos_leg: () => 'leg modelos',
  contas_secao_outros_leg: () => 'leg outras',
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

import { motivoLoginIndisponivel, type Credencial } from '@hangar/core';
import Contas from '../../../app/config/contas';
import * as m from '../../paraglide/messages';

describe('contas Codex', () => {
  beforeEach(() => {
    calls.list.mockReset();
    calls.credentials.mockReset();
    calls.engines.mockReset();
    calls.engines.mockResolvedValue({ motores: {}, arquivo_corrompido: false, arquivo_caminho: '' });
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
    // Lista única: os títulos de seção saíram.
    expect(container.textContent).not.toContain('Assinaturas');
    expect(container.textContent).not.toContain('Outras chaves');
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

  it('diz por que o login não foi conferido no lugar do indisponível genérico', async () => {
    const claude = { id: 'claude:/main', tipo: 'claude', auth_method: 'oauth', nome: 'Claude', nome_natural: 'Claude', ativa: true, usos: [],
      login: { estado: 'indisponivel', loggedIn: null, email: null, plano: null, motivo: 'cli-indisponivel' } } as Credencial;
    const codexConta = { id: 'codex:/work', tipo: 'codex', auth_method: 'oauth', codex_account: 'work', nome: 'Codex OAuth', nome_natural: 'Codex OAuth', ativa: false, usos: [],
      login: { estado: 'indisponivel', loggedIn: null, email: null, plano: null, motivo: 'prazo' } } as Credencial;
    const codexSolta = { id: 'codex:/solta', tipo: 'codex', auth_method: 'unknown', codex_account: 'solta', nome: 'Solta', nome_natural: 'Solta', ativa: false, usos: [],
      login: { estado: 'indisponivel', loggedIn: null, email: null, plano: null, motivo: 'leitura-falhou' } } as Credencial;
    calls.list.mockResolvedValue([
      { id: 'work', credential_id: 'codex:/work', name: 'Trabalho', home: '/work', is_default: true, auth: { method: 'oauth', status: 'unavailable', email: null, plan: null }, sync: { status: 'ready', trust_pending: false, issues: [] } },
    ]);
    calls.credentials.mockResolvedValue([claude, codexConta, codexSolta]);
    const container = document.createElement('div');
    const root = createRoot(container);
    await act(async () => root.render(createElement(Contas)));
    await act(async () => Promise.resolve());

    // Título, email e descrição ficam no mesmo bloco de textos; a descrição é o último.
    const description = (title: string) => [...container.querySelectorAll('span')]
      .find((node) => node.textContent === title && !node.closest('[role="radiogroup"]'))?.parentElement?.parentElement?.lastElementChild?.textContent;
    for (const row of [claude, codexConta, codexSolta]) expect(motivoLoginIndisponivel(row)).toBeTruthy();
    expect(description('Claude')).toContain(motivoLoginIndisponivel(claude));
    expect(description('Trabalho')).toBe(motivoLoginIndisponivel(codexConta));
    expect(description('Solta')).toBe(motivoLoginIndisponivel(codexSolta));
    root.unmount();
  });

  it('filtra por agente, busca por email e guarda a ordem escolhida', async () => {
    stored.clear();
    calls.credentials.mockResolvedValue([
      { id: 'claude:/a', tipo: 'claude', auth_method: 'oauth', nome: 'Bravo', nome_natural: 'Bravo', ativa: false, usos: [],
        login: { estado: 'ok', loggedIn: true, email: 'bravo@example.com', plano: 'max' },
        cota: { estado: 'lida', janelas: [{ rotulo: '5h', pct: 10 }] } },
      { id: 'claude:/b', tipo: 'claude', auth_method: 'oauth', nome: 'Alfa', nome_natural: 'Alfa', ativa: true, usos: [],
        login: { estado: 'ok', loggedIn: true, email: 'alfa@example.com', plano: 'pro' },
        cota: { estado: 'lida', janelas: [{ rotulo: '5h', pct: 90 }] } },
    ]);
    calls.list.mockResolvedValue([]);
    const container = document.createElement('div');
    const root = createRoot(container);
    await act(async () => root.render(createElement(Contas)));
    await act(async () => Promise.resolve());
    const names = () => [...container.querySelectorAll('span')].map((s) => s.textContent).filter((t) => t === 'Alfa' || t === 'Bravo');
    const press = async (label: string) => {
      const target = [...container.querySelectorAll('button')].find((b) => b.getAttribute('aria-label') === label);
      expect(target, label).toBeTruthy();
      await act(async () => target!.click());
    };

    // Sem ordem escolhida, a que está em uso vem primeiro.
    expect(names()).toEqual(['Alfa', 'Bravo']);
    await press(`${m.accounts_list_filter_codex()} 0`);
    expect(names()).toEqual([]);
    expect(container.textContent).toContain(m.accounts_list_empty());
    await press(`${m.accounts_list_filter_all()} 2`);

    const nameAsc = m.accounts_list_sort_state({ column: m.accounts_list_sort_name(), direction: m.accounts_list_sort_asc() });
    await press(`${m.accounts_list_sort()}: ${m.accounts_list_sort_default()}`);
    await press(m.accounts_list_sort_name());
    expect(names()).toEqual(['Alfa', 'Bravo']);
    expect(JSON.parse(stored.get('accounts_sort')!)).toEqual({ key: 'name', dir: 1 });
    await press(`${m.accounts_list_sort()}: ${nameAsc}`);
    await press(nameAsc);
    expect(names()).toEqual(['Bravo', 'Alfa']);
    expect(JSON.parse(stored.get('accounts_sort')!)).toEqual({ key: 'name', dir: -1 });
    root.unmount();

    // A ordem volta ao abrir a tela de novo.
    const again = document.createElement('div');
    const root2 = createRoot(again);
    await act(async () => root2.render(createElement(Contas)));
    await act(async () => Promise.resolve());
    expect([...again.querySelectorAll('span')].map((s) => s.textContent).filter((t) => t === 'Alfa' || t === 'Bravo')).toEqual(['Bravo', 'Alfa']);
    root2.unmount();
  });
});
