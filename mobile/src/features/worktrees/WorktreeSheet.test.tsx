// @vitest-environment happy-dom
import { act, createElement, type ReactNode } from 'react';
import { createRoot } from 'react-dom/client';
import { afterEach, describe, expect, it, vi } from 'vitest';
(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const api = vi.hoisted(() => ({ getWorktreeForServer: vi.fn(), deleteWorktreeForServer: vi.fn() }));
vi.mock('react-native', async (original) => ({ ...await original<typeof import('react-native')>(),
  Pressable: (p: { accessibilityLabel?: string; onPress?: () => void; children?: unknown }) =>
    createElement('button', { 'aria-label': p.accessibilityLabel, onClick: p.onPress }, p.children as never),
  Switch: (p: { accessibilityLabel?: string; value?: boolean; onValueChange?: (v: boolean) => void }) =>
    createElement('input', { type: 'checkbox', 'aria-label': p.accessibilityLabel, checked: !!p.value, onChange: () => p.onValueChange?.(!p.value) }) }));
vi.mock('../../ui/Sheet', () => ({ Sheet: ({ children }: { children: ReactNode }) => createElement('div', null, children) }));
vi.mock('expo-clipboard', () => ({ setStringAsync: vi.fn() }));
vi.mock('../../ui/Toast', () => ({ toast: { ok: vi.fn(), erro: vi.fn() } }));
vi.mock('expo-router',() => ({ router: { push: vi.fn() } }));
vi.mock('../../stores/sessions', () => ({ useSessions: () => undefined }));
// Cada mensagem vira "chave:params", para o teste conferir qual frase saiu e com o quê.
vi.mock('../../paraglide/messages', () => Object.fromEntries([
  'comum_carregando', 'comum_cancelar', 'worktrees_erro', 'worktrees_disco_calculando',
  'worktree_estado_sumida', 'worktree_estado_em_uso', 'worktree_estado_nao_commitado', 'worktree_estado_mesclada',
  'worktree_estado_sem_branch', 'worktree_estado_andamento', 'worktree_veredito_sumida', 'worktree_veredito_em_uso',
  'worktree_veredito_nao_commitado', 'worktree_veredito_mesclada', 'worktree_veredito_sem_branch', 'worktree_veredito_andamento',
  'worktree_subagente_nome', 'worktree_sessoes_aqui', 'worktree_sessao_trabalhando', 'worktree_sessao_esperando',
  'worktree_sessao_parada', 'worktree_ir_sessao', 'worktree_ir_sessao_nome', 'worktree_detalhe_branch', 'worktree_detalhe_base',
  'worktree_detalhe_so_dela', 'worktree_sem_branch_rotulo', 'worktree_detalhe_criada', 'worktree_detalhe_ultimo_commit',
  'worktree_detalhe_conversas', 'worktree_detalhe_espaco', 'worktree_detalhe_maior', 'worktree_detalhe_commits',
  'worktree_detalhe_sem_commits', 'worktree_detalhe_nao_commitado', 'worktree_bloqueada', 'worktree_apagar_reticencias',
  'worktree_apagar_titulo', 'worktree_libera', 'worktree_apagar_perde', 'worktree_n_nao_commitados', 'worktree_ignorados',
  'worktree_fica_guardado', 'worktree_fica_commits', 'worktree_fica_conversas', 'worktree_apagar_conversas',
  'worktree_apagar_branch_juntada', 'worktree_apagar_branch_tambem', 'worktree_apagar_perder', 'worktree_apagar',
  'worktree_apagar_branch_perde', 'worktree_copiar_caminho', 'toast_copiado',
  'worktree_tamanho_falhou', 'worktree_tamanho_parcial', 'worktree_leitura_incompleta',
  'worktree_estado_residual', 'worktree_residual_warning', 'worktree_residual_entries',
].map((k) => [k, (p?: Record<string, unknown>) => (p ? `${k}:${Object.values(p).join(',')}` : k)])));
vi.mock('@hangar/core', async (original) => ({ ...await original<typeof import('@hangar/core')>(), ...api }));
import { WorktreeSheet } from './WorktreeSheet';

const st = { path: '/r/hangar-x', repo: '/r/hangar', exists: true, branch: 'x', base: 'main', main_branch: 'dev', merged: false, ahead: 3,
  dirty: 2, ignored: ['.env.local'], sessions: [] as string[], closed: 0 };

const roots: ReturnType<typeof createRoot>[] = [];
afterEach(async () => { await act(async () => { for (const root of roots.splice(0)) root.unmount(); }); vi.clearAllMocks(); });
async function montar() {
  const el = document.createElement('div');
  const root = createRoot(el);
  roots.push(root);
  await act(async () => { root.render(createElement(WorktreeSheet,
    { server: { id: 's' } as never, path: '/r/hangar-x', onClose: () => {} })); });
  return el;
}
const clicar = async (el: HTMLElement, label: string) =>
  act(async () => { (el.querySelector(`[aria-label="${label}"]`) as HTMLElement).click(); });

describe('WorktreeSheet', () => {
  it('resíduo mostra fontes e ignorados e confirma preservando a branch', async () => {
    api.getWorktreeForServer.mockResolvedValue({ ...st, residual: true, dirty: 0, ignored: [], ahead: 0,
      residual_files: ['source.rs', 'node_modules/cache'], residual_entries: 2 });
    api.deleteWorktreeForServer.mockResolvedValue({ removed: st.path, branch_deleted: false, moved: 0 });
    const el = await montar();
    await clicar(el, 'worktree_apagar_reticencias');
    expect(el.textContent).toContain('source.rs');
    expect(el.textContent).toContain('node_modules/cache');
    expect(el.querySelector('input[type=checkbox]')).toBeNull();
    await clicar(el, 'worktree_apagar');
    expect(api.deleteWorktreeForServer).toHaveBeenCalledWith(expect.anything(),
      expect.objectContaining({ confirm: true, keep_branch: true, delete_branch: false }));
  });
  it('apagar com arquivos a perder confirma e mantém a branch', async () => {
    api.getWorktreeForServer.mockResolvedValue(st);
    api.deleteWorktreeForServer.mockResolvedValue({ removed: st.path, branch_deleted: false, moved: 0 });
    const el = await montar();
    expect(el.textContent).toContain('worktree_veredito_nao_commitado:2');
    await clicar(el, 'worktree_apagar_reticencias');
    expect(el.textContent).toContain('worktree_n_nao_commitados:2');
    expect(el.textContent).toContain('.env.local');
    expect(el.textContent).toContain('worktree_apagar_conversas:dev');   // retoma na branch da principal, não na base
    await clicar(el, 'worktree_apagar_perder:3');
    expect(api.deleteWorktreeForServer).toHaveBeenCalledWith(expect.anything(),
      expect.objectContaining({ repo: '/r/hangar', path: '/r/hangar-x', confirm: true, delete_branch: false }));
  });

  it('com sessão aberta não oferece apagar', async () => {
    api.getWorktreeForServer.mockResolvedValue({ ...st, sessions: ['s1'] });
    const el = await montar();
    expect(el.querySelector('[aria-label="worktree_apagar_reticencias"]')).toBeNull();
    expect(el.textContent).toContain('worktree_bloqueada');
    expect(el.querySelector('[aria-label="worktree_ir_sessao_nome:s1"]')).not.toBeNull();
  });
});
