// `disabled`: guardado neste aparelho (com o token) mas fora da lista de sessões.
// `invite`: existe porque alguém compartilhou UMA sessão com este aparelho; o token é de convidado.
// `inviteEnded`: o dono revogou ou a sessão acabou (410); a entrada fica até a pessoa remover.
// `lan`: endereço na rede local e o nome que a máquina respondeu por ele (`url` vazio = sem acesso
// local). Aprendido sozinho; ausente = ainda não perguntado.
// `credential`: `device` é um token por aparelho, que também passa pelo Connect; ausente = senha do dono.
export interface LanInfo { url: string; id: string }
export interface Server {
  id: string; label: string; baseUrl: string; token: string;
  disabled?: boolean; invite?: boolean; inviteEnded?: boolean;
  lan?: LanInfo;
  /** Alternativa Tailscale aprendida e confirmada como a mesma máquina. */
  remote?: LanInfo;
  /** Entrada do Connect provada como a mesma máquina. Só vale com `credential: 'device'`. */
  connect?: LanInfo;
  credential?: 'owner' | 'device';
  deviceId?: string;
  /** Senha do dono trocada pelo token de aparelho: é a volta quando o aparelho recebe 401. */
  ownerToken?: string;
  /** O aparelho foi recusado aqui: não promove sozinho de novo, e a revogação continua valendo. */
  connectOff?: boolean;
  /** Host do Connect que esta entrada usava antes de voltar à senha: é por ele que a conta reconhece a máquina. */
  connectHost?: string;
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
