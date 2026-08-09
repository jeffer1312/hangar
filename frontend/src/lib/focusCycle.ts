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
      !('disabled' in element && element.disabled) &&
      !element.matches?.(':disabled') &&
      !element.closest('[inert], [aria-hidden="true"]') &&
      element.getClientRects().length > 0,
  );
}

// Alvo de restauração de foco SEGURO. Um elemento CONECTADO mas oculto (drawer fechado, pai
// `aria-hidden`, `inert`) ou desabilitado não é restaurável — focá-lo é no-op ou pior (leitor de
// tela aponta pro vazio). Medido na round 3: o ConfirmDialog capturava o gatilho no mount, mas o
// AccountMenu fechava ANTES do diálogo e o "restore" ia pra um elemento fora da a11y tree.
export function isRestorableFocusTarget(el: HTMLElement | null): boolean {
  if (!el || !el.isConnected) return false;
  if (el.hasAttribute('disabled') || ('disabled' in el && el.disabled)) return false;
  if (el.getClientRects().length === 0) return false;
  if (el.closest('[inert], [aria-hidden="true"]')) return false;
  return true;
}

// Restaura o foco pro PRIMEIRO alvo restaurável; se o primário não presta (sumiu/oculto/inerte),
// cai no `fallback` explícito (quem o overlay sabe que está sempre acessível — engrenagem, hamburger,
// botão fechar). Nenhum alvo válido = não faz nada (evita o foco caindo no <body> às cegas).
export function restoreFocus(primary: HTMLElement | null, fallback?: HTMLElement | null): void {
  const target = isRestorableFocusTarget(primary) ? primary : fallback ?? null;
  if (isRestorableFocusTarget(target)) target!.focus();
}

// Foca o primeiro campo inválido (`aria-invalid="true"`) dentro do container — o fluxo de
// pareamento associa o erro ao campo via `aria-describedby` e move o foco pra onde corrigir.
// `null` (ref ainda não montada) é no-op.
export function focusFirstInvalid(container: ParentNode | null): void {
  if (!container) return;
  const el = container.querySelector(
    'input[aria-invalid="true"], select[aria-invalid="true"], textarea[aria-invalid="true"]',
  ) as HTMLElement | null;
  if (isRestorableFocusTarget(el)) el!.focus();
}
