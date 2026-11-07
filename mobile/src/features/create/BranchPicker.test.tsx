// @vitest-environment happy-dom
import { act, createElement } from 'react';
import { createRoot } from 'react-dom/client';
import { describe, expect, it, vi } from 'vitest';
(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
const theme = vi.hoisted(() => ({ tokens: { text: { muted: '#123456', primary: '#111111', secondary: '#222222' }, accent: { base: '#abcdef' }, status: { success: '#333333', warning: '#444444' } } }));
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
  worktree_dias_atras: ({ n }: { n: number }) => `ha:${n}`, worktree_subagentes: () => 'subagentes',
  native_folder_git_no_upstream: () => 'sem-remota', native_folder_git_up_to_date: () => 'atualizada', native_folder_git_behind: ({ n }: { n: number }) => `${n} atras`,
  native_folder_git_ahead: ({ n }: { n: number }) => `${n} frente`,
  native_folder_git_fetched: ({ quando }: { quando: string }) => `fetch:${quando}`, native_folder_git_never_fetched: () => 'sem-fetch',
  native_folder_git_fetch: () => 'fetch', native_folder_git_pull: () => 'pull', native_folder_git_fetch_done: () => 'fetch-ok',
  native_folder_git_pull_done: ({ upstream }: { upstream: string }) => `pull-ok:${upstream}`, native_folder_git_dirty: ({ n }: { n: number }) => `suja:${n}`,
  native_folder_git_no_upstream_pull: () => 'sem-de-onde', native_folder_git_diverged: ({ upstream }: { upstream: string }) => `divergiu:${upstream}`,
  native_folder_git_branch_entry: () => 'trocar-ou-criar', worktree_menu_buscar: () => 'buscar', native_create_checkout_dirty: () => 'fica-na-pasta', worktree_menu_no_match: () => 'sem-resultado',
  worktree_menu_commits_left_out: ({ branch, n }: { branch: string; n: number }) => `fora:${branch}:${n}` }));
vi.mock('../../ui/haptics', () => ({ haptic: { success: vi.fn(), error: vi.fn(), selection: vi.fn() } }));
vi.mock('./FolderBranchMenu', () => ({ FolderBranchMenu: () => createElement('div', { 'data-menu': 'branches' }) }));
const MAIN = { current: 'main' as string | null, branches: ['main', 'x'], remotes: [] as string[], dirty: false, worktree_parent: null as string | null };
const folder = vi.hoisted(() => ({ value: null as unknown, held: null as Promise<unknown> | null, worktrees: [] as unknown[], git: null as unknown, gitFails: false, action: null as unknown as (action: string) => Promise<unknown> }));
folder.value = { ...MAIN };
const GIT = { repo: true, current: 'main', upstream: 'origin/main', dirty: 0, ahead: 0, behind: 2, last_fetch: null };
folder.git = GIT;
folder.action = () => Promise.resolve(folder.git);
vi.mock('@hangar/core', async (original) => ({ ...await original<typeof import('@hangar/core')>(),
  getFolderBranchesForServer: () => folder.held ?? Promise.resolve(folder.value),
  getFolderGitForServer: () => folder.gitFails ? Promise.reject(new Error('sem rede')) : Promise.resolve(folder.git),
  folderGitActionForServer: (_server: unknown, _cwd: string, action: string) => folder.action(action),
  getWorktreesForServer: () => Promise.resolve([{ repo: '/r', worktrees: folder.worktrees }]) }));
import { BranchPicker } from './BranchPicker';

const wt = (name: string, sessions: string[] = []) => ({ path: `/r/.worktrees/${name}`, repo: '/r', exists: true, branch: name, sessions });

async function mount(props: Partial<{ cwd: string; worktreePath: string; value: unknown; basePicked: string }> = {}) {
  const onChange = vi.fn();
  const onPickFolder = vi.fn();
  const el = document.createElement('div');
  await act(async () => { createRoot(el).render(createElement(BranchPicker, {
    server: { id: 's' } as never, cwd: props.cwd ?? '/r', worktreePath: props.worktreePath, sessionName: 'rust-parte3', value: (props.value ?? null) as never, onChange, basePicked: { current: props.basePicked ?? null }, onPickFolder })); });
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

  it('branch local atrás: a worktree nova parte da remota e avisa o commit que fica de fora', async () => {
    folder.value = { ...MAIN, upstreams: { main: { upstream: 'origin/main', ahead: 1, behind: 112 } } };
    try {
      const { el, onChange } = await mount();
      await press(el, 'nova');
      expect(onChange).toHaveBeenLastCalledWith({ branch: '', new_branch: true, base: 'origin/main' });
      expect(el.textContent).toContain('fora:main:1');
      await press(el, 'parte origin/main');
      expect(el.textContent).toContain('112 atras');
      await press(el, 'main');
      expect(onChange).toHaveBeenLastCalledWith({ branch: '', new_branch: true, base: 'main' });
      expect(el.textContent).not.toContain('fora:main');
    } finally { folder.value = { ...MAIN }; }
  });

  it('o fetch da abertura leva para a remota a base padrão, mas não a escolhida', async () => {
    const behind = { ...MAIN, upstreams: { main: { upstream: 'origin/main', ahead: 0, behind: 3 } } };
    folder.action = () => { folder.value = behind; return Promise.resolve(folder.git); };
    try {
      const { el, onChange } = await mount();
      await press(el, 'nova');
      expect(onChange).toHaveBeenLastCalledWith({ branch: '', new_branch: true, base: 'origin/main' });
      folder.value = { ...MAIN };
      const reopened = await mount({ value: { branch: '', new_branch: true, base: 'main' } });
      expect(reopened.onChange).toHaveBeenLastCalledWith({ branch: '', new_branch: true, base: 'origin/main' });
      folder.value = { ...MAIN };
      // Reabrir depois de digitar uma branch que já existe: o valor do pai vem sem base, e a escolha continua.
      const chosen = await mount({ value: { branch: '', new_branch: true }, basePicked: 'main' });
      expect(chosen.onChange).toHaveBeenLastCalledWith({ branch: '', new_branch: true, base: 'main' });
    } finally { folder.value = { ...MAIN }; folder.action = () => Promise.resolve(folder.git); }
  });

  it('a escolha feita durante a releitura vence a remota que ela traz', async () => {
    let release = (_: unknown) => {};
    folder.action = () => { folder.held = new Promise((done) => { release = done; }); return Promise.resolve(folder.git); };
    try {
      const { el, onChange } = await mount();
      await press(el, 'nova');
      await press(el, 'parte main');
      await press(el, 'x');
      await act(async () => { release({ ...MAIN, upstreams: { main: { upstream: 'origin/main', ahead: 0, behind: 3 } } }); });
      expect(onChange).toHaveBeenLastCalledWith({ branch: '', new_branch: true, base: 'x' });
    } finally { folder.held = null; folder.action = () => Promise.resolve(folder.git); }
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
      expect(onPickFolder).toHaveBeenLastCalledWith('/r/.worktrees/cheia', true, '/r');
      expect(el.querySelector('[aria-label="voltar"]')).toBeTruthy();
      await press(el, 'livre');
      expect(onPickFolder).toHaveBeenLastCalledWith('/r/.worktrees/livre', false, '/r');
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

  it('com uma worktree escolhida o menu lê o repositório e ela segue marcada', async () => {
    folder.worktrees = [wt('livre')];
    try {
      const { el, onPickFolder } = await mount({ cwd: '/r', worktreePath: '/r/.worktrees/livre' });
      expect(el.textContent).toContain('livre');
      await press(el, 'pasta');
      expect(onPickFolder).toHaveBeenLastCalledWith('/r', false);
    } finally { folder.worktrees = []; }
  });

  it('a busca da lista acha worktree pelo nome, inclusive de subagente fechada', async () => {
    folder.worktrees = [wt('livre'), wt('agent-0123abcd')];
    try {
      const { el } = await mount();
      await press(el, 'existente');
      await act(async () => {
        const input = el.querySelector('[aria-label="buscar"]') as HTMLInputElement;
        Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')!.set!.call(input, 'agent');
        input.dispatchEvent(new Event('input', { bubbles: true }));
      });
      expect(el.querySelector('[aria-label="livre"]')).toBeNull();
      expect(el.querySelector('[aria-label="agent-0123abcd"]')).toBeTruthy();
    } finally { folder.worktrees = []; }
  });

  it('busca sem resultado diz isso em vez de deixar a lista em branco', async () => {
    folder.worktrees = [wt('livre')];
    try {
      const { el } = await mount();
      await press(el, 'existente');
      await act(async () => {
        const input = el.querySelector('[aria-label="buscar"]') as HTMLInputElement;
        Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')!.set!.call(input, 'xyz');
        input.dispatchEvent(new Event('input', { bubbles: true }));
      });
      expect(el.textContent).toContain('sem-resultado');
    } finally { folder.worktrees = []; }
  });

  it('cópia nova com a pasta alterada avisa que as mudanças ficam nela', async () => {
    folder.value = { ...MAIN, dirty: true };
    try {
      const { el } = await mount();
      expect(el.textContent).not.toContain('fica-na-pasta');
      await press(el, 'nova');
      expect(el.textContent).toContain('fica-na-pasta');
    } finally { folder.value = { ...MAIN }; }
  });

  it('a linha de trocar ou criar branch abre o menu da pasta', async () => {
    const { el } = await mount();
    await press(el, 'trocar-ou-criar');
    expect(el.querySelector('[data-menu="branches"]')).toBeTruthy();
  });

  it('abrir faz um fetch calado e mostra quantos commits a branch está atrás', async () => {
    const calls: string[] = [];
    folder.action = (action) => { calls.push(action); return Promise.resolve(folder.git); };
    try {
      const { el } = await mount();
      expect(calls).toEqual(['fetch']);
      expect(el.querySelector('[aria-label="2 atras"]')).toBeTruthy();
      expect(el.textContent).toContain('r · origin/main · sem-fetch');
      expect(el.textContent).not.toContain('fetch-ok');
    } finally { folder.action = () => Promise.resolve(folder.git); }
  });

  it('fetch da abertura que falha avisa: sem ele o cartão diria que está atualizada', async () => {
    folder.action = () => Promise.reject(new Error('502: remoto fora'));
    try {
      const { el } = await mount();
      expect(el.textContent).toContain('remoto fora');
    } finally { folder.action = () => Promise.resolve(folder.git); }
  });

  it('Pull atualiza a situação e diz com qual remota ficou igual', async () => {
    const calls: string[] = [];
    folder.action = (action) => { calls.push(action); return Promise.resolve(action === 'pull' ? { ...GIT, behind: 0 } : GIT); };
    try {
      const { el } = await mount();
      await press(el, 'pull');
      expect(calls).toEqual(['fetch', 'pull']);
      expect(el.textContent).toContain('atualizada');
      expect(el.textContent).toContain('pull-ok:origin/main');
    } finally { folder.action = () => Promise.resolve(folder.git); }
  });

  it('pasta suja trava o Pull e diz o motivo antes do toque', async () => {
    folder.git = { ...GIT, dirty: 3 };
    try {
      const { el } = await mount();
      expect(el.querySelector('[aria-label="pull"]')!.getAttribute('data-disabled')).toBe('true');
      expect(el.querySelector('[aria-label="fetch"]')!.getAttribute('data-disabled')).toBeNull();
      expect(el.textContent).toContain('suja:3');
    } finally { folder.git = GIT; }
  });

  it('recusa do servidor aparece sem o status HTTP', async () => {
    folder.action = (action) => action === 'pull' ? Promise.reject(new Error('409: divergiu agora')) : Promise.resolve(folder.git);
    try {
      const { el } = await mount();
      await press(el, 'pull');
      expect(el.textContent).toContain('divergiu agora');
      expect(el.textContent).not.toContain('409');
    } finally { folder.action = () => Promise.resolve(folder.git); }
  });

  it('pull recusado com a releitura também falhando mantém o motivo na tela', async () => {
    folder.action = (action) => action === 'pull' ? Promise.reject(new Error('409: divergiu agora')) : Promise.resolve(folder.git);
    try {
      const { el } = await mount();
      folder.gitFails = true;
      await press(el, 'pull');
      expect(el.textContent).toContain('divergiu agora');
      expect(el.querySelector('[aria-label="pull"]')).toBeTruthy();
    } finally { folder.gitFails = false; folder.action = () => Promise.resolve(folder.git); }
  });

  it('sem worktrees o resumo diz que não há nenhuma', async () => {
    const { el } = await mount();
    expect(el.textContent).toContain('nenhuma');
  });
});
