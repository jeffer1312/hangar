import { useEffect, useState } from 'react';
import { View } from 'react-native';
import { getConfigForServer, patchConfigForServer, type ConfigServidor, type Server } from '@hangar/core';
import { SectionCard } from '../config/SectionCard';
import { SettingsRow } from '../config/SettingsRow';
import { InfoNotice } from '../config/InfoNotice';
import { Pill } from '../config/PageHeader';
import { ensureActive, readIdentifier, readReach, saveIdentifier, validIdentifier, type MachineId, type Reach, type Warning } from './api';
import { Field, Notice, styles, useMachineAction } from './parts';
import { useServers } from '../../stores/servers';
import * as m from '../../paraglide/messages';

export function ServerSettings({ server }: { server: Server }) {
  const [identity, setIdentity] = useState<MachineId | null>(null);
  const [identifier, setIdentifier] = useState('');
  const [warnings, setWarnings] = useState<Warning[]>([]);
  const idRead = useMachineAction();
  const idSave = useMachineAction();
  const [config, setConfig] = useState<ConfigServidor | null>(null);
  const [origins, setOrigins] = useState('');
  const configRead = useMachineAction();
  const configSave = useMachineAction();
  const [reach, setReach] = useState<Reach | null>(null);
  const reachRead = useMachineAction();
  const loadIdentity = () => idRead.run((signal) => readIdentifier(server, signal), (value) => { setIdentity(value); setIdentifier(value.identificador); });
  const loadConfig = () => configRead.run(async (signal) => {
    const value = await getConfigForServer(server, 15_000);
    ensureActive(signal);
    if (!value || !value.campos || typeof value.campos !== 'object' || (value.campos.term_origins && typeof value.campos.term_origins.valor !== 'string')) {
      throw new Error(m.mobile_machines_invalid_response());
    }
    return value;
  }, (value) => { setConfig(value); setOrigins(String(value.campos.term_origins?.valor ?? '')); });
  const loadReach = () => reachRead.run((signal) => readReach(server, signal), setReach);
  useEffect(() => { void loadIdentity(); void loadConfig(); void loadReach(); }, [server.id, server.baseUrl, server.token]);

  return <>
    <SectionCard icon="Server" title={m.mobile_machines_identifier()}>
      <View style={styles.content}>
        {idRead.busy ? <InfoNotice text={m.native_server_loading()} /> : null}
        <Field label={m.mobile_machines_identifier()} value={identifier} onChangeText={setIdentifier} editable={!!identity && !idRead.busy && !idSave.busy} />
        <InfoNotice text={m.mobile_machines_identifier_hint()} />
        <Notice text={idRead.error || idSave.error} error /><Notice text={idSave.message} />
        {warnings.map((warning, index) => <Notice key={`${warning.id}:${index}`} text={`${warning.id}: ${warning.estado}${warning.motivo ? ` · ${warning.motivo}` : ''}`} />)}
        <View style={styles.actions}>
          {idRead.error ? <Pill label={m.mobile_machines_retry()} onPress={loadIdentity} disabled={idRead.busy} /> : null}
          <Pill label={idSave.busy ? m.mobile_machines_busy() : m.mobile_machines_save_identifier()} disabled={!identity || idRead.busy || idSave.busy || !validIdentifier(identifier)}
            onPress={() => idSave.run((signal) => saveIdentifier(server, identifier, signal), (value) => {
              useServers.getState().rememberLan(server.id, { url: identity!.lan_url, id: value.identificador }, server);
              setIdentity({ ...identity!, identificador: value.identificador }); setIdentifier(value.identificador); setWarnings(value.avisos);
            }, m.mobile_machines_saved())} />
        </View>
      </View>
    </SectionCard>
    <SectionCard icon="Terminal" title={m.mobile_machines_server()}>
      {configRead.busy ? <InfoNotice text={m.native_server_loading()} /> : null}
      {configRead.error ? <View style={styles.content}><Notice text={configRead.error} error /><Pill label={m.mobile_machines_retry()} onPress={loadConfig} disabled={configRead.busy} /></View> : null}
      {config ? <>
        <SettingsRow title={m.mobile_machines_bind()} description={String(config.somente_leitura?.lan_bind_ip ?? '—')} />
        <SettingsRow title={m.mobile_machines_port()} description={String(config.somente_leitura?.port ?? '—')} />
        <SettingsRow title={m.mobile_machines_public_url()} description={String(config.somente_leitura?.public_url ?? '—')} />
        {config.campos.term_origins ? <View style={styles.content}>
          <Field label={m.mobile_machines_term_origins()} value={origins} onChangeText={setOrigins} editable={!configSave.busy} />
          <Notice text={configSave.error} error /><Notice text={configSave.message} />
          <View style={styles.actions}><Pill label={configSave.busy ? m.mobile_machines_busy() : m.mobile_machines_save_origins()} disabled={configSave.busy}
            onPress={() => configSave.run(async (signal) => {
              const value = await patchConfigForServer(server, { term_origins: origins });
              ensureActive(signal);
              const field = value?.campos?.term_origins;
              if (!field || typeof field.valor !== 'string') throw new Error(m.mobile_machines_invalid_response());
              if (field.erro) throw new Error(field.erro);
              return value;
            }, (value) => { setConfig({ ...config, ...value }); setOrigins(String(value.campos.term_origins.valor)); }, m.mobile_machines_saved())} /></View>
        </View> : null}
      </> : null}
    </SectionCard>
    <SectionCard icon="Wifi" title={m.mobile_machines_reach()}>
      <View style={styles.content}>
        {reachRead.busy ? <InfoNotice text={m.native_server_loading()} /> : null}
        <Notice text={reachRead.error} error />
        {reach ? <>
          <InfoNotice text={m.mobile_machines_loopback({ value: reach.loopback ? m.mobile_machines_yes() : m.mobile_machines_no() })} />
          <InfoNotice text={`${m.mobile_machines_bind()}: ${reach.bind}`} />
          {!reach.enderecos.length ? <InfoNotice text={m.mobile_machines_reach_empty()} /> : null}
          {reach.enderecos.map((address, index) => <SettingsRow key={`${address.tipo}:${index}`} title={address.tipo} description={address.url || '—'}>
            <Notice text={`${address.estado}${address.tempo_ms === null ? '' : ` · ${address.tempo_ms} ms`}${address.motivo ? ` · ${address.motivo}` : ''}`} error={address.estado === 'falhou'} />
          </SettingsRow>)}
        </> : null}
        <View style={styles.actions}><Pill label={m.mobile_machines_check()} onPress={loadReach} disabled={reachRead.busy} /></View>
      </View>
    </SectionCard>
  </>;
}
