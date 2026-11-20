// @vitest-environment happy-dom
import { act, createElement, type ReactNode } from 'react';
import { createRoot } from 'react-dom/client';
import { expect, it, vi } from 'vitest';
import { ComposerActions, modeMenuHandlers } from './ComposerField';
import * as m from '../paraglide/messages';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

type MenuProps = { actions: { id: string; title: string; state?: string; attributes?: { disabled?: boolean } }[];
  onPressAction: (e: { nativeEvent: { event: string } }) => void; children: ReactNode };
const menu = vi.hoisted(() => ({ last: null as null | MenuProps }));
// O mock de react-native diz Android: o toque longo é do MenuView.
vi.mock('@react-native-menu/menu', () => ({ MenuView: (props: MenuProps) => { menu.last = props; return createElement('div', null, props.children); } }));
const native = vi.hoisted(() => ({ platform: { OS: 'android', select: (x: Record<string, unknown>) => x.android ?? x.default },
  sheet: { showActionSheetWithOptions: (() => {}) as (o: unknown, cb: (i: number) => void) => void } }));
type A11yProps = { accessible?: boolean; accessibilityLabel?: string; accessibilityValue?: { text?: string };
  accessibilityActions?: { name: string; label: string }[]; onAccessibilityAction?: (e: { nativeEvent: { actionName: string } }) => void };
const a11y = vi.hoisted(() => ({ groups: [] as A11yProps[] }));
vi.mock('react-native', async (original) => {
  const actual = await original<typeof import('react-native')>();
  // O leitor de tela só alcança elemento acessível: guarda os que se declaram assim.
  const View = (props: A11yProps & { children?: ReactNode }) => { if (props.accessible) a11y.groups.push(props); return createElement(actual.View, props); };
  return { ...actual, View, Platform: native.platform, ActionSheetIOS: native.sheet };
});
vi.mock('../ui/Icon', () => ({ Icon: () => null }));
vi.mock('../ui/Glass', () => ({ Glass: ({ children }: { children: ReactNode }) => createElement('div', null, children) }));
vi.mock('../ui/MultilineInput', () => ({ MultilineInput: () => null }));
vi.mock('../ui/RecordingWave', () => ({ RecordingWave: () => null }));

function render(status: 'ready' | 'setup' | 'error', sendDisabled: boolean) {
  const onPick = vi.fn();
  const onSend = vi.fn();
  const container = document.createElement('div');
  const root = createRoot(container);
  act(() => root.render(createElement(ComposerActions, {
    attach: { onPress: vi.fn(), disabled: false },
    mic: { onPress: vi.fn(), disabled: false, recording: false, styleLabel: 'cru', onStyle: vi.fn() },
    send: { onPress: onSend, disabled: sendDisabled, label: 'enviar' },
    settings: null,
    modeMenu: { delivery: { mode: 'direct', status, help: 'jev-help' }, onPick },
  })));
  return { container, root, onPick, onSend };
}

it('toque longo no Enviar troca o modo sem enviar; o Enviar continua desativado com o campo vazio', () => {
  const { container, root, onPick, onSend } = render('ready', true);
  // O PopupMenu do Android não marca a opção (state é só iOS): a atual vem dita no título.
  expect(menu.last!.actions.map((a) => a.title)).toEqual([m.delivery_mode_queue(),
    m.mobile_queue_mode_selected({ mode: m.delivery_mode_direct() }), m.delivery_mode_jev(), m.delivery_mode_conversation()]);
  act(() => menu.last!.onPressAction({ nativeEvent: { event: 'conversation' } }));
  expect(onPick).toHaveBeenLastCalledWith('conversation');
  onPick.mockClear();
  act(() => menu.last!.onPressAction({ nativeEvent: { event: 'jev' } }));
  expect(onPick).toHaveBeenCalledWith('jev');
  act(() => menu.last!.onPressAction({ nativeEvent: { event: 'direct' } }));
  expect(onPick).toHaveBeenCalledTimes(1);
  const send = container.querySelector<HTMLButtonElement>('[aria-label="enviar"]')!;
  expect(send.disabled).toBe(true);
  act(() => send.click());
  expect(onSend).not.toHaveBeenCalled();
  act(() => root.unmount());
});

it('Jev indisponível vem desativado no menu e não é escolhido', () => {
  const { root, onPick } = render('error', false);
  expect(menu.last!.actions.find((a) => a.id === 'jev')?.attributes?.disabled).toBe(true);
  act(() => menu.last!.onPressAction({ nativeEvent: { event: 'jev' } }));
  expect(onPick).not.toHaveBeenCalled();
  act(() => root.unmount());
});

it('servidor sem Jev configurado: o menu não oferece o Jev', () => {
  const { root } = render('setup', false);
  expect(menu.last!.actions.map((a) => a.id)).not.toContain('jev');
  act(() => root.unmount());
});

it('iOS: folha de ações só com os modos possíveis, a atual e a causa do Jev na mensagem', () => {
  const sheet = vi.fn();
  native.sheet.showActionSheetWithOptions = sheet;
  const onPick = vi.fn();
  modeMenuHandlers({ delivery: { mode: 'jev', status: 'error', help: 'jev-help' }, onPick })!.openIOS();
  const [options, done] = sheet.mock.calls[0] as [{ options: string[]; cancelButtonIndex: number; message: string }, (i: number) => void];
  expect(options.options).toEqual([m.delivery_mode_queue(), m.delivery_mode_direct(), m.delivery_mode_conversation(), m.comum_cancelar()]);
  expect(options.cancelButtonIndex).toBe(3);
  expect(options.message).toContain(m.mobile_queue_mode_current({ mode: m.delivery_mode_queue() }));
  expect(options.message).toContain('jev-help');
  done(3);
  expect(onPick).not.toHaveBeenCalled();
  // Fila aparece como a atual, mas o salvo é Jev: escolher Fila grava a troca.
  done(0);
  expect(onPick).toHaveBeenLastCalledWith('queued');
  done(1);
  expect(onPick).toHaveBeenLastCalledWith('direct');
});

it('leitor de tela: uma ação por modo possível, nas duas plataformas', () => {
  const onPick = vi.fn();
  const handlers = modeMenuHandlers({ delivery: { mode: 'queued', status: 'setup', help: 'jev-help' }, onPick })!;
  expect(handlers.a11yActions).toEqual([{ name: 'queued', label: m.delivery_mode_queue() }, { name: 'direct', label: m.delivery_mode_direct() },
    { name: 'conversation', label: m.delivery_mode_conversation() }]);
  handlers.pick('direct');
  expect(onPick).toHaveBeenCalledWith('direct');
});

it.each([['android'], ['ios']])('%s: Enviar desativado deixa os modos num elemento que o leitor de tela alcança', (os) => {
  native.platform.OS = os;
  try {
    a11y.groups = [];
    const { root, onPick } = render('setup', true);
    const group = a11y.groups.at(-1)!;
    expect(group.accessibilityLabel).toBe(m.mobile_queue_mode_label());
    expect(group.accessibilityValue?.text).toBe(m.delivery_mode_direct());
    expect(group.accessibilityActions).toEqual([{ name: 'queued', label: m.delivery_mode_queue() }, { name: 'direct', label: m.delivery_mode_direct() },
      { name: 'conversation', label: m.delivery_mode_conversation() }]);
    act(() => group.onAccessibilityAction!({ nativeEvent: { actionName: 'queued' } }));
    expect(onPick).toHaveBeenCalledWith('queued');
    act(() => root.unmount());
    a11y.groups = [];
    const enabled = render('setup', false);
    // Enviar ativo carrega as ações ele mesmo; o grupo não esconde o botão.
    expect(a11y.groups).toEqual([]);
    act(() => enabled.root.unmount());
  } finally {
    native.platform.OS = 'android';
  }
});
