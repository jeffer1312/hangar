import { useState, useSyncExternalStore } from 'react';
import { Pressable, Text, View } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import { ownUpdatableServers, updateAll } from '@hangar/core';
import type { Server, UpdateAllResult, UpdateAllState } from '@hangar/core';
import { SectionCard } from '../config/SectionCard';
import { SettingsRow } from '../config/SettingsRow';
import { useSettingsColors } from '../config/colors';
import { MachineSheet, Notice } from './parts';
import { MachineRow } from './MachineRows';
import { haptic } from '../../ui/haptics';
import * as m from '../../paraglide/messages';

function statusText(value: UpdateAllState): string {
  switch (value.status) {
    case 'consultando': return m.update_all_checking();
    case 'iniciando': return m.update_all_starting();
    case 'rodando': return m.update_all_running({ texto: value.state?.texto ?? '', passo: String(value.state?.passo ?? 0), total: String(value.state?.total ?? 0) });
    case 'esperando': return m.update_all_waiting();
    case 'em_dia': return m.update_all_up_to_date();
    case 'travado':
      switch (value.reason) {
        case 'branch': return m.update_all_held_branch();
        case 'mudancas': return m.update_all_held_changes();
        case 'rodando': return m.update_all_held_running();
        default: return m.update_all_held_missing();
      }
    case 'fora_do_ar': return m.update_all_offline();
    case 'atualizado': return m.update_all_updated();
    case 'atualizado_manual': return m.update_all_updated_manual();
    case 'falhou': return m.update_all_failed({ erro: value.errorCode === 'silent' ? m.update_all_silent()
      : value.errorCode === 'timeout' ? m.update_all_timeout() : value.error || m.erro_desconhecido() });
  }
}

const needsAttention = (value: UpdateAllState) => ['falhou', 'fora_do_ar', 'travado'].includes(value.status);

// A rodada vive fora do componente: sair da tela não a para, e voltar mostra a mesma em vez de abrir outra.
type Round = { busy: boolean; count: number; rows: UpdateAllResult[]; error: string };
let round: Round = { busy: false, count: 0, rows: [], error: '' };
const listeners = new Set<() => void>();
const setRound = (patch: Partial<Round>) => { round = { ...round, ...patch }; listeners.forEach((listener) => listener()); };
const subscribe = (listener: () => void) => { listeners.add(listener); return () => { listeners.delete(listener); }; };

export function UpdateAllMachines({ servers, compact = false }: { servers: readonly Server[]; compact?: boolean }) {
  const c = useSettingsColors();
  const candidates = ownUpdatableServers(servers);
  const [open, setOpen] = useState(false);
  const { busy, count, rows, error } = useSyncExternalStore(subscribe, () => round);

  const start = async () => {
    setOpen(true);
    if (round.busy) return;
    setRound({ busy: true, count: candidates.length, rows: [], error: '' });
    try {
      const results = await updateAll(candidates, (id, state) => {
        const server = candidates.find((entry) => entry.id === id);
        if (!server) return;
        // O core só publica representantes depois de tirar endereços da mesma máquina.
        setRound({ rows: round.rows.some((row) => row.server.id === id)
          ? round.rows.map((row) => row.server.id === id ? { server, state } : row)
          : [...round.rows, { server, state }] });
      });
      setRound({ rows: results });
      if (!results.length || results.some((row) => needsAttention(row.state))) haptic.error();
      else haptic.success();
    } catch (cause) {
      setRound({ error: m.update_all_failed({ erro: cause instanceof Error ? cause.message : String(cause) }) });
      haptic.error();
    } finally {
      setRound({ busy: false });
    }
  };

  if (candidates.length < 2 && !busy && !open) return null;
  const buttonLabel = m.update_all_button({ n: String(busy ? count : candidates.length) });
  return <>
    {candidates.length >= 2 || busy ? compact ? <MachineRow title={buttonLabel} onPress={() => void start()} /> : <Pressable accessibilityRole="button" accessibilityLabel={buttonLabel}
      onPress={() => void start()} style={({ pressed }) => [styles.button, { borderColor: c.borderStrong }, pressed && { backgroundColor: c.hover }]}>
      <Text style={[styles.label, { color: c.text }]}>{buttonLabel}</Text>
    </Pressable> : null}
    {open ? <MachineSheet title={m.update_all_title()} onClose={() => setOpen(false)}>
      {busy && rows.length === 0 ? <Notice text={m.update_all_checking()} /> : null}
      <Notice text={error} error />
      {!busy && !error && rows.length === 0 ? <Notice text={m.maquinas_vazio()} /> : null}
      {rows.length ? <SectionCard>
        {rows.map(({ server, state }) => <SettingsRow key={server.id} title={server.label} description={server.baseUrl}>
          <Notice text={statusText(state)} error={needsAttention(state)} />
          {state.status === 'travado' && state.reason === 'falta' && state.preflight?.faltando.length
            ? <Notice text={state.preflight.faltando.join(', ')} /> : null}
          {state.warnings.length ? <View style={styles.warnings}>
            <Notice text={m.update_all_warnings()} />
            {state.warnings.map((warning, index) => <Notice key={index} text={warning} />)}
          </View> : null}
        </SettingsRow>)}
      </SectionCard> : null}
    </MachineSheet> : null}
  </>;
}

const styles = StyleSheet.create((theme) => ({
  button: { minHeight: 44, justifyContent: 'center', paddingHorizontal: theme.base.space[3], paddingVertical: theme.base.space[2], borderWidth: 1, borderRadius: theme.base.radius.md },
  label: { fontSize: theme.base.text.sm, fontWeight: theme.base.weight.medium },
  warnings: { gap: theme.base.space[2], marginTop: theme.base.space[2] },
}));
