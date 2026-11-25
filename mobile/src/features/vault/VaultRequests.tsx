import { useState } from 'react';
import { answerVaultRequest, type Server, type VaultAnswer, type VaultEvent, type VaultPolicy, type VaultRequest } from '@hangar/core';
import { MachineField, MachineGroup, MachineRow } from '../machines/MachineRows';
import { Notice } from '../machines/parts';
import { PolicyRows } from './VaultEntrySheet';
import { VaultApprovalRows } from './VaultApprovalRows';
import { openVaultRequest, useVaultTarget } from './target';
import { vaultErrorText } from './errors';
import { ownerRequests } from '../requests/ownerRequests';
import { toast } from '../../ui/Toast';
import { haptic } from '../../ui/haptics';
import * as m from '../../paraglide/messages';

const vault = ownerRequests<VaultEvent, VaultRequest>({
  select: (s) => s.vault,
  items: (event) => event.requests,
  useTarget: () => {
    const target = useVaultTarget((s) => s.target);
    return target && { serverId: target.serverId, id: target.requestId };
  },
  setTarget: (target) => openVaultRequest(target && { serverId: target.serverId, requestId: target.id }),
  icon: 'KeyRound',
  hint: () => m.vault_app_requests_open(),
  machineTitle: (p) => m.vault_app_requests_title(p),
  sessionTitle: (r) => r.kind === 'entry' ? m.vault_app_session_entry({ label: r.label ?? '' }) : m.vault_app_session_approve({ entry: r.entry_name ?? '' }),
  sheet: {
    title: () => m.vault_app_request_title(), noServer: () => m.vault_app_request_no_server(), loading: () => m.vault_app_loading(),
    gone: () => m.vault_err_pedido_perdido(), next: () => m.vault_app_request_next(),
  },
  storedElsewhere: true,
  Card: RequestCard,
});

/** Aviso por máquina com pedido do cofre aberto (a máquina do aviso é a dona). */
export const VaultRequests = vault.Notices;
/** Botão de chave com a contagem, para barras sem espaço para o aviso (as três formas da barra do tablet). */
export const VaultRequestsButton = vault.CountButton;
/** Pedido feito pela sessão aberta: aparece no chat dela. */
export const SessionVaultRequests = vault.SessionNotices;
/** A folha que responde, na raiz do app: abre da lista, do chat e do push. */
export const VaultRequestSheet = vault.Sheet;

function RequestCard({ server, item: request, onDone }: { server: Server; item: VaultRequest; onDone: () => void }) {
  const [password, setPassword] = useState('');
  const [name, setName] = useState(request.label ?? '');
  const [policy, setPolicy] = useState<VaultPolicy>('ask');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');

  const send = async (answer: VaultAnswer, done: string) => {
    if (busy) return;
    setBusy(true);
    setError('');
    try {
      await answerVaultRequest(server, request.id, answer);
      setPassword('');
      vault.answered.add(request.id);
      toast.ok(done);
      onDone();
    } catch (e) {
      const code = (e as { code?: unknown } | null)?.code;
      // Pedido que outra tela respondeu ou que expirou não volta: o cartão sai e o motivo vai no aviso.
      if (code === 'erro_interacao_desatualizada' || code === 'erro_cofre_pedido_respondido' || code === 'erro_cofre_pedido_perdido') {
        vault.answered.add(request.id);
        toast.erro(vaultErrorText(e));
        onDone();
        return;
      }
      setError(vaultErrorText(e));
      haptic.error();
    } finally { setBusy(false); }
  };
  const iid = request.interaction_id;

  if (request.kind === 'approve') return <VaultApprovalRows request={request} busy={busy} notice={<Notice text={error} error />}
    onAllow={() => void send({ interaction_id: iid, decision: 'allow' }, m.vault_app_allowed())}
    onDeny={() => void send({ interaction_id: iid, decision: 'deny' }, m.vault_app_denied())} />;

  const save = (keep: boolean) => {
    if (!password) return;
    void send({ interaction_id: iid, decision: 'save', password, keep, name: keep ? name.trim() || undefined : undefined, policy: keep ? policy : undefined },
      keep ? m.vault_app_saved() : m.vault_app_sent_once());
  };
  return <>
    <MachineGroup>
      <MachineRow title={request.label ?? m.vault_app_request_entry()} description={request.reason} />
      <MachineRow title={m.vault_app_request_origin()} value={request.origin} />
    </MachineGroup>
    <MachineGroup>
      <MachineField label={m.vault_app_field_password()} value={password} onChangeText={setPassword} editable={!busy}
        secureTextEntry autoComplete="off" textContentType="none" placeholder={request.placeholder} />
      <MachineField label={m.vault_app_field_name()} value={name} onChangeText={setName} editable={!busy} />
    </MachineGroup>
    <MachineGroup>
      <PolicyRows value={policy} onChange={setPolicy} disabled={busy} />
    </MachineGroup>
    <Notice text={error} error />
    <MachineGroup>
      <MachineRow title={m.vault_app_keep()} action disabled={busy || !password} onPress={() => save(true)} />
      <MachineRow title={m.vault_app_once()} disabled={busy || !password} onPress={() => save(false)} />
      <MachineRow title={m.vault_app_decline()} danger disabled={busy} onPress={() => void send({ interaction_id: iid, decision: 'decline' }, m.vault_app_declined())} />
    </MachineGroup>
  </>;
}
