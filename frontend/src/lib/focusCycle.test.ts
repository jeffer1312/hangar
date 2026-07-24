import { describe, expect, it } from 'vitest';
import { focusableElements, nextFocusIndex } from './focusCycle';

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
};

function elementStub(options: ElementOptions = {}): HTMLElement {
  return {
    tabIndex: options.tabIndex ?? 0,
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
