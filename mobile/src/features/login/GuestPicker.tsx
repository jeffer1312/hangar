import { useState } from 'react';
import { Pressable, Text, View } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import type { Server } from '@hangar/core';
import { CheckRow } from '../config/CheckRow';
import { SectionCard } from '../config/SectionCard';
import { useSettingsColors } from '../config/colors';
import { MachineHelp, MachineRow } from '../machines/MachineRows';
import { LoginSubmit } from './LoginFields';
import * as m from '../../paraglide/messages';

// O cofre da convidada guarda o host do Connect de cada máquina (`<máquina>.<conta>.hangar.dev.br`): a conta é o dono.
function ownerOf(url: string): string {
  try {
    const labels = new URL(url).hostname.split('.');
    return labels.length === 5 && labels.slice(-3).join('.') === 'hangar.dev.br' ? labels[1] : '';
  } catch {
    return '';
  }
}

/** Convidada entrou: escolhe quais das máquinas liberadas entram neste aparelho. */
export function GuestPicker({ user, servers, onEnter, onLeave }: {
  user: string; servers: Server[]; onEnter: (picked: Server[]) => void; onLeave: () => void;
}) {
  const c = useSettingsColors();
  const [picked, setPicked] = useState(() => new Set(servers.map((server) => server.id)));
  const toggle = (id: string) => setPicked((before) => {
    const next = new Set(before);
    if (!next.delete(id)) next.add(id);
    return next;
  });
  const chosen = servers.filter((server) => picked.has(server.id));
  const label = chosen.length === 1 ? m.login_guest_enter_one() : m.login_guest_enter_many({ n: String(chosen.length) });
  return <View style={styles.wrap}>
    <View style={styles.top}>
      <View style={[styles.tag, { backgroundColor: c.accentDim }]}><Text style={[styles.tagText, { color: c.accentText }]}>{m.login_guest_tag()}</Text></View>
      <Text numberOfLines={1} style={[styles.user, { color: c.text }]}>{user}</Text>
      <Pressable accessibilityRole="button" accessibilityLabel={m.contas_sair()} hitSlop={8} onPress={onLeave} style={styles.leave}>
        <Text style={[styles.leaveText, { color: c.accent }]}>{m.contas_sair()}</Text>
      </Pressable>
    </View>
    <SectionCard icon="Laptop" title={m.login_guest_machines()}>
      {servers.length === 0 ? <MachineRow title={m.mobile_sync_login_empty()} /> : null}
      {servers.map((server) => {
        const owner = ownerOf(server.baseUrl);
        return <CheckRow key={server.id} title={server.label} description={owner ? m.login_guest_owner({ owner }) : undefined}
          checked={picked.has(server.id)} onToggle={() => toggle(server.id)} />;
      })}
    </SectionCard>
    {servers.length ? <LoginSubmit label={label} blocked={!chosen.length} onPress={() => onEnter(chosen)} /> : null}
    <MachineHelp>{m.login_guest_hint()}</MachineHelp>
  </View>;
}

const styles = StyleSheet.create((theme) => ({
  wrap: { gap: theme.base.space[4] },
  top: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2] },
  tag: { borderRadius: theme.base.radius.full, paddingHorizontal: theme.base.space[2], paddingVertical: theme.base.space[0.5] },
  tagText: { ...theme.type.badge },
  user: { flex: 1, minWidth: 0, ...theme.type.label },
  leave: { minHeight: 44, minWidth: 44, justifyContent: 'center', alignItems: 'flex-end' },
  leaveText: { ...theme.type.control },
}));
