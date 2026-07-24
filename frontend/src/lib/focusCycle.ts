const FOCUSABLE_SELECTOR = 'a[href], button, input, select, textarea, [tabindex]';

export function nextFocusIndex(
  currentIndex: number,
  itemCount: number,
  direction: number,
): number {
  if (itemCount <= 0) return -1;
  return ((currentIndex + direction) % itemCount + itemCount) % itemCount;
}

export function focusableElements(container: HTMLElement): HTMLElement[] {
  return Array.from(container.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR)).filter(
    (element) =>
      element.tabIndex >= 0 &&
      !element.hasAttribute('disabled') &&
      !element.closest('[inert], [aria-hidden="true"]') &&
      element.getClientRects().length > 0,
  );
}
