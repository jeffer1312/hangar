import { useEffect, useState } from 'react';
import { getConfigForServer, patchConfigForServer, type ConfigServidor, type Server } from '@hangar/core';
import { ensureActive, readIdentifier, readReach, saveIdentifier, validIdentifier, type MachineId, type Reach, type Warning } from './api';
import { Notice, useMachineAction } from './parts';
import { MachineField, MachineGroup, MachineHelp, MachineRow } from './MachineRows';
import { MachinePage } from './MachinePage';
import { useServers } from '../../stores/servers';
import * as m from '../../paraglide/messages';

export function ServerSettings({ server, view }: { server: Server; view: 'identity' | 'terminal' | 'diagnostics' }) {
  if (view === 'identity') return <IdentitySettings server={server} />;
  if (view === 'terminal') return <TerminalSettings server={server} />;
  return <NetworkDiagnostics server={server} />;
}

function IdentitySettings({ server }: { server: Server }) {
  const [identity, setIdentity] = useState<MachineId | null>(null);
  const [identifier, setIdentifier] = useState('');
  const [warnings, setWarnings] = useState<Warning[]>([]);
  const read = useMachineAction();
  const save = useMachineAction();
  const load = () => read.run((signal) => readIdentifier(server, signal), (value) => { setIdentity(value); setIdentifier(value.identificador); });
  useEffect(() => { void load(); }, [server.id, server.baseUrl, server.token]);
  const busy = read.busy || save.busy;
  return <MachinePage title={m.mobile_machines_identity_title()} save={{ disabled: !identity || busy || identifier === identity.identificador || !validIdentifier(identifier), busy: save.busy,
    onPress: () => save.run((signal) => saveIdentifier(server, identifier, signal), (value) => {
      useServers.getState().rememberLan(server.id, { url: identity!.lan_url, id: value.identificador }, server);
      setIdentity({ ...identity!, identificador: value.identificador }); setIdentifier(value.identificador); setWarnings(value.avisos);
    }, m.mobile_machines_saved()) }}>
    {read.busy ? <Notice text={m.native_server_loading()} /> : null}
    <MachineGroup><MachineField label={m.mobile_machines_identifier()} value={identifier} onChangeText={setIdentifier} editable={!!identity && !busy} stacked /></MachineGroup>
    <MachineHelp>{m.mobile_machines_identifier_help()}</MachineHelp>
    {!validIdentifier(identifier) ? <Notice text={m.mobile_machines_identifier_hint()} error /> : null}
    <Notice text={read.error || save.error} error /><Notice text={save.message} />
    {read.error ? <MachineGroup><MachineRow title={m.mobile_machines_retry()} action onPress={load} disabled={busy} /></MachineGroup> : null}
    {warnings.map((warning, index) => <Notice key={`${warning.id}:${index}`} text={`${warning.id}: ${warning.estado}${warning.motivo ? ` · ${warning.motivo}` : ''}`} />)}
  </MachinePage>;
}

function useMachineConfig(server: Server) {
  const [config, setConfig] = useState<ConfigServidor | null>(null);
  const read = useMachineAction();
  const load = () => read.run(async (signal) => {
    const value = await getConfigForServer(server, 15_000);
    ensureActive(signal);
    if (!value || !value.campos || typeof value.campos !== 'object' || (value.campos.term_origins && typeof value.campos.term_origins.valor !== 'string')) throw new Error(m.mobile_machines_invalid_response());
    return value;
  }, setConfig);
  useEffect(() => { void load(); }, [server.id, server.baseUrl, server.token]);
  return { config, setConfig, read, load };
}

function TerminalSettings({ server }: { server: Server }) {
  const { config, setConfig, read, load } = useMachineConfig(server);
  const [origins, setOrigins] = useState<string | null>(null);
  const save = useMachineAction();
  const stored = String(config?.campos.term_origins?.valor ?? '');
  const value = origins ?? stored;
  const busy = read.busy || save.busy;
  return <MachinePage title={m.mobile_machines_terminal_title()} save={{ disabled: !config?.campos.term_origins || busy || value === stored, busy: save.busy,
    onPress: () => save.run(async (signal) => {
      const next = await patchConfigForServer(server, { term_origins: value });
      ensureActive(signal);
      const field = next?.campos?.term_origins;
      if (!field || typeof field.valor !== 'string') throw new Error(m.mobile_machines_invalid_response());
      if (field.erro) throw new Error(field.erro);
      return next;
    }, (next) => { setConfig({ ...config!, ...next }); setOrigins(String(next.campos.term_origins.valor)); }, m.mobile_machines_saved()) }}>
    {read.busy ? <Notice text={m.native_server_loading()} /> : null}
    {config?.campos.term_origins ? <>
      <MachineGroup><MachineField label={m.mobile_machines_term_origins()} value={value} onChangeText={setOrigins} editable={!busy} multiline stacked /></MachineGroup>
      <MachineHelp>{m.mobile_machines_terminal_help()}</MachineHelp>
    </> : config ? <Notice text={m.mobile_machines_terminal_unavailable()} /> : null}
    <Notice text={read.error || save.error} error /><Notice text={save.message} />
    {read.error ? <MachineGroup><MachineRow title={m.mobile_machines_retry()} action onPress={load} disabled={busy} /></MachineGroup> : null}
  </MachinePage>;
}

function NetworkDiagnostics({ server }: { server: Server }) {
  const { config, read, load } = useMachineConfig(server);
  const [reach, setReach] = useState<Reach | null>(null);
  const check = useMachineAction();
  const loadReach = () => check.run((signal) => readReach(server, signal), setReach);
  useEffect(() => { void loadReach(); }, [server.id, server.baseUrl, server.token]);
  const names = { nesta_maquina: m.mobile_machines_this_machine(), rede_local: m.mobile_machines_local_address(), tailscale: m.mobile_machines_remote_address(), publico: m.mobile_machines_public_url() };
  return <MachinePage title={m.mobile_machines_diagnostics_title()}>
    {read.busy || check.busy ? <Notice text={m.native_server_loading()} /> : null}
    <Notice text={read.error || check.error} error />
    {config ? <MachineGroup>
      <MachineRow title={m.mobile_machines_bind()} value={String(config.somente_leitura?.lan_bind_ip ?? '—')} />
      <MachineRow title={m.mobile_machines_port()} value={String(config.somente_leitura?.port ?? '—')} />
      <MachineRow title={m.mobile_machines_public_url()} description={String(config.somente_leitura?.public_url ?? '—')} />
    </MachineGroup> : null}
    {reach ? <MachineGroup>
      <MachineRow title={m.mobile_machines_loopback_label()} value={reach.loopback ? m.mobile_machines_yes() : m.mobile_machines_no()} />
      {reach.bind !== config?.somente_leitura?.lan_bind_ip ? <MachineRow title={m.mobile_machines_detected_bind()} value={reach.bind} /> : null}
      {reach.enderecos.map((address, index) => <MachineRow key={`${address.tipo}:${index}`} title={names[address.tipo]}
        description={[address.url, `${address.estado}${address.tempo_ms === null ? '' : ` · ${address.tempo_ms} ms`}`, address.motivo].filter(Boolean).join('\n')} />)}
    </MachineGroup> : null}
    {reach && !reach.enderecos.length ? <Notice text={m.mobile_machines_reach_empty()} /> : null}
    <MachineGroup><MachineRow title={m.mobile_machines_run_diagnostics()} action disabled={read.busy || check.busy} onPress={() => { void load(); void loadReach(); }} /></MachineGroup>
  </MachinePage>;
}
