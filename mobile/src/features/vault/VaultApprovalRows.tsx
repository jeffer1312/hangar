import type { ReactNode } from 'react';
import { View } from 'react-native';
import type { VaultRequest } from '@hangar/core';
import { MachineGroup, MachineRow } from '../machines/MachineRows';
import { useSettingsColors } from '../config/colors';
import * as m from '../../paraglide/messages';

const USE: Record<string, () => string> = { fill: m.vault_app_use_fill, exec: m.vault_app_use_exec, file: m.vault_app_use_file };

/** Cartão de aprovação de uso. `allowPressed`: só a animação da novidade, que mostra o toque em Permitir. */
export function VaultApprovalRows({ request, busy, notice, allowPressed, onAllow, onDeny }: {
  request: Pick<VaultRequest, 'entry_name' | 'origin' | 'use' | 'target' | 'reason'>;
  busy?: boolean;
  notice?: ReactNode;
  allowPressed?: boolean;
  onAllow: () => void;
  onDeny: () => void;
}) {
  const c = useSettingsColors();
  return <>
    <MachineGroup>
      <MachineRow title={m.vault_app_request_entry()} value={request.entry_name ?? '?'} />
      <MachineRow title={m.vault_app_request_origin()} value={request.origin} />
      {request.use ? <MachineRow title={m.vault_app_request_use()} value={USE[request.use]?.() ?? request.use} /> : null}
      {request.target ? <MachineRow title={m.vault_app_request_target()} description={request.target} /> : null}
      {request.reason ? <MachineRow title={m.vault_app_request_reason()} description={request.reason} /> : null}
    </MachineGroup>
    {notice}
    <MachineGroup>
      <View style={allowPressed ? { backgroundColor: c.accentDim } : undefined}>
        <MachineRow title={m.vault_app_allow()} action disabled={busy} selected={allowPressed} onPress={onAllow} />
      </View>
      <MachineRow title={m.vault_app_deny()} danger disabled={busy} onPress={onDeny} />
    </MachineGroup>
  </>;
}
