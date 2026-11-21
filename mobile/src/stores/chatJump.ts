import { create } from 'zustand';

// "Na conversa" vindo de outra tela (Mídia e arquivos): a tela volta, e a lista da conversa que
// estava embaixo rola até a mensagem. Pedido velho não vale: a lista que monta depois não pula sozinha.
interface ChatJumpState {
  request: { route: string; eventId: string; n: number; at: number } | null;
}

export const useChatJump = create<ChatJumpState>(() => ({ request: null }));

export const JUMP_TTL_MS = 5000;
let seq = 0;

export const chatJumpRoute = (serverId: string, sessionName: string) => `${serverId}::${sessionName}`;

export function requestChatJump(route: string, eventId: string): void {
  useChatJump.setState({ request: { route, eventId, n: ++seq, at: Date.now() } });
}

export function consumeChatJump(n: number): void {
  if (useChatJump.getState().request?.n === n) useChatJump.setState({ request: null });
}
