import { useCallback, useEffect, useRef, useState } from 'react';
import { transcribeDictationForServer } from '@hangar/core';
import type { MotivoFim, Server } from '@hangar/core';
import { useDitado, type RecordingStop } from '../ditado/useDitado';
import { failureCode, recordTiming, stageClock } from '../ditado/clientTiming';
import { dictationStyleStore } from '../ditado/ditadoEstiloStore';
import * as m from '../../paraglide/messages';

// Ditado antes de a sessão existir: o servidor só transcreve e limpa (rota sem sessão), e o texto
// entra no campo. Falhou, o áudio fica na memória para "transcrever de novo".
export function useHomeDictation(server: Server, onText: (text: string) => void) {
  const [transcribing, setTranscribing] = useState(false);
  const [error, setError] = useState('');
  const [failed, setFailed] = useState<File | null>(null);
  const mounted = useRef(true);
  const busy = useRef(false);
  const estilo = useRef<{ serverId: string; value: string | undefined } | null>(null);
  const onTextRef = useRef(onText);
  onTextRef.current = onText;

  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; };
  }, []);

  // `stop` só na gravação nova: o "de novo" não mede, como na conversa.
  const transcribe = useCallback(async (file: File, stop?: RecordingStop & { motivo: MotivoFim }) => {
    if (busy.current) return;
    busy.current = true;
    setTranscribing(true);
    setError('');
    setFailed(null);
    const clock = stageClock();
    try {
      const style = dictationStyleStore(server).getState();
      const value = estilo.current?.serverId === server.id ? estilo.current.value : style.pronto ? style.valor : undefined;
      // Sobe e transcreve num pedido só, sem sessão.
      clock.enter('transcribe');
      const { text, aviso } = await transcribeDictationForServer(server, file, value);
      const trimmed = text.trim();
      if (!trimmed) throw new Error(m.composer_transcricao_vazia());
      clock.enter('finish');
      if (mounted.current) onTextRef.current(trimmed);
      if (stop) recordTiming(server, stop, clock, stop.motivo, undefined, !!aviso);
      if (!mounted.current) return;
      // A limpeza pode desistir e devolver o cru: isso aparece, não passa por limpo.
      if (aviso) setError(aviso);
    } catch (e) {
      const detail = e instanceof Error ? e.message : m.composer_falha_transcricao();
      if (stop) recordTiming(server, stop, clock, stop.motivo, { code: failureCode(detail) });
      if (!mounted.current) return;
      setError(/^(501|503):/.test(detail) ? `${m.composer_ditado_indisponivel()}: ${detail}` : detail);
      setFailed(file);
    } finally {
      busy.current = false;
      if (mounted.current) setTranscribing(false);
    }
  }, [server]);

  // Cancelar para o microfone e joga o áudio fora, sem transcrever nem guardar para "de novo".
  const discard = useRef(false);
  const { gravando, rms, iniciar, parar } = useDitado({
    server,
    onFim: (file, motivo, _uri, stop) => {
      if (discard.current) { discard.current = false; return; }
      void transcribe(file, { ...stop, motivo });
    },
    onErroParada: (e) => {
      if (mounted.current) setError(e.message === 'ditado_parada_falhou' ? m.composer_falha_gravacao() : e.message || m.composer_falha_gravacao());
    },
  });

  const toggle = useCallback(async () => {
    if (busy.current) return;
    if (gravando) { void parar('botao'); return; }
    setError('');
    setFailed(null);
    discard.current = false;
    // O estilo que a pílula mostra na hora de falar é o que o servidor aplica.
    const style = dictationStyleStore(server).getState();
    estilo.current = { serverId: server.id, value: style.pronto ? style.valor : undefined };
    try {
      await iniciar();
    } catch (e) {
      const msg = e instanceof Error ? e.message : '';
      setError(msg === 'permission_denied' ? m.composer_sem_acesso_mic() : msg || m.composer_falha_gravacao());
    }
  }, [gravando, iniciar, parar, server]);

  const retry = useCallback(() => { if (failed) void transcribe(failed); }, [failed, transcribe]);
  const dismiss = useCallback(() => { setError(''); setFailed(null); }, []);
  // Enviar ou trocar de máquina no meio da gravação não pode deixar o microfone aberto.
  const stop = useCallback(() => parar('botao'), [parar]);
  const cancel = useCallback(() => {
    discard.current = true;
    void parar('botao');
  }, [parar]);

  return { gravando, rms, transcribing, error, canRetry: !!failed, toggle, retry, dismiss, stop, cancel };
}
