import { useEffect, useRef, useState } from 'react';
import { Alert, View } from 'react-native';
import { getAtualizacaoEm, iniciarAtualizacaoEm, reiniciarServidorEm, matchesRestartResult, type AtualizacaoEstado, type Server } from '@hangar/core';
import { MachineGroup, MachineRow } from './MachineRows';
import { ensureActive, reason } from './api';
import { Notice, styles, useMachineAction } from './parts';
import { useRouter } from 'expo-router';
import { firstPendingServer, requestUpdateConfirm, whatsNewHref } from '../updates/pending';
import * as m from '../../paraglide/messages';

type Operation = 'restart' | 'update';

function pause(signal: AbortSignal, ms: number): Promise<void> {
  return new Promise((resolve, reject) => {
    if (signal.aborted) { reject(new Error('cancelled')); return; }
    const abort = () => { clearTimeout(timer); reject(new Error('cancelled')); };
    const timer = setTimeout(() => { signal.removeEventListener('abort', abort); resolve(); }, ms);
    signal.addEventListener('abort', abort, { once: true });
  });
}

// Os wrappers do core não recebem sinal; cancelamos a espera sem aproveitar a resposta antiga.
function bounded<T>(promise: Promise<T>, signal: AbortSignal, ms: number): Promise<T> {
  return new Promise((resolve, reject) => {
    let settled = false;
    const finish = (apply: () => void) => {
      if (settled) return;
      settled = true; clearTimeout(timer); signal.removeEventListener('abort', abort); apply();
    };
    const abort = () => finish(() => reject(new Error('cancelled')));
    const timer = setTimeout(() => finish(() => reject(Object.assign(new Error(m.mobile_machines_unconfirmed_reason()), { name: 'TimeoutError' }))), ms);
    signal.addEventListener('abort', abort, { once: true });
    if (signal.aborted) abort();
    promise.then((value) => finish(() => resolve(value)), (error) => finish(() => reject(error)));
  });
}

export function MachineOperations({ server }: { server: Server }) {
  return <OperationControls key={`${server.id}:${server.baseUrl}:${server.token}`} server={server} />;
}
function OperationControls({ server }: { server: Server }) {
  const action = useMachineAction();
  const router = useRouter();
  const alive = useRef(true);
  const confirmation = useRef<{ started: boolean } | null>(null);
  const [asking, setAsking] = useState(false);
  const [result, setResult] = useState<AtualizacaoEstado | null>(null);
  useEffect(() => {
    alive.current = true;
    return () => { alive.current = false; confirmation.current = null; };
  }, []);

  const request = (operation: Operation) => {
    if (confirmation.current || action.busy || !alive.current) return;
    const ticket = { started: false };
    confirmation.current = ticket; setAsking(true);
    const label = operation === 'restart' ? m.mobile_machines_restart() : m.mobile_machines_update();
    const cancel = () => {
      if (confirmation.current !== ticket || ticket.started || !alive.current) return;
      confirmation.current = null; setAsking(false);
    };
    let updated = false;
    const start = () => {
        if (confirmation.current !== ticket || ticket.started || !alive.current) return;
        ticket.started = true; setAsking(false); setResult(null);
        void action.run(async (signal) => {
          ensureActive(signal);
          const started = await bounded(operation === 'restart' ? reiniciarServidorEm(server) : iniciarAtualizacaoEm(server), signal, 15_000);
          ensureActive(signal);
          if (started?.ok !== true || !Number.isSafeInteger(started.pid) || started.pid <= 0) throw new Error(m.mobile_machines_invalid_response());
          const deadline = Date.now() + (operation === 'restart' ? 120_000 : 600_000);
          let lastCause: string = m.mobile_machines_unconfirmed_reason();
          const unconfirmed = () => new Error(m.mobile_machines_operation_unconfirmed({ reason: lastCause }));
          for (;;) {
            const remaining = deadline - Date.now();
            if (remaining <= 0) throw unconfirmed();
            await pause(signal, Math.min(2000, remaining));
            ensureActive(signal);
            if (Date.now() >= deadline) throw unconfirmed();
            let state: AtualizacaoEstado;
            try {
              const response = await bounded(getAtualizacaoEm(server), signal, deadline - Date.now());
              ensureActive(signal);
              if (!response?.estado || typeof response.estado !== 'object' || Array.isArray(response.estado)) throw new Error(m.mobile_machines_invalid_response());
              state = response.estado;
            } catch (error) {
              ensureActive(signal);
              if (Date.now() >= deadline) throw unconfirmed();
              const status = (error as { status?: unknown })?.status;
              if (status === 401) throw error;
              lastCause = reason(error);
              continue;
            }
            if (operation === 'restart' ? !matchesRestartResult(state, started) : state.pid !== started.pid) continue;
            setResult(state);
            if (state.fase !== 'pronto') continue;
            if (state.ok !== true || state.reinicio_erro || state.erro) {
              throw new Error(m.mobile_machines_operation_failed({ reason: state.reinicio_erro || state.erro || m.mobile_machines_unconfirmed_reason() }));
            }
            updated = operation === 'update';
            return state;
          }
        }, setResult, m.mobile_machines_operation_ready()).finally(() => {
          if (alive.current && confirmation.current === ticket) { confirmation.current = null; setAsking(false); }
          if (updated && alive.current && firstPendingServer([server.id])) router.push(whatsNewHref(server.id));
        });
    };
    if (operation === 'update') {
      void requestUpdateConfirm(server, () => router.push('/update-confirm' as never)).then((ok) => { if (ok) start(); else cancel(); });
      return;
    }
    Alert.alert(m.mobile_machines_confirm_operation({ action: label, name: server.label }), m.mobile_machines_confirm_operation_body(), [
      { text: m.comum_cancelar(), style: 'cancel', onPress: cancel },
      { text: label, onPress: start },
    ], { cancelable: true, onDismiss: cancel });
  };
  const busy = asking || action.busy;
  return <>
    <MachineGroup>
      <MachineRow title={m.mobile_machines_update()} action onPress={() => request('update')} disabled={busy} />
      <MachineRow title={m.mobile_machines_restart()} onPress={() => request('restart')} disabled={busy} />
    </MachineGroup>
    <View style={styles.content}>
      {busy ? <Notice text={m.mobile_machines_busy()} /> : null}
      {result?.fase === 'rodando' ? <Notice text={m.mobile_machines_progress({ step: String(result.passo ?? 0), total: String(result.total ?? 0), text: result.texto ?? '' })} /> : null}
      <Notice text={action.error} error />
      <Notice text={action.message} />
      {result?.reiniciar_manual ? <Notice text={m.mobile_machines_restart_manual()} /> : null}
      {Array.isArray(result?.avisos) ? result.avisos.filter((warning): warning is string => typeof warning === 'string').map((warning, index) => <Notice key={index} text={m.mobile_machines_warning({ reason: warning })} />) : null}
      {Array.isArray(result?.passos_invalidos) && result.passos_invalidos.length ? <Notice text={m.mobile_machines_warning({ reason: result.passos_invalidos.join(', ') })} /> : null}
    </View>
  </>;
}
