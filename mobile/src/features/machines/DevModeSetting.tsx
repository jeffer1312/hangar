import { useEffect, useRef, useState } from 'react';
import { Alert, View } from 'react-native';
import type { AtualizacaoEstado, Server } from '@hangar/core';
import { readDevMode, switchDevMode, type DevMode } from './api';
import { followOperation, pause } from './MachineOperations';
import { MachineField, MachineGroup, MachineRow } from './MachineRows';
import { Notice, styles, useMachineAction } from './parts';
import * as m from '../../paraglide/messages';

/** Liga e desliga o modo desenvolvedor: a mesma troca do `hangar-server dev-mode`, pelo executor que reinicia o servidor. */
export function DevModeSetting({ server }: { server: Server }) {
  return <DevModeControls key={`${server.id}:${server.baseUrl}:${server.token}`} server={server} />;
}

/** Releituras logo após a troca, enquanto o servidor reinicia. */
const REREADS = 5;
const REREAD_PAUSE_MS = 1500;

function DevModeControls({ server }: { server: Server }) {
  // `undefined` = ainda sem leitura boa; `null` = máquina sem a troca pela tela (só na primeira
  // leitura); `unknown` = depois de uma leitura boa o servidor deixou de informar o modo.
  const [mode, setMode] = useState<DevMode | null | undefined | 'unknown'>(undefined);
  const seen = useRef(false);
  const [folder, setFolder] = useState('');
  const [progress, setProgress] = useState<AtualizacaoEstado | null>(null);
  const [note, setNote] = useState('');
  const [asking, setAsking] = useState(false);
  const alive = useRef(true);
  const read = useMachineAction();
  const change = useMachineAction();
  const load = (attempts = 1) => read.run(async (signal) => {
    for (let attempt = 1; ; attempt++) {
      try { return await readDevMode(server, signal); }
      catch (error) {
        if (attempt >= attempts || (error as { status?: number }).status === 401) throw error;
        await pause(signal, REREAD_PAUSE_MS);
      }
    }
  }, (value) => {
    if (value) seen.current = true;
    // Quem já viu o controle continua com ele, para poder desligar um checkout sem a troca.
    setMode(value ?? (seen.current ? 'unknown' : null));
    // A pasta sugerida só entra com o campo vazio: o que a pessoa digitou fica.
    if (value) setFolder((current) => current.trim() ? current : value.checkout);
  });
  useEffect(() => {
    alive.current = true;
    void load();
    return () => { alive.current = false; };
  }, []);
  if (mode === null) return null;

  const busy = asking || change.busy;
  const known = mode === 'unknown' ? undefined : mode;
  // Sem leitura boa depois de uma: o modo velho não vale, sobra desligar.
  const lost = mode === 'unknown' || (seen.current && !!read.error && !read.busy);
  const developer = known?.developer === true;
  const running = change.busy && progress?.fase === 'rodando' && progress.total
    ? m.mobile_machines_progress({ step: String(progress.passo ?? 0), total: String(progress.total), text: progress.texto ?? '' }) : '';
  const value = change.busy ? running || m.native_dev_mode_switching()
    : lost ? m.native_dev_mode_unknown()
    : !known ? read.error ? undefined : m.native_server_loading()
    : developer ? m.native_dev_mode_developer({ folder: known.checkout }) : m.native_dev_mode_package({ channel: known.channel });

  const request = () => {
    if ((!known && !lost) || busy || !alive.current) return;
    const on = !lost && !developer;
    const checkout = folder.trim();
    let handled = false;
    const cancel = () => { if (handled) return; handled = true; if (alive.current) setAsking(false); };
    setAsking(true);
    Alert.alert(on ? m.native_dev_mode_confirm_on({ server: server.label }) : m.native_dev_mode_confirm_off({ server: server.label }),
      m.native_dev_mode_confirm_desc(), [
        { text: m.comum_cancelar(), style: 'cancel', onPress: cancel },
        { text: on ? m.native_dev_mode_turn_on() : m.native_dev_mode_turn_off(), onPress: () => {
          if (handled || !alive.current) return;
          handled = true; setAsking(false); setProgress(null); setNote('');
          void change.run(async (signal) => {
            const started = await switchDevMode(server, on, checkout, signal);
            // Já estava no modo pedido: nada foi lançado, a rota diz por quê.
            if ('already' in started) return started.already;
            const state = await followOperation(server, (state) => state.pid === started.pid, 300_000, signal, setProgress);
            if (state.ok !== true || state.erro) throw new Error(m.native_dev_mode_failed({ reason: state.erro || m.mobile_machines_unconfirmed_reason() }));
            return state.reiniciar_manual ? m.native_dev_mode_done_manual() : m.native_dev_mode_done();
          }, (text) => { setNote(text); setMode(undefined); void load(REREADS); });
        } },
      ], { cancelable: true, onDismiss: cancel });
  };

  return <>
    <MachineGroup>
      <MachineRow title={m.native_dev_mode_title()} description={m.native_dev_mode_help()} value={value}
        checked={lost || !known || busy ? 'mixed' : developer}
        disabled={read.busy || busy || (!lost && (!known || (!developer && !folder.trim())))} onPress={request} />
      {known && !developer && !lost ? <MachineField stacked label={m.native_dev_mode_folder()} value={folder} onChangeText={setFolder}
        editable={!busy} placeholder={m.native_dev_mode_folder_placeholder()} /> : null}
      {read.error ? <MachineRow title={m.mobile_machines_retry()} action onPress={() => void load()} disabled={read.busy} /> : null}
    </MachineGroup>
    <View style={styles.content}>
      <Notice text={read.error || change.error || (mode === 'unknown' ? m.native_dev_mode_unknown_reason() : '')} error />
      <Notice text={change.busy ? '' : note} />
    </View>
  </>;
}
