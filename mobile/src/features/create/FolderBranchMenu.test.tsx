// @vitest-environment happy-dom
import { act, createElement } from 'react';
import { createRoot } from 'react-dom/client';
import { beforeEach, describe, expect, it, vi } from 'vitest';
(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
vi.mock('react-native-unistyles', async (original) => ({ ...await original<typeof import('react-native-unistyles')>(),
  useUnistyles: () => ({ theme: { tokens: { text: { muted: '#111' }, accent: { base: '#222' } } } }) }));
vi.mock('react-native', async (original) => ({ ...await original<typeof import('react-native')>(),
  Pressable: (p: { accessibilityLabel?: string; onPress?: () => void; disabled?: boolean; children?: unknown }) =>
    createElement('button', { 'aria-label': p.accessibilityLabel, 'data-disabled': p.disabled ? 'true' : undefined, onClick: p.disabled ? undefined : p.onPress }, p.children as never),
  TextInput: (p: { accessibilityLabel?: string; onChangeText?: (t: string) => void }) =>
    createElement('input', { 'aria-label': p.accessibilityLabel, onChange: (e: { target: { value: string } }) => p.onChangeText?.(e.target.value) }) }));
vi.mock('../../ui/Icon', () => ({ Icon: () => null }));
vi.mock('../../ui/haptics', () => ({ haptic: { success: vi.fn(), error: vi.fn(), selection: vi.fn() } }));
vi.mock('../usage/HomeUsageCard', () => ({
  TextTabs: ({ options, onChange }: { options: { v: string; label: string }[]; onChange: (v: string) => void }) =>
    options.map((o) => createElement('button', { key: o.v, 'aria-label': o.label, onClick: () => onChange(o.v) })) }));
vi.mock('../../paraglide/messages', () => ({
  comum_voltar: () => 'voltar', comum_cancelar: () => 'cancelar', native_folder_git_branch_title: () => 'titulo',
  native_folder_git_switch_tab: () => 'aba-trocar', native_folder_git_create_tab: () => 'aba-criar', native_folder_git_remote: () => 'remota',
  native_folder_git_current: () => 'atual', native_folder_git_name: () => 'nome', native_folder_git_checkout: () => 'trocar-para-ela',
  native_folder_git_create: () => 'criar', native_folder_git_base: () => 'base', native_folder_git_switch_anyway: () => 'trocar-mesmo',
  native_folder_git_dirty: ({ n }: { n: number }) => `suja:${n}`, native_folder_git_switched: ({ branch }: { branch: string }) => `em:${branch}`,
  native_folder_git_created: ({ branch }: { branch: string }) => `criada:${branch}`,
  native_folder_git_sessions: ({ sessoes, branch }: { sessoes: string; branch: string }) => `sessoes:${sessoes}:${branch}`,
  native_git_branch_search: () => 'buscar', native_git_no_branch: () => 'sem-branch', native_git_elsewhere_tag: ({ pasta }: { pasta: string }) => `em ${pasta}`,
  native_git_elsewhere_body: ({ branch }: { branch: string }) => `aberta:${branch}`, native_git_elsewhere_main: () => 'principal',
  native_git_elsewhere_close_help: () => 'ajuda', native_git_elsewhere_use: () => 'abrir-la', native_git_elsewhere_close: () => 'fechar-e-trazer',
  native_git_elsewhere_busy: ({ pasta, nomes }: { pasta: string; nomes: string }) => `ocupada:${pasta}:${nomes}`,
  native_git_elsewhere_dirty: ({ pasta }: { pasta: string }) => `alterada:${pasta}`,
  native_git_elsewhere_closed_failed: ({ pasta, motivo }: { pasta: string; motivo: string }) => `fechada:${pasta}:${motivo}`,
  native_git_elsewhere_failed: ({ pasta, motivo }: { pasta: string; motivo: string }) => `falhou:${pasta}:${motivo}` }));
const api = vi.hoisted(() => ({ switchTo: vi.fn(), create: vi.fn(), remove: vi.fn() }));
vi.mock('@hangar/core', async (original) => ({ ...await original<typeof import('@hangar/core')>(),
  folderGitSwitchForServer: api.switchTo, folderGitBranchForServer: api.create, deleteWorktreeForServer: api.remove }));
import { FolderBranchMenu } from './FolderBranchMenu';

const INFO = { current: 'main', branches: ['main', 'dev', 'presa'], remotes: ['so-remota'], dirty: false,
  elsewhere: { presa: { path: '/wt/presa', main: false } } };
const GIT = { repo: true, current: 'main', upstream: 'origin/main', dirty: 0, sessions: [] as string[] };

async function mount(git: Partial<typeof GIT> = {}) {
  const props = { onBack: vi.fn(), onDone: vi.fn(), onRefused: vi.fn(), onOpenFolder: vi.fn() };
  const el = document.createElement('div');
  await act(async () => { createRoot(el).render(createElement(FolderBranchMenu, {
    server: { id: 's' } as never, cwd: '/r', info: INFO, git: { ...GIT, ...git }, ...props })); });
  return { el, ...props };
}
const press = (el: HTMLElement, label: string) => act(async () => { (el.querySelector(`[aria-label="${label}"]`) as HTMLElement).click(); });
const type = (el: HTMLElement, label: string, text: string) => act(async () => {
  const input = el.querySelector(`[aria-label="${label}"]`) as HTMLInputElement;
  Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')!.set!.call(input, text);
  input.dispatchEvent(new Event('input', { bubbles: true }));
});

describe('FolderBranchMenu', () => {
  beforeEach(() => { api.switchTo.mockReset(); api.create.mockReset(); api.remove.mockReset(); });

  it('tocar numa branch troca a pasta e devolve o estado relido', async () => {
    api.switchTo.mockResolvedValue({ ...GIT, current: 'dev' });
    const { el, onDone } = await mount();
    await press(el, 'dev');
    expect(api.switchTo).toHaveBeenCalledWith({ id: 's' }, '/r', 'dev', false);
    expect(onDone).toHaveBeenCalledWith({ ...GIT, current: 'dev' }, 'em:dev');
  });

  it('branch só no remoto também troca; a atual não faz nada', async () => {
    api.switchTo.mockResolvedValue(GIT);
    const { el } = await mount();
    await press(el, 'main');
    expect(api.switchTo).not.toHaveBeenCalled();
    await press(el, 'so-remota');
    expect(api.switchTo).toHaveBeenCalledWith({ id: 's' }, '/r', 'so-remota', false);
  });

  it('pasta suja trava a troca e diz o motivo', async () => {
    const { el } = await mount({ dirty: 2 });
    expect(el.textContent).toContain('suja:2');
    expect(el.querySelector('[aria-label="dev"]')!.getAttribute('data-disabled')).toBe('true');
  });

  it('com sessões na pasta pede confirmação antes de trocar', async () => {
    api.switchTo.mockResolvedValue(GIT);
    const { el } = await mount({ sessions: ['outra'] });
    await press(el, 'dev');
    expect(api.switchTo).not.toHaveBeenCalled();
    expect(el.textContent).toContain('sessoes:outra:dev');
    await press(el, 'trocar-mesmo');
    expect(api.switchTo).toHaveBeenCalledWith({ id: 's' }, '/r', 'dev', true);
  });

  it('a recusa de sessões vinda do servidor também pede confirmação', async () => {
    api.switchTo.mockRejectedValue(Object.assign(new Error('409: x'), { code: 'erro_git_folder_sessions' }));
    const { el, onRefused } = await mount();
    await press(el, 'dev');
    expect(el.querySelector('[aria-label="trocar-mesmo"]')).toBeTruthy();
    expect(onRefused).toHaveBeenCalled();
  });

  it('branch aberta noutra pasta oferece abrir lá ou fechar e trazer', async () => {
    api.remove.mockResolvedValue({});
    api.switchTo.mockResolvedValue({ ...GIT, current: 'presa' });
    const { el, onDone, onOpenFolder } = await mount();
    await press(el, 'presa');
    expect(api.switchTo).not.toHaveBeenCalled();
    expect(el.textContent).toContain('aberta:presa');
    await press(el, 'abrir-la');
    expect(onOpenFolder).toHaveBeenCalledWith('/wt/presa');
    await press(el, 'fechar-e-trazer');
    expect(api.remove).toHaveBeenCalledWith({ id: 's' }, { repo: '/wt/presa', path: '/wt/presa', confirm: false, delete_branch: false, keep_branch: true });
    expect(onDone).toHaveBeenCalledWith({ ...GIT, current: 'presa' }, 'em:presa');
  });

  it('fechar a outra pasta recusado não troca e diz por quê', async () => {
    api.remove.mockRejectedValue(new Error('409: sessão aberta dentro: filha'));
    const { el } = await mount();
    await press(el, 'presa');
    await press(el, 'fechar-e-trazer');
    expect(api.switchTo).not.toHaveBeenCalled();
    expect(el.textContent).toContain('ocupada:presa:filha');
  });

  it('troca que falha depois de fechar a outra pasta diz que ela já foi fechada', async () => {
    api.remove.mockResolvedValue({});
    api.switchTo.mockRejectedValue(new Error('409: conflito'));
    const { el } = await mount();
    await press(el, 'presa');
    await press(el, 'fechar-e-trazer');
    expect(el.textContent).toContain('fechada:presa:conflito');
  });

  it('criar leva o nome, a base escolhida e se troca para ela', async () => {
    api.create.mockResolvedValue({ ...GIT, current: 'nova' });
    const { el, onDone } = await mount();
    await press(el, 'aba-criar');
    expect(el.querySelector('[aria-label="criar"]')!.getAttribute('data-disabled')).toBe('true');
    await type(el, 'nome', ' nova ');
    await press(el, 'dev');
    await press(el, 'criar');
    expect(api.create).toHaveBeenCalledWith({ id: 's' }, '/r', 'nova', { checkout: true, base: 'dev', confirmSessions: false });
    expect(onDone).toHaveBeenCalledWith({ ...GIT, current: 'nova' }, 'criada:nova');
  });

  it('criar sem trocar não para na pasta suja nem nas sessões', async () => {
    api.create.mockResolvedValue(GIT);
    const { el } = await mount({ dirty: 1, sessions: ['outra'] });
    await press(el, 'aba-criar');
    await type(el, 'nome', 'nova');
    expect(el.querySelector('[aria-label="criar"]')!.getAttribute('data-disabled')).toBe('true');
    await press(el, 'trocar-para-ela');
    await press(el, 'criar');
    expect(api.create).toHaveBeenCalledWith({ id: 's' }, '/r', 'nova', { checkout: false, base: undefined, confirmSessions: false });
  });
});
