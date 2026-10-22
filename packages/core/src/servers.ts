// `disabled`: guardado neste aparelho (com o token) mas fora da lista de sessões.
// `invite`: existe porque alguém compartilhou UMA sessão com este aparelho; o token é de convidado.
// `inviteEnded`: o dono revogou ou a sessão acabou (410); a entrada fica até a pessoa remover.
// `lan`: endereço na rede local e o nome que a máquina respondeu por ele (`url` vazio = sem acesso
// local). Aprendido sozinho; ausente = ainda não perguntado.
export interface LanInfo { url: string; id: string }
export interface Server {
  id: string; label: string; baseUrl: string; token: string;
  disabled?: boolean; invite?: boolean; inviteEnded?: boolean;
  lan?: LanInfo;
  /** Alternativa Tailscale aprendida e confirmada como a mesma máquina. */
  remote?: LanInfo;
  // Saiu da lista com um chat dele aberto: toda chamada é recusada, nunca vai ao ativo.
  removed?: boolean;
  // Entrada montada só para um par externo (`reconcilePairEntries`): não é guardada nem vira seção.
  ephemeral?: boolean;
}
export const SERVER_COLORS = ['#7c6af7', '#3ba55d', '#e0a23b', '#e0563b', '#3b9fe0', '#c43be0'];
export function serverColor(id: string): string {
  let h = 0;
  for (let i = 0; i < id.length; i++) h = (h * 31 + id.charCodeAt(i)) >>> 0;
  return SERVER_COLORS[h % SERVER_COLORS.length];
}
