import { writable } from 'svelte/store';
import type { SessionInfo, StateEvent } from './types';

// Svelte 5 stores using writable (compatible with Svelte 5)
export const sessions = writable<SessionInfo[]>([]);
export const activeSessionName = writable<string | null>(null);
export const sessionState = writable<StateEvent | null>(null);
