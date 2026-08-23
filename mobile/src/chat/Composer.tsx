import { useCallback, useEffect, useRef, useState } from 'react';
import { ActivityIndicator, Platform, Pressable, ScrollView, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import * as ImagePicker from 'expo-image-picker';
import { uploadFile, transcribeFile, steerSession, podeEnviarSozinho } from '@hangar/core';
import type { MotivoFim } from '@hangar/core';
import { Glass } from '../ui/Glass';
import { MultiTextInput, type MultiTextInputHandle } from '../vendor/happy/components/MultiTextInput';
import * as m from '../paraglide/messages';
import { chatStore, filaCount as filaCountOf } from '../stores/chat';
import { useSessions } from '../stores/sessions';
import { useRouter } from 'expo-router';
import { ModelPill } from '../features/pills/ModelPill';
import { EffortPill } from '../features/pills/EffortPill';
import { PermissionPill } from '../features/pills/PermissionPill';
import { EstiloPill } from '../features/ditado/EstiloPill';
import { useDitado } from '../features/ditado/useDitado';
import { useDitadoEstiloStore } from '../features/ditado/ditadoEstiloStore';

interface Props {
  serverId: string;
  name: string;
  draft?: string;
}

export function Composer({ serverId, name, draft }: Props) {
  const { theme } = useUnistyles();
  const router = useRouter();
  const chat = chatStore(serverId, name);
  const pending = chat.use((s) => s.pending);
  const events = chat.use((s) => s.events);
  const state = chat.use((s) => s.stateEvent?.state ?? 'idle');
  const provider = useSessions((s) => {
    const byServer = s.byServerRecord?.[serverId];
    if (byServer) {
      const hit = byServer.find((x) => x.name === name);
      if (hit?.provider) return hit.provider;
    }
    return (s.rows.find((x) => x.name === name)?.provider ?? null) as string | null;
  });
  const isCodex = provider === 'codex';
  const filaCount = filaCountOf({ events, pending });

  const [text, setText] = useState('');
  const textRef = useRef(text);
  useEffect(() => {
    textRef.current = text;
  }, [text]);
  const [sending, setSending] = useState(false);
  const [uploading, setUploading] = useState(false);
  const [error, setError] = useState('');
  const [transcribing, setTranscribing] = useState(false);
  const [undo, setUndo] = useState<{ before: string; raw: string } | null>(null);
  const [failed, setFailed] = useState<{ file: File; motivo: MotivoFim } | null>(null);
  const [autoN, setAutoN] = useState<number | null>(null);
  const inputRef = useRef<MultiTextInputHandle>(null);
  const undoTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const autoTimerRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const autoAlvoRef = useRef<number>(0);
  const autoTextoRef = useRef<string>('');

  const limparUndo = useCallback(() => {
    setUndo(null);
    if (undoTimerRef.current) {
      clearTimeout(undoTimerRef.current);
      undoTimerRef.current = null;
    }
  }, []);

  const cancelarAuto = useCallback(() => {
    if (autoTimerRef.current) {
      clearInterval(autoTimerRef.current);
      autoTimerRef.current = null;
    }
    setAutoN(null);
    autoAlvoRef.current = 0;
  }, []);

  const handleChangeText = useCallback(
    (v: string) => {
      setText(v);
      if (undo) limparUndo();
      if (autoN !== null) cancelarAuto();
    },
    [undo, autoN, limparUndo, cancelarAuto],
  );

  // draft devolvido pelo cancelar do picker (Task 3): adota quando muda
  useEffect(() => {
    if (draft !== undefined && draft !== text) {
      setText(draft);
      requestAnimationFrame(() => {
        inputRef.current?.setTextAndSelection(draft, { start: draft.length, end: draft.length });
      });
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [draft]);

  const canSend = text.trim().length > 0 && !sending && !uploading;

  const handleSend = useCallback(async () => {
    const trimmed = text.trim();
    if (!trimmed || sending || uploading) return;
    limparUndo();
    cancelarAuto();
    setSending(true);
    setError('');
    setText('');
    try {
      await chat.send(trimmed);
    } catch (e) {
      const msg = e instanceof Error ? e.message : m.composer_falha_envio();
      setError(msg);
      setText((prev) => (prev.trim() ? prev : trimmed));
    } finally {
      setSending(false);
    }
  }, [text, sending, uploading, chat, limparUndo, cancelarAuto]);

  // auto-envio: contagem de 3s
  const iniciarAuto = useCallback(
    (textoParaEnviar: string) => {
      cancelarAuto();
      autoTextoRef.current = textoParaEnviar;
      autoAlvoRef.current = Date.now() + 3000;
      setAutoN(3);
      autoTimerRef.current = setInterval(() => {
        const rest = autoAlvoRef.current - Date.now();
        if (rest <= 0) {
          cancelarAuto();
          // envia como handleSend faz (limpa otimista)
          const toSend = autoTextoRef.current.trim();
          if (!toSend) return;
          setText('');
          limparUndo();
          void chat
            .send(toSend)
            .then(() => setError(''))
            .catch((e: unknown) => {
              const msg = e instanceof Error ? e.message : m.composer_falha_envio();
              setError(msg);
              setText((prev) => (prev.trim() ? prev : toSend));
            });
          return;
        }
        setAutoN(Math.ceil(rest / 1000));
      }, 250);
    },
    [cancelarAuto, chat, limparUndo],
  );

  const handleTranscribe = useCallback(
    async (file: File, motivo: MotivoFim) => {
      if (transcribing) {
        setError(m.composer_aguarde_transcricao());
        return;
      }
      setTranscribing(true);
      setError('');
      setFailed(null);
      try {
        const estilo = useDitadoEstiloStore.getState().pronto
          ? useDitadoEstiloStore.getState().valor
          : undefined;
        const { text: t, raw, aviso } = await transcribeFile(name, file, {
          limpar: true,
          estilo,
        });
        const trimmed = t.trim();
        if (!trimmed) {
          setError(m.composer_transcricao_vazia());
          return;
        }
        const before = textRef.current.trim();
        const next = before ? `${before} ${trimmed}` : trimmed;
        setText(next);
        requestAnimationFrame(() => {
          inputRef.current?.setTextAndSelection(next, { start: next.length, end: next.length });
        });
        if (raw && raw.trim() !== trimmed) {
          setUndo({ before, raw: raw.trim() });
          if (undoTimerRef.current) clearTimeout(undoTimerRef.current);
          undoTimerRef.current = setTimeout(() => limparUndo(), 10_000);
        } else {
          limparUndo();
        }
        if (aviso) {
          setError(aviso);
        }
        const shouldAuto = podeEnviarSozinho({
          motivo,
          texto: trimmed,
          aviso: aviso ?? null,
          rascunhoAntes: before.length > 0,
        });
        if (shouldAuto) {
          iniciarAuto(next);
        }
      } catch (e) {
        setFailed({ file, motivo });
        const msg = e instanceof Error ? e.message : m.composer_falha_transcricao();
        setError(msg);
      } finally {
        setTranscribing(false);
      }
    },
    [name, transcribing, limparUndo, iniciarAuto],
  );

  const { gravando, rms, iniciar, parar } = useDitado({ onFim: handleTranscribe });

  const handleMicPress = useCallback(async () => {
    if (transcribing) return;
    if (gravando) {
      void parar('botao');
      return;
    }
    if (autoN !== null) cancelarAuto();
    try {
      await iniciar();
    } catch (e) {
      const msg = e instanceof Error ? e.message : '';
      if (msg === 'permission_denied') {
        setError(m.composer_sem_acesso_mic());
      } else {
        setError(e instanceof Error ? e.message : m.composer_falha_gravacao());
      }
    }
  }, [gravando, transcribing, iniciar, parar, autoN, cancelarAuto]);

  const handleUndo = useCallback(() => {
    if (!undo) return;
    const { before, raw } = undo;
    const restored = before ? `${before} ${raw}` : raw;
    setText(restored);
    limparUndo();
    requestAnimationFrame(() => {
      inputRef.current?.setTextAndSelection(restored, { start: restored.length, end: restored.length });
    });
  }, [undo, limparUndo]);

  const handleRetry = useCallback(() => {
    if (!failed) return;
    void handleTranscribe(failed.file, failed.motivo);
  }, [failed, handleTranscribe]);

  const handleSteer = useCallback(async () => {
    try {
      await steerSession(name);
    } catch (e) {
      const status = (e as { status?: number } | null)?.status;
      setError(status === 409 ? m.composer_fila_erro() : e instanceof Error ? e.message : m.composer_fila_erro());
    }
  }, [name]);

  const isKimi = provider === 'kimi';
  const showSteer = isKimi && state === 'working' && filaCount > 0;

  const handleKeyPress = useCallback(
    (ev: { key: string; shiftKey: boolean }) => {
      if (ev.key === 'Enter' && !ev.shiftKey && Platform.OS === 'web') {
        void handleSend();
        return true;
      }
      return false;
    },
    [handleSend],
  );

  const handlePickImage = useCallback(async () => {
    setError('');
    try {
      const res = await ImagePicker.launchImageLibraryAsync({
        mediaTypes: ImagePicker.MediaTypeOptions.Images,
        quality: 0.8,
      });
      if (res.canceled || !res.assets?.[0]) return;
      const asset = res.assets[0];
      setUploading(true);
      const blobRes = await fetch(asset.uri);
      const blob = await blobRes.blob();
      const fileName = asset.fileName ?? 'imagem.jpg';
      const mime = asset.mimeType ?? blob.type ?? 'image/jpeg';
      const file = new File([blob], fileName, { type: mime });
      const { path } = await uploadFile(name, file);
      const insert = `📎 ${m.board_imagem()}: ${path}`;
      const next = text.trim() ? `${text.trim()} — ${insert}` : insert;
      setText(next);
      requestAnimationFrame(() => {
        inputRef.current?.setTextAndSelection(next, { start: next.length, end: next.length });
      });
    } catch (e) {
      setError(e instanceof Error ? e.message : m.board_falha_upload());
    } finally {
      setUploading(false);
    }
  }, [name, text]);

  useEffect(() => {
    return () => {
      if (undoTimerRef.current) clearTimeout(undoTimerRef.current);
      if (autoTimerRef.current) clearInterval(autoTimerRef.current);
    };
  }, []);

  return (
    <Glass variant="chrome" style={styles.glass}>
        {/* chip de fila: pending local + queued-* do SSE (contado no store como pending até chegar o real) */}
        {filaCount > 0 ? (
          <View style={styles.filaChip}>
            <Text style={[styles.filaText, { color: theme.tokens.text.secondary }]}>
              ⏳ {m.composer_fila_contagem({ n: filaCount })}
            </Text>
            {showSteer ? (
              <Pressable
                onPress={handleSteer}
                style={[styles.steerBtn, { borderColor: theme.tokens.accent.base }]}
                accessibilityLabel={m.composer_fila_aria()}
                accessibilityRole="button"
              >
                <Text style={[styles.steerText, { color: theme.tokens.accent.base }]}>
                  {m.composer_fila_acao()}
                </Text>
              </Pressable>
            ) : null}
          </View>
        ) : null}

        <ScrollView horizontal showsHorizontalScrollIndicator={false} contentContainerStyle={styles.pillsRow}>
          <View style={styles.pillDuo}>
            <ModelPill serverId={serverId} name={name} />
            <EffortPill serverId={serverId} name={name} />
          </View>
          <PermissionPill serverId={serverId} name={name} />
          {isCodex ? (
            <Pressable
              onPress={() => router.push(`/s/${serverId}/${name}/codex-limits` as never)}
              style={[styles.codexChip, { backgroundColor: theme.tokens.bg.elevated, borderColor: theme.tokens.border.subtle }]}
              accessibilityRole="button"
              accessibilityLabel={m.codex_limites_titulo()}
            >
              <Text style={[styles.codexChipText, { color: theme.tokens.text.primary }]}>{m.codex_limites_titulo()}</Text>
            </Pressable>
          ) : null}
        </ScrollView>

        <View style={styles.row}>
          <View style={styles.inputWrap}>
            <MultiTextInput
              ref={inputRef}
              value={text}
              onChangeText={handleChangeText}
              placeholder={m.composer_mensagem()}
              maxHeight={120}
              onKeyPress={handleKeyPress}
            />
          </View>

          <EstiloPill />

          <Pressable
            onPress={handleMicPress}
            disabled={transcribing || sending}
            style={[
              styles.iconBtn,
              (transcribing || sending) && styles.iconBtnDisabled,
              gravando && { backgroundColor: theme.tokens.status.error, borderColor: theme.tokens.status.error },
            ]}
            accessibilityLabel={gravando ? m.composer_parar_gravacao() : m.composer_gravar_audio()}
            accessibilityRole="button"
          >
            <Text style={[styles.iconGlyph, { color: gravando ? '#fff' : theme.tokens.text.secondary }]}>
              {gravando ? '■' : '🎤'}
            </Text>
          </Pressable>

          <Pressable
            onPress={handlePickImage}
            disabled={uploading || sending || gravando}
            style={[styles.iconBtn, (uploading || gravando) && styles.iconBtnDisabled]}
            accessibilityLabel={m.composer_anexar_arquivo()}
            accessibilityRole="button"
          >
            {uploading ? (
              <ActivityIndicator size="small" color={theme.tokens.text.secondary} />
            ) : (
              <Text style={[styles.iconGlyph, { color: theme.tokens.text.secondary }]}>📎</Text>
            )}
          </Pressable>

          <Pressable
            onPress={handleSend}
            disabled={!canSend}
            style={[styles.sendBtn, { backgroundColor: theme.tokens.accent.base }, !canSend && styles.sendBtnDisabled]}
            accessibilityLabel={m.composer_enviar_mensagem()}
            accessibilityRole="button"
          >
            <Text style={[styles.sendGlyph, { color: theme.tokens.text.inverse }]}>↑</Text>
          </Pressable>
        </View>

        {gravando ? (
          <View style={[styles.rmsTrack, { backgroundColor: theme.tokens.bg.elevated, borderColor: theme.tokens.border.subtle }]} accessibilityLabel={m.composer_gravando_audio()}>
            <View style={[styles.rmsFill, { width: `${Math.round(Math.min(1, rms) * 100)}%`, backgroundColor: theme.tokens.accent.base }]} />
          </View>
        ) : null}

        {transcribing ? (
          <Text style={[styles.hint, { color: theme.tokens.text.muted }]}>{m.composer_transcrevendo_audio()}</Text>
        ) : null}

        {autoN !== null ? (
          <Pressable onPress={cancelarAuto} style={[styles.autoChip, { backgroundColor: theme.tokens.bg.elevated, borderColor: theme.tokens.border.subtle }]} accessibilityRole="button">
            <Text style={[styles.autoText, { color: theme.tokens.text.primary }]}>{m.composer_enviando_cancelar({ n: autoN })}</Text>
          </Pressable>
        ) : null}

        {undo ? (
          <View style={styles.undoRow}>
            <Text style={[styles.hint, { color: theme.tokens.text.muted }]}>{m.composer_ditado_limpo()}</Text>
            <Pressable onPress={handleUndo} style={[styles.undoBtn, { borderColor: theme.tokens.border.subtle }]} accessibilityRole="button">
              <Text style={[styles.undoText, { color: theme.tokens.accent.base }]}>{m.composer_desfazer_limpeza()}</Text>
            </Pressable>
          </View>
        ) : null}

        {error ? (
          <View style={styles.errorRow}>
            <Text style={[styles.error, { color: theme.tokens.status.error }]}>{error}</Text>
            {failed ? (
              <Pressable onPress={handleRetry} style={[styles.retryBtn, { borderColor: theme.tokens.border.subtle }]} accessibilityRole="button">
                <Text style={[styles.retryText, { color: theme.tokens.accent.base }]}>{m.composer_transcrever_de_novo()}</Text>
              </Pressable>
            ) : null}
          </View>
        ) : null}

        {/* hint sutil do estado working: quando há pending/queued, já há chip; este texto só aparece em working sem fila */}
        {state === 'working' && filaCount === 0 && !gravando && !transcribing ? (
          <Text style={[styles.hint, { color: theme.tokens.text.muted }]}>{m.composer_sessao_trabalhando()}</Text>
        ) : null}
    </Glass>
  );
}

const styles = StyleSheet.create((theme) => ({
  glass: {
    marginHorizontal: theme.base.space[2],
    marginBottom: theme.base.space[2],
    padding: theme.base.space[2],
    gap: theme.base.space[2],
  },
  filaChip: {
    alignSelf: 'flex-start',
    flexDirection: 'row',
    alignItems: 'center',
    gap: theme.base.space[2],
    backgroundColor: theme.tokens.bg.elevated,
    borderRadius: theme.base.radius.full,
    paddingHorizontal: theme.base.space[2],
    paddingVertical: 4,
    borderWidth: 1,
    borderColor: theme.tokens.border.subtle,
  },
  filaText: {
    fontSize: theme.base.text.xs,
    fontWeight: '500',
  },
  steerBtn: {
    borderWidth: 1,
    borderRadius: theme.base.radius.full,
    paddingHorizontal: theme.base.space[2],
    paddingVertical: 4,
  },
  steerText: {
    fontSize: theme.base.text.xs,
    fontWeight: '700',
  },
  pillsRow: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: theme.base.space[2],
    paddingVertical: 2,
  },
  pillDuo: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: theme.base.space[1],
  },
  codexChip: {
    borderWidth: 1,
    borderRadius: theme.base.radius.full,
    paddingHorizontal: theme.base.space[2],
    paddingVertical: 6,
    minHeight: 32,
    justifyContent: 'center',
  },
  codexChipText: {
    fontSize: theme.base.text.xs,
    fontWeight: '600',
  },
  row: {
    flexDirection: 'row',
    alignItems: 'flex-end',
    gap: theme.base.space[2],
  },
  inputWrap: {
    flex: 1,
    minHeight: 44,
    justifyContent: 'center',
    backgroundColor: theme.tokens.bg.surface,
    borderRadius: theme.base.radius.lg,
    borderWidth: 1,
    borderColor: theme.tokens.border.subtle,
    paddingHorizontal: theme.base.space[2],
    paddingVertical: 6,
  },
  iconBtn: {
    width: 44,
    height: 44,
    borderRadius: theme.base.radius.full,
    alignItems: 'center',
    justifyContent: 'center',
    backgroundColor: theme.tokens.bg.elevated,
    borderWidth: 1,
    borderColor: theme.tokens.border.subtle,
  },
  iconBtnDisabled: {
    opacity: 0.5,
  },
  iconGlyph: {
    fontSize: 18,
    lineHeight: 22,
  },
  sendBtn: {
    width: 44,
    height: 44,
    borderRadius: theme.base.radius.full,
    alignItems: 'center',
    justifyContent: 'center',
  },
  sendBtnDisabled: {
    opacity: 0.4,
  },
  sendGlyph: {
    fontSize: 20,
    fontWeight: '700',
    lineHeight: 22,
  },
  rmsTrack: {
    height: 6,
    borderRadius: 3,
    overflow: 'hidden',
    borderWidth: 1,
  },
  rmsFill: {
    height: '100%',
    borderRadius: 3,
  },
  autoChip: {
    alignSelf: 'flex-start',
    borderWidth: 1,
    borderRadius: theme.base.radius.full,
    paddingHorizontal: theme.base.space[3],
    paddingVertical: 6,
  },
  autoText: {
    fontSize: theme.base.text.xs,
    fontWeight: '600',
  },
  undoRow: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: theme.base.space[2],
  },
  undoBtn: {
    borderWidth: 1,
    borderRadius: theme.base.radius.full,
    paddingHorizontal: theme.base.space[2],
    paddingVertical: 4,
  },
  undoText: {
    fontSize: theme.base.text.xs,
    fontWeight: '600',
  },
  error: {
    fontSize: theme.base.text.xs,
    paddingHorizontal: theme.base.space[1],
  },
  errorRow: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: theme.base.space[2],
    flexWrap: 'wrap',
  },
  retryBtn: {
    borderWidth: 1,
    borderRadius: theme.base.radius.full,
    paddingHorizontal: theme.base.space[2],
    paddingVertical: 4,
  },
  retryText: {
    fontSize: theme.base.text.xs,
    fontWeight: '600',
  },
  hint: {
    fontSize: theme.base.text.xs,
    paddingHorizontal: theme.base.space[1],
  },
}));
