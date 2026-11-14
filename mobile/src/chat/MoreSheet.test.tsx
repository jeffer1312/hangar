// @vitest-environment happy-dom
import { act, createElement, forwardRef, useImperativeHandle, useState, type ReactNode } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';
import { MoreSheet } from './MoreSheet';
import { _resetChatsForTests } from '../stores/chat';
import { useMods } from '../stores/mods';
import { useServers } from '../stores/servers';
import * as m from '../paraglide/messages';

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
const boundary = vi.hoisted(() => ({ push: vi.fn(), finishDismiss: null as (() => void) | null }));
vi.mock('expo-router', () => ({ useRouter: () => ({ push: boundary.push }) }));
vi.mock('expo-secure-store', () => ({ getItemAsync: async () => null, setItemAsync: async () => {} }));
vi.mock('react-native-mmkv', () => ({ createMMKV: () => ({ getString: () => undefined, set: () => {} }) }));
vi.mock('expo-blur', async () => ({ BlurView: (await import('react-native')).View }));
vi.mock('expo-glass-effect', async () => ({ GlassView: (await import('react-native')).View, isLiquidGlassAvailable: () => false }));
vi.mock('sonner-native', () => ({ toast: Object.assign(() => {}, { success: () => {}, error: () => {}, dismiss: () => {} }), Toaster: () => null }));
vi.mock('@lodev09/react-native-true-sheet', () => ({ TrueSheet: forwardRef(function NativeSheet({ children, onDidDismiss }: { children: ReactNode; onDidDismiss?: () => void }, ref) {
  const [visible, setVisible] = useState(false);
  useImperativeHandle(ref, () => ({ present: async () => { setVisible(true); }, dismiss: async () => { setVisible(false); } }));
  boundary.finishDismiss = () => { setVisible(false); onDidDismiss?.(); };
  return visible ? createElement('div', null, children) : null;
}) }));
vi.mock('react-native', async (original) => {
  const actual = await original<typeof import('react-native')>();
  return { ...actual, Pressable: ({ accessibilityState, ...props }: Record<string, unknown>) => createElement(actual.Pressable, props) };
});

const own = { id: 'own', label: 'owner-server', baseUrl: 'http://owner.test:8765', token: 'owner-token' };
const guest = { id: 'guest', label: 'guest-server', baseUrl: 'https://guest.ts.net:8443', token: 'guest-token', invite: true };
const removed = { ...own, id: 'removed', removed: true };
const roots: Root[] = [];

beforeEach(() => {
  boundary.push.mockReset();
  boundary.finishDismiss = null;
  useServers.setState({ servers: [own, guest, removed], activeId: guest.id, ready: true, vaultReadable: true, aviso: null });
  useMods.setState({ visible: false });
  vi.stubGlobal('fetch', vi.fn(() => { throw new Error('unexpected-network'); }));
});
afterEach(async () => {
  await act(async () => { roots.splice(0).forEach(root => root.unmount()); });
  _resetChatsForTests();
  vi.unstubAllGlobals();
});

async function render(serverId: string, orq = false, name = 'conversation', provider?: string | null) {
  const container = document.createElement('div');
  const root = createRoot(container);
  roots.push(root);
  const onClose = vi.fn();
  function Host() {
    const [open, setOpen] = useState(true);
    return createElement(MoreSheet, { open, onClose: () => { onClose(); setOpen(false); }, serverId, name, orq, provider });
  }
  await act(async () => { root.render(createElement(Host)); });
  const finishDismiss = boundary.finishDismiss!;
  return { container, onClose, finishDismiss: async () => { await act(async () => { finishDismiss(); }); } };
}

const shareButton = (container: HTMLElement) => container.querySelector<HTMLButtonElement>(`[aria-label="${m.compartilhar_menu()}"]`);
const gitButton = (container: HTMLElement) => container.querySelector<HTMLButtonElement>(`[aria-label="${m.sessao_git()}"]`);

test('Codex espera o fechamento nativo e abre o destino uma única vez', async () => {
  const { container, onClose, finishDismiss } = await render(own.id, false, 'conversation', 'codex');
  const files = container.querySelector<HTMLButtonElement>(`[aria-label="${m.arq_aba()}"]`);
  expect(files).not.toBeNull();
  await act(async () => { files!.click(); });
  expect(onClose).toHaveBeenCalledExactlyOnceWith();
  expect(boundary.push).not.toHaveBeenCalled();

  await finishDismiss();
  expect(boundary.push).toHaveBeenCalledExactlyOnceWith('/s/own/conversation/files');
  await finishDismiss();
  expect(boundary.push).toHaveBeenCalledExactlyOnceWith('/s/own/conversation/files');
});

test('dispensar o menu sem escolher uma ação não abre outro destino', async () => {
  const { onClose, finishDismiss } = await render(own.id, false, 'conversation', 'codex');
  await finishDismiss();
  expect(onClose).toHaveBeenCalledExactlyOnceWith();
  expect(boundary.push).not.toHaveBeenCalled();
});

test('troca de conta: Claude no servidor próprio fecha o menu e abre a mesma conversa mesmo com convidado ativo', async () => {
  const { container, onClose, finishDismiss } = await render(own.id, false, 'same-conversation', 'claude');
  const account = container.querySelector<HTMLButtonElement>(`[aria-label="${m.bastao_menu()}"]`);
  expect(account, 'ação de continuar em outra conta').not.toBeNull();
  expect(account!.textContent).toContain(m.conta_mesma_conversa_sub());
  await act(async () => { account!.click(); });
  expect(onClose).toHaveBeenCalledExactlyOnceWith();
  await finishDismiss();
  expect(boundary.push).toHaveBeenCalledExactlyOnceWith('/s/own/same-conversation/account');
  expect(fetch).not.toHaveBeenCalled();
});

test.each([
  { kind: 'guest', serverId: guest.id, orq: false, provider: 'claude' },
  { kind: 'removed', serverId: removed.id, orq: false, provider: 'claude' },
  { kind: 'unknown-server', serverId: 'unknown', orq: false, provider: 'claude' },
  { kind: 'orq', serverId: own.id, orq: true, provider: 'claude' },
  { kind: 'codex', serverId: own.id, orq: false, provider: 'codex' },
  { kind: 'pi', serverId: own.id, orq: false, provider: 'pi' },
  { kind: 'omp', serverId: own.id, orq: false, provider: 'omp' },
  { kind: 'kimi', serverId: own.id, orq: false, provider: 'kimi' },
  { kind: 'unknown-provider', serverId: own.id, orq: false, provider: 'other' },
  { kind: 'null-provider', serverId: own.id, orq: false, provider: null },
  { kind: 'missing-provider', serverId: own.id, orq: false, provider: undefined },
])('troca de conta: não oferece ação para $kind mesmo com servidor próprio ativo', async ({ serverId, orq, provider }) => {
  useServers.setState({ activeId: own.id });
  const { container, onClose } = await render(serverId, orq, 'conversation', provider);
  expect(container.querySelector(`[aria-label="${m.bastao_menu()}"]`)).toBeNull();
  expect(onClose).not.toHaveBeenCalled();
  expect(boundary.push).not.toHaveBeenCalled();
  expect(fetch).not.toHaveBeenCalled();
});

test.each([false, true])('git abre a rota da conversa e fecha o menu no servidor próprio, orq=%s', async orq => {
  const { container, onClose, finishDismiss } = await render(own.id, orq);
  const git = gitButton(container);
  expect(git).not.toBeNull();
  expect(git!.previousElementSibling?.getAttribute('aria-label')).toBe(m.arq_aba());
  expect(git!.textContent).toContain(m.chat_acao_git_detalhe());
  await act(async () => { git!.click(); });
  expect(onClose).toHaveBeenCalledExactlyOnceWith();
  await finishDismiss();
  expect(boundary.push).toHaveBeenCalledExactlyOnceWith('/s/own/conversation/git');
  expect(fetch).not.toHaveBeenCalled();
});

test.each([guest.id, removed.id, 'unknown'])('git não aparece no servidor %s mesmo com servidor próprio ativo', async serverId => {
  useServers.setState({ activeId: own.id });
  const { container } = await render(serverId);
  expect(gitButton(container)).toBeNull();
});

test.each([
  { kind: 'normal', orq: false, name: 'conversation' },
  { kind: 'orq', orq: true, name: 'orq-run' },
])('Time do trabalho: dono da conversa $kind abre o editor no servidor explícito', async ({ orq, name }) => {
  const { container, onClose, finishDismiss } = await render(own.id, orq, name);
  const team = container.querySelector<HTMLButtonElement>(`[aria-label="${m.orqcfg_aba_papeis()}"]`);
  expect(team).not.toBeNull();
  await act(async () => { team!.click(); });
  expect(onClose).toHaveBeenCalledExactlyOnceWith();
  await finishDismiss();
  expect(boundary.push).toHaveBeenCalledExactlyOnceWith(`/s/own/${name}/orchestration-team`);
  expect(fetch).not.toHaveBeenCalled();
});

test.each([guest.id, removed.id, 'unknown'])('Time do trabalho: servidor %s não oferece editor em conversa normal nem orq', async serverId => {
  useServers.setState({ activeId: own.id });
  for (const orq of [false, true]) {
    const { container } = await render(serverId, orq);
    expect(container.querySelector(`[aria-label="${m.orqcfg_aba_papeis()}"]`)).toBeNull();
  }
  expect(boundary.push).not.toHaveBeenCalled();
  expect(fetch).not.toHaveBeenCalled();
});

test('busca global fecha Mais e abre rota independente da sessão sem mudar ações existentes', async () => {
  const { container, onClose, finishDismiss } = await render(own.id);
  const search = container.querySelector<HTMLButtonElement>(`[aria-label="${m.switcher_buscar_conversas()}"]`);
  expect(search, 'ação de busca global').not.toBeNull();
  await act(async () => { search!.click(); });
  expect(onClose).toHaveBeenCalledExactlyOnceWith();
  expect(boundary.push).not.toHaveBeenCalled();
  await finishDismiss();
  expect(boundary.push).toHaveBeenCalledExactlyOnceWith('/search');
  expect(fetch).not.toHaveBeenCalled();
});

test('servidor próprio da conversa oferece compartilhar entre grupo e arquivos mesmo com convidado ativo', async () => {
  const { container, onClose, finishDismiss } = await render(own.id);
  const share = shareButton(container);
  expect(share).not.toBeNull();
  expect(share!.textContent).toBe(m.compartilhar_menu());
  expect(share!.previousElementSibling?.getAttribute('aria-label')).toBe(m.par_titulo());
  expect(share!.nextElementSibling?.getAttribute('aria-label')).toBe(m.arq_aba());
  expect(container.textContent).not.toContain(m.mobile_share_send_link());
  await act(async () => { share!.click(); });
  expect(onClose).toHaveBeenCalledExactlyOnceWith();
  await finishDismiss();
  expect(boundary.push).toHaveBeenCalledExactlyOnceWith('/s/own/conversation/share');
  expect(fetch).not.toHaveBeenCalled();
});

test.each([guest, removed])('servidor $id da conversa não oferece compartilhar com servidor próprio ativo', async server => {
  useServers.setState({ activeId: own.id });
  const { container } = await render(server.id);
  expect(container.querySelector(`[aria-label="${m.par_titulo()}"]`)).not.toBeNull();
  expect(shareButton(container)).toBeNull();
});

test('conversa de orquestração em servidor próprio não oferece compartilhar', async () => {
  const { container } = await render(own.id, true);
  expect(container.querySelector(`[aria-label="${m.arq_aba()}"]`)).not.toBeNull();
  expect(shareButton(container)).toBeNull();
});

test('orquestração abre seu painel no lugar de Atividade e sessão comum mantém Atividade', async () => {
  const { container, onClose, finishDismiss } = await render(own.id, true);
  const panel = container.querySelector<HTMLButtonElement>(`[aria-label="${m.orq_tab_title()}"]`);
  expect(panel).not.toBeNull();
  expect(container.querySelector(`[aria-label="${m.ctx_atividade()}"]`)).toBeNull();
  await act(async () => { panel!.click(); });
  expect(onClose).toHaveBeenCalledExactlyOnceWith();
  await finishDismiss();
  expect(boundary.push).toHaveBeenCalledExactlyOnceWith('/s/own/conversation/orchestration');

  boundary.push.mockClear();
  const normal = await render(own.id);
  expect(normal.container.querySelector(`[aria-label="${m.orq_tab_title()}"]`)).toBeNull();
  const activity = normal.container.querySelector<HTMLButtonElement>(`[aria-label="${m.ctx_atividade()}"]`);
  expect(activity).not.toBeNull();
  await act(async () => { activity!.click(); });
  expect(normal.onClose).toHaveBeenCalledExactlyOnceWith();
  await normal.finishDismiss();
  expect(boundary.push).toHaveBeenCalledExactlyOnceWith('/s/own/conversation/activity');
});

test('servidor inexistente não oferece compartilhar', async () => {
  useServers.setState({ activeId: own.id });
  const { container } = await render('unknown');
  expect(container.querySelector(`[aria-label="${m.par_titulo()}"]`)).not.toBeNull();
  expect(shareButton(container)).toBeNull();
});
