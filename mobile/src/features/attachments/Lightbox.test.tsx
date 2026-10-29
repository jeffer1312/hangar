// @vitest-environment happy-dom
import { act, createElement, type ReactNode } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, expect, it, vi } from 'vitest';
import type { GestureType } from 'react-native-gesture-handler';

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
const native = vi.hoisted(() => ({
  gestures: [] as GestureType[],
  style: () => ({ transform: [] as Record<string, number>[] }),
}));

// Construtores reais; só a entrega de eventos e a animação nativas são substituídas.
vi.mock('react-native-gesture-handler', async () => {
  const { GestureObjects } = await vi.importActual<{ GestureObjects: typeof import('react-native-gesture-handler').Gesture }>(
    new URL('../../../node_modules/react-native-gesture-handler/lib/module/handlers/gestures/gestureObjects.js', import.meta.url).pathname,
  );
  return {
    Gesture: GestureObjects,
    GestureHandlerRootView: ({ children }: { children: ReactNode }) => children,
    GestureDetector: ({ gesture, children }: { gesture: ReturnType<typeof GestureObjects.Simultaneous>; children: ReactNode }) => {
      gesture.prepare();
      native.gestures = gesture.toGestureArray();
      return children;
    },
  };
});
vi.mock('react-native', async (original) => ({ ...await original<object>(), StatusBar: { currentHeight: 24 } }));
vi.mock('react-native-reanimated', async (original) => ({
  ...await original<object>(),
  useAnimatedStyle: (fn: typeof native.style) => { native.style = fn; return {}; },
}));
vi.mock('./mediaCache', () => ({ localCopy: vi.fn(), saveImage: vi.fn() }));
vi.mock('../../ui/Toast', () => ({ toast: { ok: vi.fn(), erro: vi.fn() } }));
vi.mock('../../ui/Icon', () => ({ Icon: () => null }));
vi.mock('../../paraglide/messages', () => ({
  anexos_fechar_imagem: () => 'fechar', anexos_baixar: () => 'salvar',
  visor_anterior: () => 'anterior', visor_proxima: () => 'proxima',
  anexos_ver: () => 'abrir',
}));

import { Lightbox, type LightboxProps } from './Lightbox';
import { ImageThumb } from '../../chat/ImageThumb';

const roots: Root[] = [];
afterEach(() => { act(() => roots.splice(0).forEach((root) => root.unmount())); });

async function render(props: Partial<LightboxProps> = {}) {
  const container = document.createElement('div');
  const root = createRoot(container);
  roots.push(root);
  const onClose = vi.fn();
  const onStep = vi.fn();
  await act(async () => root.render(createElement(Lightbox, {
    visible: true, uri: 'https://example.test/1.png', filename: '1.png', onClose, onStep, ...props,
  })));
  return { container, root, onClose, onStep };
}

function gesture(name: string) {
  const result = native.gestures.find((g) => g.handlerName === name);
  if (!result) throw new Error(`Gesto ausente: ${name}`);
  return result;
}

function event(translationX = 0, translationY = 0, scale = 1, numberOfPointers = 1) {
  return { translationX, translationY, scale, numberOfPointers, velocityX: 0, velocityY: 0 };
}

function deliver(name: string, phase: 'onBegin' | 'onStart' | 'onUpdate' | 'onEnd' | 'onFinalize', data = event(), success = true) {
  const callback = gesture(name).handlers[phase] as ((e: ReturnType<typeof event>, success: boolean) => void) | undefined;
  act(() => callback?.(data, success));
}

function drag(x: number, y = 0, success = true) {
  deliver('PanGestureHandler', 'onBegin');
  deliver('PanGestureHandler', 'onStart');
  deliver('PanGestureHandler', 'onUpdate', event(x, y));
  deliver('PanGestureHandler', 'onEnd', event(x, y), success);
  deliver('PanGestureHandler', 'onFinalize', event(x, y), success);
}

function translation() {
  return native.style().transform.find((t) => 'translateX' in t)?.translateX;
}

it('sem zoom a foto acompanha o dedo antes de soltar e passa ao soltar', async () => {
  const { onStep, onClose } = await render();
  deliver('PanGestureHandler', 'onBegin');
  deliver('PanGestureHandler', 'onStart');
  deliver('PanGestureHandler', 'onUpdate', event(-95, 4));
  expect(translation()).toBe(-95);
  expect(onStep).not.toHaveBeenCalled();
  deliver('PanGestureHandler', 'onEnd', event(-95, 4));
  deliver('PanGestureHandler', 'onFinalize', event(-95, 4));
  expect(onStep).toHaveBeenCalledExactlyOnceWith(1);
  expect(onClose).not.toHaveBeenCalled();
  expect(translation()).toBe(0);
});

it('toque de fechar não reconhece o mesmo arrasto da galeria no iOS', async () => {
  await render();
  const pan = gesture('PanGestureHandler');
  const pinch = gesture('PinchGestureHandler');
  const tap = gesture('TapGestureHandler');
  expect(pan.config.simultaneousWith).not.toContain(tap);
  expect(pan.config.simultaneousWith).toContain(pinch);
  expect(tap.config).toMatchObject({ maxDist: 10 });
});

it('arrasto cancelado volta ao centro sem trocar de foto', async () => {
  const { onStep, onClose } = await render();
  drag(-100, 0, false);
  expect(onStep).not.toHaveBeenCalled();
  expect(onClose).not.toHaveBeenCalled();
  expect(translation()).toBe(0);
});

it('pinça que retorna ao tamanho original não vira passagem de foto', async () => {
  const { onStep } = await render();
  deliver('PanGestureHandler', 'onBegin');
  deliver('PanGestureHandler', 'onStart');
  deliver('PinchGestureHandler', 'onStart', event(0, 0, 1, 2));
  deliver('PinchGestureHandler', 'onUpdate', event(0, 0, 1.5, 2));
  deliver('PanGestureHandler', 'onUpdate', event(100, 0, 1, 2));
  deliver('PinchGestureHandler', 'onUpdate', event(0, 0, 1, 2));
  deliver('PinchGestureHandler', 'onEnd');
  deliver('PanGestureHandler', 'onEnd', event(100));
  deliver('PanGestureHandler', 'onFinalize', event(100));
  expect(onStep).not.toHaveBeenCalled();
  expect(translation()).toBe(0);
});

it.each([[25, 0], [100, 150]])('arrasto curto ou vertical (%s, %s) não passa nem fecha', async (x, y) => {
  const { onStep, onClose } = await render();
  drag(x, y);
  expect(onStep).not.toHaveBeenCalled();
  expect(onClose).not.toHaveBeenCalled();
  expect(translation()).toBe(0);
});

it('zoom permite panorâmica acumulada e toque não fecha', async () => {
  const { onStep, onClose } = await render();
  deliver('PinchGestureHandler', 'onStart', event(0, 0, 1, 2));
  deliver('PinchGestureHandler', 'onUpdate', event(0, 0, 2, 2));
  deliver('PinchGestureHandler', 'onEnd');
  drag(100, 20);
  drag(40, 10);
  deliver('TapGestureHandler', 'onEnd');
  expect(translation()).toBe(140);
  expect(native.style().transform).toContainEqual({ translateY: 30 });
  expect(native.style().transform).toContainEqual({ scale: 2 });
  expect(onStep).not.toHaveBeenCalled();
  expect(onClose).not.toHaveBeenCalled();
});

it('imagem avulsa não desliza e toque simples ainda fecha', async () => {
  const { onClose } = await render({ onStep: undefined });
  drag(-100);
  expect(translation()).toBe(0);
  deliver('TapGestureHandler', 'onEnd');
  expect(onClose).toHaveBeenCalledTimes(1);
});

it('conversa navega nas duas direções, respeita extremos e mantém as setas', async () => {
  const container = document.createElement('div');
  const root = createRoot(container);
  roots.push(root);
  await act(async () => root.render(createElement(ImageThumb, {
    items: [1, 2].map((n) => ({ uri: `https://example.test/${n}.png`, name: `${n}.png`, kind: 'image' as const })),
    index: 0, style: {},
  })));
  await act(async () => container.querySelector<HTMLButtonElement>('[aria-label="abrir"]')!.click());
  expect(container.textContent).toContain('1/2 · 1.png');
  drag(-100);
  expect(container.textContent).toContain('2/2 · 2.png');
  expect(translation()).toBe(0);
  drag(-100);
  expect(container.textContent).toContain('2/2 · 2.png');
  expect(translation()).toBe(0);
  drag(100);
  expect(container.textContent).toContain('1/2 · 1.png');
  drag(100);
  expect(container.textContent).toContain('1/2 · 1.png');
  await act(async () => container.querySelector<HTMLButtonElement>('[aria-label="proxima"]')!.click());
  expect(container.textContent).toContain('2/2 · 2.png');
  await act(async () => container.querySelector<HTMLButtonElement>('[aria-label="anterior"]')!.click());
  expect(container.textContent).toContain('1/2 · 1.png');
});

it('fechar por fora (voltar do Android) e reabrir a mesma imagem volta sem zoom', async () => {
  const { root, onClose, onStep } = await render();
  deliver('PinchGestureHandler', 'onStart', event(0, 0, 1, 2));
  deliver('PinchGestureHandler', 'onUpdate', event(0, 0, 2, 2));
  deliver('PinchGestureHandler', 'onEnd');
  drag(100, 20);
  const props = { uri: 'https://example.test/1.png', filename: '1.png', onClose, onStep };
  await act(async () => root.render(createElement(Lightbox, { ...props, visible: false })));
  await act(async () => root.render(createElement(Lightbox, { ...props, visible: true })));
  expect(native.style().transform).toEqual([{ translateX: 0 }, { translateY: 0 }, { scale: 1 }]);
});

it('vídeo também tem o botão de salvar, e ele salva o vídeo aberto', async () => {
  const { saveImage } = await import('./mediaCache');
  vi.mocked(saveImage).mockResolvedValue(false);
  const { container } = await render({ kind: 'video', uri: 'https://example.test/demo.mp4', filename: 'demo.mp4' });
  const salvar = container.querySelector<HTMLButtonElement>('[aria-label="salvar"]');
  expect(salvar).not.toBeNull();
  await act(async () => salvar!.click());
  expect(saveImage).toHaveBeenCalledWith('https://example.test/demo.mp4', undefined, 'demo.mp4');
});
