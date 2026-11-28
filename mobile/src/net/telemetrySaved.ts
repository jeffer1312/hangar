import type { Server, TelemetrySetting } from '@hangar/core';

// Fora do diag.ts: quem grava o aceite não pode puxar o expo-updates que o diag importa.
type Listener = (server: Server, state: TelemetrySetting) => void;
let listener: Listener | undefined;

export function setTelemetrySavedListener(next: Listener | undefined): void {
  listener = next;
}

export function onTelemetrySaved(server: Server, state: TelemetrySetting): void {
  listener?.(server, state);
}
