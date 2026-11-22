import type { ConnectMachine, GuestAccount, GuestFailure, GuestMachine, Server } from '@hangar/core';
import { sameMachine } from '../connect/connectStore';
import { ownServers } from '../../stores/servers';
import { DEMO_BASE_URL } from '../../net/demo';
import * as m from '../../paraglide/messages';

/** O servidor diz quando o Connect recusou o nome (`name_taken`); a leitura do core ainda não o traz. */
export type ListedGuest = GuestAccount & { nameTaken?: boolean };

/** Máquinas que podem receber convidado, cada uma com o host do Connect que a conta conhece (vai ao cofre dela). */
export function guestMachines(servers: Server[], account: ConnectMachine[] | null): GuestMachine[] {
  return ownServers(servers).filter((server) => !server.disabled && !server.removed && !server.ephemeral && server.baseUrl !== DEMO_BASE_URL)
    .map((server) => {
      const machine = account?.find((candidate) => sameMachine(server, candidate));
      const connectUrl = machine ? `https://${machine.host}` : server.connect?.url;
      return connectUrl ? { server, connectUrl } : { server };
    });
}

/** Uma linha por máquina que falhou; `labelOf` dá o nome de uma máquina que só veio pelo id. */
export function failureText(failure: GuestFailure, labelOf: (id: string) => string): string {
  if (failure.code === 'guest_server_missing') {
    return m.convidados_falha_servidor({ servidor: labelOf(failure.label), erro: m.convidados_servidor_fora_da_lista() });
  }
  if (failure.code === 'guest_store_failed') {
    return failure.status === 409 ? m.convidados_usuario_em_uso() : m.guests_store_failed({ servidor: failure.label });
  }
  return m.convidados_falha_servidor({ servidor: failure.label, erro: failure.message });
}

/** Recusa antes de tocar nas máquinas (nome ou senha) ou erro inesperado. */
export function thrownText(error: unknown): string {
  const code = (error as { code?: string } | null)?.code;
  if (code === 'guest_user_invalid') return m.guests_user_invalid();
  if (code === 'guest_password_required') return m.guests_password_required();
  return error instanceof Error && error.message ? error.message : String(error);
}

export function guestSummary(guest: GuestAccount): string {
  const servers = m.convidados_resumo({ servidores: String(guest.servers.length) });
  return `${servers} · ${guest.seesOwner ? m.guests_sees_mine() : m.guests_not_sees_mine()}`;
}
