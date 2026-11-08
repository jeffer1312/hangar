import { useCallback, useEffect, useRef, useState } from 'react';
import { AppState, Platform } from 'react-native';
import {
  AudioModule,
  AudioQuality,
  IOSOutputFormat,
  RecordingPresets,
  requestNotificationPermissionsAsync,
  requestRecordingPermissionsAsync,
  setAudioModeAsync,
} from 'expo-audio';
import type { AudioRecorder, RecordingEvents, RecordingOptions, RecordingStatus } from 'expo-audio';
import type { SharedObject } from 'expo';
import { File as FsFile, FileMode } from 'expo-file-system';
import { runtimeVersion } from 'expo-updates';
import { useSharedValue } from 'react-native-reanimated';
import { novoEstadoVad, passoVad, registrarDiag } from '@hangar/core';
import type { EstadoVad, MotivoFim, Server } from '@hangar/core';
import * as m from '../../paraglide/messages';
import { dictationStyleStore } from './ditadoEstiloStore';
import { beginVoiceDictation } from '../voice/audioSession';
import { toast } from '../../ui/Toast';
import { startLivePieces, type GrowingFile, type LivePieces, type PiecesHandle } from './livePieces';

const STOP_STATUS_TIMEOUT_MS = 5_000;
// Servidor lento não segura o microfone: passado isto vale o último teto conhecido.
const CEILING_REFRESH_TIMEOUT_MS = 1_000;
// O nativo termina pelo `forDuration` um pouco antes do relógio JS: dentro desta folga ainda é o teto.
const CEILING_TOLERANCE_MS = 500;
// Um aviso por execução do app: repetir a cada gravação vira ruído.
let foregroundOnlyWarned = false;

// O serviço de segundo plano precisa existir no binário, não só na atualização JS.
const backgroundRecording = () => Platform.OS === 'ios'
  || (Platform.OS === 'android' && Number(/^native-(\d+)$/.exec(runtimeVersion ?? '')?.[1]) >= 3);

// O tipo do áudio não resolve a base quando o core do Expo fica aninhado no SDK.
type ManagedRecorder = AudioRecorder & InstanceType<SharedObject<RecordingEvents>>;

const M4A_OPTIONS: RecordingOptions = { ...RecordingPresets.HIGH_QUALITY, isMeteringEnabled: true };
// iPhone: PCM16 16 kHz mono num WAV que o gravador escreve aos poucos e dá para ler enquanto cresce.
const WAV_OPTIONS: RecordingOptions = {
  ...M4A_OPTIONS, extension: '.wav', sampleRate: 16000, numberOfChannels: 1, bitRate: 256000,
  ios: { extension: '.wav', outputFormat: IOSOutputFormat.LINEARPCM, audioQuality: AudioQuality.MAX, sampleRate: 16000,
    linearPCMBitDepth: 16, linearPCMIsBigEndian: false, linearPCMIsFloat: false },
};

function openGrowingFile(uri: string): GrowingFile {
  const handle = new FsFile(uri).open(FileMode.ReadOnly);
  return {
    size: () => handle.size ?? 0,
    read: (offset, length) => { handle.offset = offset; return handle.readBytes(length); },
    close: () => handle.close(),
  };
}

interface Opts {
  /** `pieces`: pedaços já transcritos durante a fala (só iPhone, só com `pieces` ligado); `null` = caminho de hoje. */
  onFim: (file: File, motivo: MotivoFim, uri: string, pieces: PiecesHandle | null) => void;
  onErroParada?: (e: Error) => void;
  /** Máquina da conversa: o teto da gravação é o configurado nela. */
  server?: Server | null;
  /** Transcreve em pedaços enquanto a pessoa fala; quem liga organiza o texto no fim (`runDictation`). */
  pieces?: boolean;
}

interface Recording {
  recorder: ManagedRecorder | null;
  releaseAudio: () => void;
  phase: 'permission' | 'preparing' | 'recording' | 'stopping';
  interrupted: boolean;
  onFim: Opts['onFim'];
  onErroParada: Opts['onErroParada'];
  reason?: MotivoFim;
  stopping?: Promise<void>;
  finished: Promise<RecordingStatus>;
  error?: Error;
  errorReported?: boolean;
  ceilingMs?: number;
  /** Antes do `record()`: o teto nativo conta a partir dele. */
  recordStartedAt?: number;
  background: boolean;
  subscription?: { remove(): void };
  /** Gravando em WAV (iPhone); falso = o .m4a de hoje. */
  wav: boolean;
  pieces?: LivePieces;
}

function reportRecordingError(recording: Recording, error: unknown) {
  if (recording.errorReported) return;
  recording.errorReported = true;
  try {
    recording.onErroParada?.(error instanceof Error ? error : new Error('ditado_parada_falhou'));
  } catch (callbackError) {
    console.error(callbackError);
  }
}

export function useDitado({ onFim, onErroParada, server, pieces: piecesWanted = false }: Opts) {
  const [gravando, setGravando] = useState(false);
  const rms = useSharedValue(0);

  const vadRef = useRef<EstadoVad>(novoEstadoVad());
  const recordingRef = useRef<Recording | null>(null);
  const mountedRef = useRef(true);
  const appStateRef = useRef(AppState.currentState);
  const intervaloRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const tetoRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const inicioRef = useRef<number>(0);

  const limparTimers = useCallback(() => {
    if (intervaloRef.current) {
      clearInterval(intervaloRef.current);
      intervaloRef.current = null;
    }
    if (tetoRef.current) {
      clearTimeout(tetoRef.current);
      tetoRef.current = null;
    }
  }, []);

  const releaseRecording = useCallback((recording: Recording) => {
    // Sem efeito depois de entregues ao `onFim`: aí quem aborta é quem os recebeu.
    recording.pieces?.abandon('recording_released');
    try {
      recording.subscription?.remove();
    } catch (e) {
      reportRecordingError(recording, e);
    }
    try {
      recording.recorder?.release();
    } catch (e) {
      reportRecordingError(recording, e);
    } finally {
      if (recordingRef.current === recording) recordingRef.current = null;
      recording.releaseAudio();
    }
  }, []);

  const finishRecording = useCallback(
    (recording: Recording, motivo: MotivoFim): Promise<void> => {
      recording.interrupted = true;
      if (!recording.reason || motivo === 'escondeu') recording.reason = motivo;
      if (recording.stopping) return recording.stopping;
      // A preparação pendente termina antes de liberar o objeto nativo.
      if (recording.phase !== 'recording' || !recording.recorder) return Promise.resolve();
      const recorder = recording.recorder;
      recording.phase = 'stopping';
      limparTimers();
      recording.stopping = (async () => {
        let statusTimer: ReturnType<typeof setTimeout> | undefined;
        try {
          // Sem confirmação nativa, falha em vez de entregar áudio incerto.
          const status = await Promise.race([
            (async () => { await recorder.stop(); return recording.finished; })(),
            new Promise<never>((_, reject) => {
              statusTimer = setTimeout(() => {
                reject(new Error('ditado_parada_falhou', { cause: 'recording_status_timeout' }));
              }, STOP_STATUS_TIMEOUT_MS);
            }),
          ]);
          if (recording.error) throw recording.error;
          const uri = status.url ?? recorder.uri;
          if (!uri) throw new Error('ditado_parada_falhou');
          const res = await fetch(uri);
          const blob = await res.blob();
          if (recording.error) throw recording.error;
          const file = recording.wav ? new File([blob], 'ditado.wav', { type: 'audio/wav' })
            : new File([blob], 'ditado.m4a', { type: 'audio/m4a' });
          // Gravação parada: o arquivo está completo e o resto sai daqui.
          const pieces = recording.pieces?.finish() ?? null;
          recording.onFim(file, recording.reason ?? motivo, uri, pieces);
        } catch (e) {
          reportRecordingError(recording, e);
        } finally {
          if (statusTimer !== undefined) clearTimeout(statusTimer);
          releaseRecording(recording);
          if (mountedRef.current) {
            setGravando(false);
            rms.value = 0;
          }
        }
      })();
      return recording.stopping;
    },
    [limparTimers, releaseRecording, rms],
  );

  const parar = useCallback((motivo: MotivoFim) => {
    const recording = recordingRef.current;
    return recording ? finishRecording(recording, motivo) : Promise.resolve();
  }, [finishRecording]);

  const pararRef = useRef(parar);
  pararRef.current = parar;

  const iniciar = useCallback(async () => {
    if (recordingRef.current || !mountedRef.current || appStateRef.current !== 'active') return;
    let complete!: (status: RecordingStatus) => void;
    const finished = new Promise<RecordingStatus>((resolve) => { complete = resolve; });
    const recording: Recording = {
      recorder: null, releaseAudio: beginVoiceDictation(),
      phase: 'permission', interrupted: false, onFim, onErroParada, finished, background: false, wav: false,
    };
    recordingRef.current = recording;
    try {
      const perm = await requestRecordingPermissionsAsync();
      if (recording.interrupted) return;
      if (!perm.granted) throw new Error('permission_denied');
      let background = backgroundRecording();
      if (background && Platform.OS === 'android' && Number(Platform.Version) >= 33) {
        const notifications = await requestNotificationPermissionsAsync();
        if (recording.interrupted) return;
        // Sem a notificação do serviço, grava só com o app na frente.
        background = notifications.granted;
        if (!background) {
          registrarDiag({ evento: 'dictation.background_unavailable', nivel: 'aviso', codigo: 'notifications_denied' });
          if (!foregroundOnlyWarned) { foregroundOnlyWarned = true; toast.aviso(m.composer_ditado_so_na_frente()); }
        }
      }
      recording.background = background;
      recording.phase = 'preparing';
      await setAudioModeAsync({ allowsRecording: true, playsInSilentMode: true, allowsBackgroundRecording: background });
      if (recording.interrupted) return;

      const onStatus = (status: RecordingStatus) => {
        if (status.hasError || status.mediaServicesDidReset) {
          if (!recording.error) {
            registrarDiag({ evento: 'dictation.recording_failed', nivel: 'erro',
              codigo: status.mediaServicesDidReset ? 'media_services_reset' : 'recording_error', detalhe: status.error ?? undefined });
            recording.error = new Error(m.composer_falha_gravacao());
          }
          recording.interrupted = true;
        }
        if (status.isFinished || recording.error) {
          complete(status);
          // Sem parada pedida, só é teto se o `forDuration` venceu; antes disso o sistema interrompeu.
          const atCeiling = !recording.error && recording.ceilingMs !== undefined && recording.recordStartedAt !== undefined
            && Date.now() - recording.recordStartedAt >= recording.ceilingMs - CEILING_TOLERANCE_MS;
          if (recording.phase === 'recording') void finishRecording(recording, atCeiling ? 'teto' : 'escondeu');
        }
      };
      // O hook do Expo libera no unmount antes da parada assíncrona; este objeto vive até a leitura.
      const create = (options: RecordingOptions) => {
        const created = new AudioModule.AudioRecorder({
          ...options,
          ...(Platform.OS === 'ios' ? options.ios : Platform.OS === 'android' ? options.android : options.web),
        }) as ManagedRecorder;
        recording.recorder = created;
        recording.subscription = created.addListener('recordingStatusUpdate', onStatus);
        return created;
      };
      // iPhone grava em WAV; o que não abre volta ao .m4a de hoje, sem pedaços.
      const prepare = async (): Promise<ManagedRecorder> => {
        if (Platform.OS === 'ios') {
          try {
            const wav = create(WAV_OPTIONS);
            await wav.prepareToRecordAsync();
            recording.wav = true;
            return wav;
          } catch (e) {
            registrarDiag({ evento: 'dictation.wav_unavailable', nivel: 'aviso', codigo: 'wav_prepare_failed',
              detalhe: e instanceof Error ? e.message : undefined });
            recording.subscription?.remove();
            recording.recorder?.release();
            recording.subscription = undefined;
            recording.recorder = null;
          }
        }
        const created = create(M4A_OPTIONS);
        await created.prepareToRecordAsync();
        return created;
      };
      const styleStore = dictationStyleStore(server);
      let refreshTimer: ReturnType<typeof setTimeout> | undefined;
      const refreshed = Promise.race([
        styleStore.getState().refreshCeiling().then(() => true),
        new Promise<false>((resolve) => { refreshTimer = setTimeout(() => resolve(false), CEILING_REFRESH_TIMEOUT_MS); }),
      ]);
      let recorder: ManagedRecorder;
      try {
        let fresh: boolean;
        [recorder, fresh] = await Promise.all([prepare(), refreshed]);
        if (!fresh) registrarDiag({ evento: 'dictation.ceiling_stale', nivel: 'aviso', codigo: 'ceiling_refresh_timeout' });
      } finally {
        clearTimeout(refreshTimer);
      }
      if (recording.interrupted) return;
      const ceilingMs = styleStore.getState().ceilingMs;
      recording.ceilingMs = ceilingMs;
      // O teto nativo continua valendo quando o sistema suspende os timers JS.
      recording.recordStartedAt = Date.now();
      recorder.record({ forDuration: ceilingMs / 1000 });
      if (recording.interrupted) return;
      recording.phase = 'recording';
      vadRef.current = novoEstadoVad();
      inicioRef.current = Date.now();
      setGravando(true);
      rms.value = 0;
      // Convidado não alcança a rota dos pedaços; app fora da frente segue o caminho de hoje.
      const uri = recorder.uri;
      if (piecesWanted && recording.wav && uri && server && !server.invite && !server.inviteEnded && !server.removed
        && appStateRef.current === 'active') {
        recording.pieces = startLivePieces({
          server: { ...server }, open: () => openGrowingFile(uri),
          onFail: (codigo, detalhe) => registrarDiag({ evento: 'dictation.pieces_fallback', nivel: 'aviso', codigo, detalhe }),
        });
      }

      tetoRef.current = setTimeout(() => {
        void finishRecording(recording, 'teto');
      }, ceilingMs);

      // Fora da tela o VAD continua: a fala que termina encerra como 'escondeu', que guarda o áudio
      // sem envio automático. Calado desde que saiu, só o teto encerra, para olhar outro app no meio.
      intervaloRef.current = setInterval(() => {
        const hidden = appStateRef.current !== 'active';
        const st = recorder.getStatus();
        const dB = (st as unknown as { metering?: number }).metering;
        let rmsVal = 0;
        let raw = 0;
        if (typeof dB === 'number' && Number.isFinite(dB)) {
          raw = Math.pow(10, dB / 20);
          if (!Number.isFinite(raw)) raw = 0;
          // O relógio do gravador anda junto com os bytes do arquivo; sem ele, o tempo desde o início.
          const recorded = (st as unknown as { durationMillis?: number }).durationMillis;
          recording.pieces?.feed(typeof recorded === 'number' ? recorded : Date.now() - inicioRef.current, raw);
          // para exibição, amplia como no front (*5) para a barra não ficar invisível
          rmsVal = Math.min(1, raw * 5);
          // VAD usa o raw (0..1), não o ampliado
          const r = passoVad(vadRef.current, raw, Date.now());
          if (r === 'encerra') {
            void finishRecording(recording, hidden ? 'escondeu' : 'silencio');
            return;
          }
        } else {
          // sem metering (Android sem suporte ou falha) → VAD desligado, só botão/teto
          rmsVal = 0;
        }
        rms.value = rmsVal;
        if (Date.now() - inicioRef.current >= ceilingMs) {
          void finishRecording(recording, 'teto');
        }
      }, 55);
    } catch (e) {
      if (recording.error) reportRecordingError(recording, recording.error);
      else throw e;
    } finally {
      if (recording.phase === 'permission' || recording.phase === 'preparing') {
        try {
          if (recording.error) reportRecordingError(recording, recording.error);
        } finally {
          releaseRecording(recording);
        }
      }
    }
  }, [onFim, onErroParada, server, finishRecording, releaseRecording, rms]);

  useEffect(() => {
    mountedRef.current = true;
    const sub = AppState.addEventListener('change', (next) => {
      appStateRef.current = next;
      const recording = recordingRef.current;
      if (!recording || recording.phase === 'permission') return;
      if (next !== 'active') {
        vadRef.current = novoEstadoVad();
        // Fora da frente o sistema pode suspender o JS no meio de um pedaço: o arquivo inteiro é mais seguro.
        recording.pieces?.abandon('background');
        if (recording.phase !== 'recording' || !recording.background) void pararRef.current('escondeu');
      } else if (recording.phase === 'recording' && recording.recorder) {
        // Uma interrupção real do microfone não pode deixar a onda fingindo que ainda capta.
        try {
          if (recording.recorder.getStatus().isRecording === false) void pararRef.current('escondeu');
        } catch (error) {
          registrarDiag({ evento: 'dictation.recording_failed', nivel: 'erro', codigo: 'recording_status_failed' });
          recording.error = error instanceof Error ? error : new Error(m.composer_falha_gravacao());
          void pararRef.current('escondeu');
        }
      }
    });
    return () => {
      sub.remove();
      mountedRef.current = false;
      void pararRef.current('escondeu');
    };
  }, []);

  return { gravando, rms, iniciar, parar };
}
