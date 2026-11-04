// @vitest-environment happy-dom
import { act, createElement } from 'react';
import { createRoot } from 'react-dom/client';
import { describe, expect, it, vi } from 'vitest';
(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
const theme = vi.hoisted(() => ({ tokens: { text: { muted: '#123456' }, accent: { base: '#abcdef' } } }));
vi.mock('react-native-unistyles', async (original) => ({ ...await original<typeof import('react-native-unistyles')>(),
  useUnistyles: () => ({ theme }) }));
vi.mock('react-native', async (original) => ({ ...await original<typeof import('react-native')>(),
  Pressable: (p: { accessibilityLabel?: string; onPress?: () => void; disabled?: boolean; children?: unknown }) =>
    createElement('button', { 'aria-label': p.accessibilityLabel, 'data-disabled': p.disabled ? 'true' : undefined, onClick: p.onPress }, p.children as never),
  TextInput: (p: { accessibilityLabel?: string; onChangeText?: (t: string) => void; placeholderTextColor?: string }) =>
    createElement('input', { 'aria-label': p.accessibilityLabel, 'data-placeholder-color': p.placeholderTextColor, onChange: (e: { target: { value: string } }) => p.onChangeText?.(e.target.value) }) }));
vi.mock('../../ui/Icon', () => ({ Icon: () => null }));
vi.mock('../../paraglide/messages', () => ({
  native_create_checkout_current: () => 'atual', native_create_checkout_loading: () => 'lendo',
  native_create_checkout_failed: ({ reason }: { reason: string }) => `falha:${reason}`, native_retry: () => 'de-novo', comum_voltar: () => 'voltar',
  worktree_menu_titulo: () => 'titulo', worktree_menu_pasta: () => 'pasta', worktree_menu_pasta_dica: ({ pasta, branch }: { pasta: string; branch: string }) => `${pasta}@${branch}`,
  worktree_menu_nova: () => 'nova', worktree_menu_nova_dica: () => 'nova-dica', worktree_menu_parte_de: () => 'parte',
  worktree_menu_branch: () => 'branch', worktree_menu_pasta_destino: () => 'destino', worktree_menu_nome_sessao: () => 'nome-sessao',
  worktree_menu_ja_existe: ({ branch }: { branch: string }) => `existe:${branch}`, worktree_nova_so_remoto: () => 'remoto',
  worktree_menu_ja_aberta: ({ branch, pasta }: { branch: string; pasta: string }) => `aberta:${branch}@${pasta}`,
  worktree_menu_existente: () => 'existente', worktree_menu_nenhuma: () => 'nenhuma', worktree_menu_uma: () => 'uma',
  worktree_menu_n: ({ n }: { n: number }) => `${n} wts`, worktree_menu_n_em_uso: ({ n }: { n: number }) => `${n} em-uso`,
  worktree_menu_existentes: () => 'existentes', worktree_menu_ocupada: ({ nome }: { nome: string }) => `ocupada:${nome}`,
  worktree_menu_em_uso: ({ nome }: { nome: string }) => `usa:${nome}`, worktree_parada_dias: ({ n }: { n: number }) => `parada:${n}`,
  worktree_dias_atras: ({ n }: { n: number }) => `ha:${n}`, worktree_subagentes: () => 'subagentes' }));
const MAIN = { current: 'main' as string | null, branches: ['main', 'x'], remotes: [] as string[], dirty: false, worktree_parent: null as string | null };
const folder = vi.hoisted(() => ({ value: null as unknown, worktrees: [] as unknown[] }));
folder.value = { ...MAIN };
vi.mock('@hangar/core', async (original) => ({ ...await original<typeof import('@hangar/core')>(),
  getFolderBranchesForServer: () => Promise.resolve(folder.value),
  getWorktreesForServer: () => Promise.resolve([{ repo: '/r', worktrees: folder.worktrees }]) }));
import { BranchPicker } from './BranchPicker';

const wt = (name: string, sessions: string[] = []) => ({ path: `/r/.worktrees/${name}`, repo: '/r', exists: true, branch: name, sessions });

async function mount(props: Partial<{ cwd: string; value: unknown }> = {}) {
  const onChange = vi.fn();
  const onPickFolder = vi.fn();
  const el = document.createElement('div');
  await act(async () => { createRoot(el).render(createElement(BranchPicker, {
    server: { id: 's' } as never, cwd: props.cwd ?? '/r', sessionName: 'rust-parte3', value: (props.value ?? null) as never, onChange, onPickFolder })); });
  return { el, onChange, onPickFolder };
}
const press = (el: HTMLElement, label: string) => act(async () => { (el.querySelector(`[aria-label="${label}"]`) as HTMLElement).click(); });
const type = (el: HTMLElement, text: string) => act(async () => {
  const input = el.querySelector('[aria-label="branch"]') as HTMLInputElement;
  Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')!.set!.call(input, text);
  input.dispatchEvent(new Event('input', { bubbles: true }));
});

describe('BranchPicker', () => {
  // Sem nome digitado a branch vai vazia: o store a preenche com o nome final da sessão.
  it('worktree nova sem nome devolve new_branch com a base atual', async () => {
    const { el, onChange } = await mount();
    expect(onChange).toHaveBeenLastCalledWith(null);
    await press(el, 'nova');
    expect(onChange).toHaveBeenLastCalledWith({ branch: '', new_branch: true, base: 'main' });
  });

  it('a única lista de branches é a das bases, escondida até tocar em "Parte de"', async () => {
    const { el, onChange } = await mount();
    expect(el.querySelector('[aria-label="x"]')).toBeNull();
    await press(el, 'nova');
    expect(el.querySelector('[aria-label="x"]')).toBeNull();
    await press(el, 'parte main');
    await press(el, 'x');
    expect(onChange).toHaveBeenLastCalledWith({ branch: '', new_branch: true, base: 'x' });
    expect(el.querySelector('[aria-label="x"]')).toBeNull();
  });

  it('nome digitado de uma branch que já existe abre a worktree nela e trava a base', async () => {
    const { el, onChange } = await mount();
    await press(el, 'nova');
    await type(el, 'x');
    expect(onChange).toHaveBeenLastCalledWith({ branch: 'x' });
    expect(el.querySelector('[aria-label="parte main"]')!.getAttribute('data-disabled')).toBe('true');
    expect(el.textContent).toContain('existe:x');
    await type(el, 'nova-coisa');
    expect(onChange).toHaveBeenLastCalledWith({ branch: 'nova-coisa', new_branch: true, base: 'main' });
  });

  it('branch só no remoto avisa que cria a local; a atual avisa que já está na pasta', async () => {
    folder.value = { ...MAIN, remotes: ['feat'] };
    try {
      const { el, onChange } = await mount();
      await press(el, 'nova');
      await type(el, 'feat');
      expect(el.textContent).toContain('existe:feat (remoto)');
      await type(el, 'main');
      expect(onChange).toHaveBeenLastCalledWith({ branch: 'main', new_branch: true, base: 'main' });
      expect(el.textContent).toContain('aberta:main@');
    } finally { folder.value = { ...MAIN }; }
  });

  it('mostra a pasta onde a worktree nasce, com o nome passado pela regra do servidor', async () => {
    folder.value = { ...MAIN, worktree_parent: '/home/u/wt/r/' };
    try {
      const { el } = await mount();
      await press(el, 'nova');
      expect(el.textContent).toContain('/home/u/wt/r/nome-sessao');
      await type(el, 'Área X');
      expect(el.textContent).toContain('/home/u/wt/r/Area-X');
    } finally { folder.value = { ...MAIN }; }
  });

  it('remontar com uma worktree nova escolhida mantém o nome digitado e a base', async () => {
    const { el, onChange } = await mount({ value: { branch: 'novo', new_branch: true, base: 'main' } });
    expect(el.querySelector('[aria-label="branch"]')!.getAttribute('data-placeholder-color')).toBe(theme.tokens.text.muted);
    expect(onChange).toHaveBeenLastCalledWith({ branch: 'novo', new_branch: true, base: 'main' });
    expect(onChange).not.toHaveBeenCalledWith(null);
  });

  it('HEAD solto: a base da worktree nova é a primeira branch de verdade', async () => {
    folder.value = { ...MAIN, current: null, branches: ['dev', 'main'] };
    try {
      const { el, onChange } = await mount();
      await press(el, 'nova');
      expect(onChange).toHaveBeenLastCalledWith({ branch: '', new_branch: true, base: 'dev' });
    } finally { folder.value = { ...MAIN }; }
  });

  it('escolher uma worktree que já existe abre a sessão na pasta dela; ocupada avisa e mantém o painel', async () => {
    folder.worktrees = [wt('livre'), wt('cheia', ['outra']), wt('agent-0123abcd')];
    try {
      const { el, onPickFolder } = await mount();
      expect(el.textContent).toContain('3 wts · 1 em-uso');
      await press(el, 'existente');
      expect(el.querySelector('[aria-label="agent-0123abcd"]')).toBeNull();
      await press(el, 'subagentes');
      expect(el.querySelector('[aria-label="agent-0123abcd"]')).toBeTruthy();
      await press(el, 'cheia');
      expect(onPickFolder).toHaveBeenLastCalledWith('/r/.worktrees/cheia', true);
      expect(el.querySelector('[aria-label="voltar"]')).toBeTruthy();
      await press(el, 'livre');
      expect(onPickFolder).toHaveBeenLastCalledWith('/r/.worktrees/livre', false);
    } finally { folder.worktrees = []; }
  });

  it('dentro de uma worktree ocupada mostra o aviso, e "Direto na sua pasta" volta à principal', async () => {
    folder.worktrees = [wt('cheia', ['outra'])];
    try {
      const { el, onPickFolder } = await mount({ cwd: '/r/.worktrees/cheia' });
      expect(el.textContent).toContain('ocupada:outra');
      await press(el, 'pasta');
      expect(onPickFolder).toHaveBeenLastCalledWith('/r', false);
    } finally { folder.worktrees = []; }
  });

  it('sem worktrees o resumo diz que não há nenhuma', async () => {
    const { el } = await mount();
    expect(el.textContent).toContain('nenhuma');
  });
});
