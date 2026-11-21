import { create } from 'zustand';
import * as m from '../paraglide/messages';
import { toast } from '../ui/Toast';

// "Na conversa" vindo de outra tela (Mídia e arquivos): a tela volta, e a lista da conversa que
// estava embaixo rola até a mensagem. Pedido velho não vale: a lista que monta depois não pula sozinha.
interface ChatJumpState {
  request: { route: string; eventId: string; n: number; at: number } | null;
}

export const useChatJump = create<ChatJumpState>(() => ({ request: null }));

export const JUMP_TTL_MS = 2000;
let seq = 0;

export const chatJumpRoute = (serverId: string, sessionName: string) => `${serverId}::${sessionName}`;

export function requestChatJump(route: string, eventId: string): void {
  const n = ++seq;
  useChatJump.setState({ request: { route, eventId, n, at: Date.now() } });
  // Nenhuma lista da sessão achou a mensagem (fora do histórico carregado): avisa em vez de sumir calado.
  setTimeout(() => {
    if (useChatJump.getState().request?.n !== n) return;
    useChatJump.setState({ request: null });
    toast.erro(m.gallery_jump_not_found());
  }, JUMP_TTL_MS);
}

export function consumeChatJump(n: number): void {
  if (useChatJump.getState().request?.n === n) useChatJump.setState({ request: null });
}
