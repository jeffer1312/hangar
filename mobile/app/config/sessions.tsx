import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { Switch, Text, TextInput, View } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import { Stack, useRouter } from 'expo-router';
import { autoHideDiverges, ownUpdatableServers, readAutoHideAll, saveAutoHideAll, type AutoHideConfig, type AutoHideResult } from '@hangar/core';
import { Pagina } from '../../src/features/config/Pagina';
import { InfoNotice } from '../../src/features/config/InfoNotice';
import { useSettingsColors } from '../../src/features/config/colors';
import { MachineGroup, MachineRow } from '../../src/features/machines/MachineRows';
import { Notice } from '../../src/features/machines/parts';
import { setHiddenSectionOpen } from '../../src/features/sessions/hiddenSectionPref';
import { useServers } from '../../src/stores/servers';
import { useSessions } from '../../src/stores/sessions';
import { haptic } from '../../src/ui/haptics';
import * as m from '../../src/paraglide/messages';

// Arquivo ausente no servidor = desligada, 3 h.
const DEFAULT: AutoHideConfig = { enabled: false, after_minutes: 180 };
const clampHours = (h: number) => Math.min(168, Math.max(1, Math.round(h)));

// Uma configuração para todas as máquinas próprias: grava em todas e mostra o resultado de cada uma.
export default function SessionsSettings() {
  const router = useRouter();
  const c = useSettingsColors();
  const servers = useServers((s) => s.servers);
  const ready = useServers((s) => s.ready);
  const own = useMemo(() => ownUpdatableServers(servers), [servers]);
  const ownKey = own.map((s) => `${s.id}@${s.baseUrl}`).join('|');
  const hiddenCount = useSessions((s) => s.rows.filter((r) => r.hidden_from_list && r.state !== 'dead').length);
  useEffect(() => useSessions.getState().retain(), []);

  const [read, setRead] = useState<AutoHideResult[] | null>(null);
  const [saved, setSaved] = useState<AutoHideResult[] | null>(null);
  const [cfg, setCfg] = useState<AutoHideConfig>(DEFAULT);
  const [draft, setDraft] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  // Só a resposta da última leitura ou gravação vale: outra mais nova a substitui.
  const seq = useRef(0);

  const load = useCallback(() => {
    const n = ++seq.current;
    setRead(null);
    setSaved(null);
    setSaving(false);
    void readAutoHideAll(own).then((results) => {
      if (n !== seq.current) return;
      const diverging = new Set(autoHideDiverges(results));
      setCfg(results.find((r) => r.ok && r.config && !diverging.has(r.serverId))?.config ?? DEFAULT);
      setRead(results);
    });
  }, [own]);
  useEffect(() => { if (own.length) load(); }, [ownKey]);

  const save = (next: AutoHideConfig) => {
    const n = ++seq.current;
    const before = cfg;
    setCfg(next);
    setSaving(true);
    void saveAutoHideAll(own, next).then((results) => {
      if (n !== seq.current) return;
      setSaving(false);
      setSaved(results);
      // Nenhuma gravou: o interruptor e o prazo voltam ao que as máquinas guardam.
      if (results.every((r) => !r.ok)) setCfg(before);
      if (results.every((r) => r.ok)) haptic.success();
      else haptic.error();
    });
  };

  const hours = clampHours(cfg.after_minutes / 60);
  const commitHours = () => {
    const typed = draft === null ? NaN : Number.parseInt(draft, 10);
    setDraft(null);
    if (Number.isNaN(typed) || clampHours(typed) === hours) return;
    save({ ...cfg, after_minutes: clampHours(typed) * 60 });
  };
  const toggle = (enabled: boolean) => {
    haptic.selection();
    save({ ...cfg, enabled });
  };
  const openHidden = () => {
    setHiddenSectionOpen(true);
    router.dismissTo('/?hidden=1' as never);
  };

  const label = (id: string) => own.find((s) => s.id === id)?.label ?? id;
  const failedReads = read?.filter((r) => !r.ok) ?? [];
  const differs = read && !saved ? autoHideDiverges(read) : [];
  // A frase traz o número; a unidade sai dela para ficar ao lado do campo.
  const unit = m.session_auto_hide_hours({ n: '' }).trim();

  let body: ReactNode;
  if (!ready || (own.length > 0 && read === null)) body = <InfoNotice text={m.native_server_loading()} />;
  else if (own.length === 0) body = <InfoNotice text={m.maquinas_vazio()} />;
  else if (read && failedReads.length === read.length) {
    body = <MachineGroup>
      {failedReads.map((r) => <MachineRow key={r.serverId} title={label(r.serverId)} description={r.error} />)}
      <MachineRow title={m.mobile_machines_retry()} action onPress={load} />
    </MachineGroup>;
  } else {
    body = <>
      <MachineGroup>
        <View style={[styles.row, { borderTopColor: c.border }]}>
          <Text style={[styles.label, { color: c.text }]}>{m.session_auto_hide_label()}</Text>
          <TextInput
            value={draft ?? String(hours)}
            onChangeText={setDraft}
            onEndEditing={commitHours}
            keyboardType="number-pad"
            returnKeyType="done"
            maxLength={3}
            selectTextOnFocus
            editable={!saving}
            accessibilityLabel={`${m.session_auto_hide_label()} (${unit})`}
            style={[styles.hours, { color: c.text, borderColor: c.border, backgroundColor: c.inset }]}
          />
          <Text style={[styles.unit, { color: c.muted }]}>{unit}</Text>
          <Switch value={cfg.enabled} disabled={saving} onValueChange={toggle}
            accessibilityLabel={`${m.session_auto_hide_label()} ${m.session_auto_hide_hours({ n: hours })}`} />
        </View>
      </MachineGroup>
      {saving ? <Notice text={m.native_server_saving()} /> : null}
      {saved ? saved.map((r) => <Notice key={r.serverId} error={!r.ok}
        text={r.ok ? m.session_auto_hide_saved_on({ server: label(r.serverId) })
          : m.session_auto_hide_not_saved_on({ server: label(r.serverId), error: r.error ?? '' })} />) : null}
      {!saved ? failedReads.map((r) => <Notice key={r.serverId} error text={`${label(r.serverId)}: ${r.error ?? ''}`} />) : null}
      {differs.length ? <MachineGroup>
        <MachineRow title={m.session_auto_hide_differs({ servers: differs.map(label).join(', ') })} />
        <MachineRow title={m.session_auto_hide_apply_all()} action disabled={saving} onPress={() => save(cfg)} />
      </MachineGroup> : null}
    </>;
  }

  return (
    <Pagina>
      <Stack.Screen options={{ title: m.settings_sessions_title() }} />
      {body}
      <MachineGroup>
        <MachineRow title={m.session_hidden_see({ n: hiddenCount })} disabled={hiddenCount === 0} onPress={openHidden} />
      </MachineGroup>
    </Pagina>
  );
}

const styles = StyleSheet.create((theme) => ({
  row: { minHeight: 56, flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2], paddingHorizontal: theme.base.space[4], paddingVertical: theme.base.space[2], borderTopWidth: 1 },
  label: { flex: 1, minWidth: 0, fontSize: theme.base.text.base },
  hours: { width: 56, minHeight: 44, borderWidth: 1, borderRadius: theme.base.radius.sm, textAlign: 'center', fontSize: theme.base.text.base, fontVariant: ['tabular-nums'] },
  unit: { fontSize: theme.base.text.base },
}));
