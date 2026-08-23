import { useCallback, useEffect, useRef, useState } from 'react';
import { AppState } from 'react-native';
import {
  useAudioRecorder,
  RecordingPresets,
  requestRecordingPermissionsAsync,
  setAudioModeAsync,
} from 'expo-audio';
import { novoEstadoVad, passoVad } from '@hangar/core';
import type { EstadoVad, MotivoFim } from '@hangar/core';

const TETO_MS = 180_000;

interface Opts {
  onFim: (file: File, motivo: MotivoFim) => void;
}

export function useDitado({ onFim }: Opts) {
  // isMeteringEnabled: true para VAD; RecordingPresets.HIGH_QUALITY já traz extension/sampleRate etc
  const recorder = useAudioRecorder({
    ...RecordingPresets.HIGH_QUALITY,
    isMeteringEnabled: true,
  });

  const [gravando, setGravando] = useState(false);
  const [rms, setRms] = useState(0);

  const vadRef = useRef<EstadoVad>(novoEstadoVad());
  const gravandoRef = useRef(false);
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

  const parar = useCallback(
    async (motivo: MotivoFim) => {
      if (!gravandoRef.current) return;
      gravandoRef.current = false;
      setGravando(false);
      limparTimers();
      setRms(0);
      try {
        await recorder.stop();
        const uri = recorder.uri;
        if (!uri) return;
        const res = await fetch(uri);
        const blob = await res.blob();
        const file = new File([blob], 'ditado.m4a', { type: 'audio/m4a' });
        onFim(file, motivo);
      } catch {
        // sem arquivo — nada a fazer, o caller mostra erro se precisar
      }
    },
    [recorder, onFim, limparTimers],
  );

  const iniciar = useCallback(async () => {
    if (gravandoRef.current) return;
    const perm = await requestRecordingPermissionsAsync();
    if (!perm.granted) {
      throw new Error('permission_denied');
    }
    await setAudioModeAsync({ allowsRecording: true, playsInSilentMode: true });

    vadRef.current = novoEstadoVad();
    inicioRef.current = Date.now();
    gravandoRef.current = true;
    setGravando(true);
    setRms(0);

    try {
      await recorder.prepareToRecordAsync();
    } catch {
      // prepare pode falhar se já preparado — segue para record
    }
    recorder.record();

    tetoRef.current = setTimeout(() => {
      void parar('teto');
    }, TETO_MS);

    intervaloRef.current = setInterval(() => {
      const st = recorder.getStatus();
      const dB = (st as unknown as { metering?: number }).metering;
      let rmsVal = 0;
      let raw = 0;
      if (typeof dB === 'number' && Number.isFinite(dB)) {
        raw = Math.pow(10, dB / 20);
        if (!Number.isFinite(raw)) raw = 0;
        // para exibição, amplia como no front (*5) para a barra não ficar invisível
        rmsVal = Math.min(1, raw * 5);
        // VAD usa o raw (0..1), não o ampliado
        const r = passoVad(vadRef.current, raw, Date.now());
        if (r === 'encerra') {
          void parar('silencio');
          return;
        }
      } else {
        // sem metering (Android sem suporte ou falha) → VAD desligado, só botão/teto
        rmsVal = 0;
      }
      setRms(rmsVal);
      if (Date.now() - inicioRef.current >= TETO_MS) {
        void parar('teto');
      }
    }, 55);
  }, [recorder, parar]);

  useEffect(() => {
    const sub = AppState.addEventListener('change', (next) => {
      if (next !== 'active' && gravandoRef.current) {
        void parar('escondeu');
      }
    });
    return () => {
      sub.remove();
      limparTimers();
      gravandoRef.current = false;
    };
  }, [parar, limparTimers]);

  return { gravando, rms, iniciar, parar };
}
