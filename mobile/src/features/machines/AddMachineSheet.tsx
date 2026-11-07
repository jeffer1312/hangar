import { useState } from 'react';
import { Pressable, Text, View } from 'react-native';
import { getConfigForServer, type Server } from '@hangar/core';
import { useServers } from '../../stores/servers';
import { Icon } from '../../ui/Icon';
import { SectionCard } from '../config/SectionCard';
import { InfoNotice } from '../config/InfoNotice';
import { Pill } from '../config/PageHeader';
import { useSettingsColors } from '../config/colors';
import { discover, ensureActive, normalizeUrl, readIdentifier, reason, registerBoth, tailscaleFailure, type Directions, type Discovered } from './api';
import { DirectionsNotice, Field, MachineSheet, Notice, styles, useMachineAction } from './parts';
import { discoverSavedMachines } from './importMachines';
import * as m from '../../paraglide/messages';

type Added = { warnings: string[]; directions?: Directions; registered: boolean };
export function AddMachineSheet({ owner, onClose, onQr }: { owner: Server | null; onClose: () => void; onQr: () => void }) {
  const c = useSettingsColors();
  const [label, setLabel] = useState('');
  const [address, setAddress] = useState('');
  const [token, setToken] = useState('');
  const [both, setBoth] = useState(false);
  const [discovered, setDiscovered] = useState<Discovered[] | null>(null);
  const [added, setAdded] = useState<Added | null>(null);
  const action = useMachineAction();
  const off = action.busy || !!added;
  const save = () => action.run(async (signal) => {
    const baseUrl = normalizeUrl(address);
    const credential = token.trim();
    if (!credential) throw new Error(m.mobile_machines_token_required());
    const candidate = { id: `machine-probe:${baseUrl}`, baseUrl, token: credential, label: label.trim() || m.native_settings_group_server() };
    const config = await getConfigForServer(candidate, 15_000).catch((error: unknown) => {
      const hint = tailscaleFailure(error, baseUrl);
      throw hint ? new Error(hint) : error;
    });
    ensureActive(signal);
    if (!config || !config.campos || typeof config.campos !== 'object') throw new Error(m.mobile_machines_invalid_response());
    const warnings: string[] = [];
    let identifier = '';
    try {
      identifier = (await readIdentifier(candidate, signal)).identificador;
      if (!identifier && !label.trim()) throw new Error(m.mobile_machines_invalid_response());
    } catch (error) { ensureActive(signal); warnings.push(reason(error)); }
    ensureActive(signal);
    const typedLabel = label.trim();
    const saved = useServers.getState().add({ baseUrl, token: credential, label: typedLabel || identifier || m.native_settings_group_server() });
    if (typedLabel && saved.label !== typedLabel) useServers.getState().update(saved.id, { label: typedLabel });
    void discoverSavedMachines();
    const result: Added = { warnings, registered: false };
    if (both && owner) {
      try {
        const registration = await registerBoth(owner, candidate, identifier, signal);
        ensureActive(signal);
        result.directions = registration.directions;
        result.registered = !registration.partial;
        if (registration.partial) warnings.push(m.mobile_machines_partial({ reason: registration.partial }));
      } catch (error) { ensureActive(signal); warnings.push(m.mobile_machines_partial({ reason: reason(error) })); }
    }
    return result;
  }, setAdded, m.mobile_machines_saved_locally());

  return <MachineSheet title={m.mobile_machines_add()} onClose={onClose}>
    <SectionCard icon="Smartphone" title={m.mobile_machines_local()}>
      <View style={styles.content}>
        <Field label={m.mobile_machines_label()} value={label} onChangeText={setLabel} editable={!off} />
        <Field label={m.mobile_machines_endpoint()} value={address} onChangeText={setAddress} editable={!off} keyboardType="url" />
        <Field label={m.mobile_machines_token()} value={token} onChangeText={setToken} editable={!off} secureTextEntry autoComplete="off" />
        {owner ? <>
          <Pressable accessibilityRole="checkbox" accessibilityLabel={m.mobile_machines_register_both()} accessibilityState={{ checked: both, disabled: off }}
            disabled={off} onPress={() => setBoth((value) => !value)} style={[styles.actions, { minHeight: 44 }]}>
            <Icon name={both ? 'SquareCheck' : 'Square'} size={20} color={c.text} />
            <Text style={{ color: c.text, flex: 1 }}>{m.mobile_machines_register_both()}</Text>
          </Pressable>
          <InfoNotice text={m.mobile_machines_register_both_help()} />
        </> : null}
        <Notice text={action.error} error />
        {added?.warnings.map((warning, index) => <Notice key={index} text={warning} error />)}
        <Notice text={action.message} />
        {added?.directions ? <DirectionsNotice value={added.directions} /> : null}
        {added?.registered ? <Notice text={m.mobile_machines_registered()} /> : null}
        <View style={styles.actions}>
          <Pill label={action.busy ? m.mobile_machines_busy() : m.mobile_machines_save()} disabled={off} onPress={save} />
          <Pill label={m.mobile_machines_qr()} onPress={onQr} disabled={action.busy} />
        </View>
      </View>
    </SectionCard>
    <SectionCard icon="Globe" title={m.mobile_machines_discover()}>
      <View style={styles.content}>
        <View style={styles.actions}><Pill label={m.mobile_machines_discover()} disabled={!owner || off}
          onPress={() => { if (owner) void action.run((signal) => discover(owner, signal), setDiscovered); }} /></View>
        {!owner ? <InfoNotice text={m.native_settings_offline()} /> : null}
        {action.busy ? <InfoNotice text={m.native_server_loading()} /> : null}
        {discovered?.length === 0 ? <InfoNotice text={m.mobile_machines_discover_empty()} /> : null}
        {discovered?.[0]?.connect_error ? <InfoNotice text={m.mobile_machines_search_partial({ reason: discovered[0].connect_error })} /> : null}
        {discovered?.map((found) => <Pill key={found.base_url} label={`${found.nome} · ${found.base_url}`} disabled={off}
          onPress={() => setAddress(found.base_url)} />)}
      </View>
    </SectionCard>
  </MachineSheet>;
}
