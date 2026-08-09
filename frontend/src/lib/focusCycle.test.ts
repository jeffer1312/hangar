import { describe, expect, it, vi } from 'vitest';
import { focusableElements, nextFocusIndex, isRestorableFocusTarget, restoreFocus, focusFirstInvalid } from './focusCycle';

describe('nextFocusIndex', () => {
  it('moves from the first and last items', () => {
    expect(nextFocusIndex(0, 3, 1)).toBe(1);
    expect(nextFocusIndex(2, 3, -1)).toBe(1);
  });

  it('wraps Tab from the last item to the first', () => {
    expect(nextFocusIndex(2, 3, 1)).toBe(0);
  });

  it('wraps Shift+Tab from the first item to the last', () => {
    expect(nextFocusIndex(0, 3, -1)).toBe(2);
  });

  it('returns -1 for an empty list', () => {
    expect(nextFocusIndex(0, 0, 1)).toBe(-1);
  });
});

type ElementOptions = {
  disabled?: boolean;
  hiddenFromAccessibilityTree?: boolean;
  invisible?: boolean;
  tabIndex?: number;
  connected?: boolean;
};

function elementStub(options: ElementOptions = {}): HTMLElement {
  return {
    tabIndex: options.tabIndex ?? 0,
    isConnected: options.connected ?? true,
    disabled: Boolean(options.disabled),
    focus: vi.fn(),
    getClientRects: () => ({ length: options.invisible ? 0 : 1 }),
    hasAttribute: (name: string) => name === 'disabled' && Boolean(options.disabled),
    closest: (selector: string) => {
      if (
        selector === '[inert], [aria-hidden="true"]' &&
        options.hiddenFromAccessibilityTree
      ) {
        return {};
      }
      return null;
    },
  } as unknown as HTMLElement;
}

describe('focusableElements', () => {
  it('uses an explicit selector and keeps visible, enabled candidates', () => {
    const visibleButton = elementStub();
    let selector = '';
    const container = {
      querySelectorAll: (value: string) => {
        selector = value;
        return [visibleButton];
      },
    } as unknown as HTMLElement;

    expect(focusableElements(container)).toEqual([visibleButton]);
    expect(selector).toBe('a[href], button, input, select, textarea, [tabindex]');
  });

  it('excludes disabled, inert, aria-hidden, invisible and negative-tabindex items', () => {
    const visibleButton = elementStub();
    const container = {
      querySelectorAll: () => [
        elementStub({ disabled: true }),
        elementStub({ hiddenFromAccessibilityTree: true }),
        elementStub({ invisible: true }),
        elementStub({ tabIndex: -1 }),
        visibleButton,
      ],
    } as unknown as HTMLElement;

    expect(focusableElements(container)).toEqual([visibleButton]);
  });
});

// Round 4: restauração de foco SEGURA — um alvo "conectado" mas oculto/inerte/desabilitado não
// presta (o AccountMenu fechava antes do diálogo e o restore ia pra fora da árvore acessível).
describe('isRestorableFocusTarget', () => {
  it('aceita alvo conectado, visível, habilitado e fora de inert/aria-hidden', () => {
    expect(isRestorableFocusTarget(elementStub())).toBe(true);
  });

  it('recusa null, desconectado, invisível, disabled e inert/aria-hidden', () => {
    expect(isRestorableFocusTarget(null)).toBe(false);
    expect(isRestorableFocusTarget(elementStub({ connected: false }))).toBe(false);
    expect(isRestorableFocusTarget(elementStub({ invisible: true }))).toBe(false);
    expect(isRestorableFocusTarget(elementStub({ disabled: true }))).toBe(false);
    expect(isRestorableFocusTarget(elementStub({ hiddenFromAccessibilityTree: true }))).toBe(false);
  });
});

describe('restoreFocus', () => {
  it('primário válido recebe o foco', () => {
    const prim = elementStub();
    restoreFocus(prim);
    expect(prim.focus).toHaveBeenCalledTimes(1);
  });

  it('primário inválido (oculto) cai no fallback explícito', () => {
    const prim = elementStub({ hiddenFromAccessibilityTree: true });
    const fb = elementStub();
    restoreFocus(prim, fb);
    expect(prim.focus).not.toHaveBeenCalled();
    expect(fb.focus).toHaveBeenCalledTimes(1);
  });

  it('nenhum alvo válido não faz nada (foco não cai no body às cegas)', () => {
    const prim = elementStub({ connected: false });
    const fb = elementStub({ connected: false });
    expect(() => restoreFocus(prim, fb)).not.toThrow();
  });
});

describe('focusFirstInvalid', () => {
  it('foca o primeiro campo com aria-invalid=true', () => {
    const campo = elementStub();
    const container = { querySelector: () => campo } as unknown as ParentNode;
    focusFirstInvalid(container);
    expect(campo.focus).toHaveBeenCalledTimes(1);
  });

  it('sem campo inválido não faz nada', () => {
    const container = { querySelector: () => null } as unknown as ParentNode;
    expect(() => focusFirstInvalid(container)).not.toThrow();
  });
});
