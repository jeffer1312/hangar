import { useCallback, useEffect, useRef, useState } from 'react';
import { ActivityIndicator, Platform, Pressable, ScrollView, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import * as ImagePicker from 'expo-image-picker';
import * as DocumentPicker from 'expo-document-picker';
import { Image } from 'expo-image';
import { broadcast, formataErro, uploadFile, transcribeFile, steerSession, podeEnviarSozinho } from '@hangar/core';
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
import { PillMenu } from '../features/pills/PillMenu';

interface Props {
  serverId: string;
  name: string;
  draft?: string;
}

type PendingAttach = {
  uri: string;
  name: string;
  mime: string;
  kind: 'image' | 'file';
  size?: number;
};

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
  const pairPeers = useSessions((s) => {
    const byServer = s.byServerRecord?.[serverId];
    return byServer?.find((x) => x.name === name)?.pair_peers ?? s.rows.find((x) => x.name === name)?.pair_peers ?? null;
  });
  const pairPeersKey = pairPeers?.join('\u0000') ?? '';
  const isCodex = provider === 'codex';
  const filaCount = filaCountOf({ events, pending });

  const [sendToPair, setSendToPair] = useState(false);
  useEffect(() => {
    setSendToPair(false);
  }, [pairPeersKey]);

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
  const [attachMenuOpen, setAttachMenuOpen] = useState(false);
  const [pendingAttach, setPendingAttach] = useState<PendingAttach | null>(null);
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

  const canSend = (text.trim().length > 0 || pendingAttach !== null) && !sending && !uploading;

  const handleSend = useCallback(async () => {
    const trimmed = text.trim();
    const hasAttach = pendingAttach !== null;
    if (!trimmed && !hasAttach) return;
    if (sending || uploading) return;
    limparUndo();
    cancelarAuto();
    setSending(true);
    setError('');
    // upload do anexo pendente antes de enviar
    let finalText = trimmed;
    let toClearAttach = false;
    if (hasAttach && pendingAttach) {
      setUploading(true);
      try {
        const cur = pendingAttach;
        const blobRes = await fetch(cur.uri);
        const blob = await blobRes.blob();
        const file = new File([blob], cur.name, { type: cur.mime });
        const { path } = await uploadFile(name, file);
        const insert = `📎 ${cur.kind === 'image' ? m.board_imagem() : m.board_arquivo()}: ${path}`;
        finalText = trimmed ? `${trimmed} — ${insert}` : insert;
        toClearAttach = true;
      } catch (e) {
        setError(e instanceof Error ? e.message : m.board_falha_upload());
        setSending(false);
        setUploading(false);
        return;
      } finally {
        setUploading(false);
      }
    }
    if (!finalText.trim()) {
      setSending(false);
      return;
    }
    setText('');
    if (toClearAttach) setPendingAttach(null);
    const sendToPairNow = sendToPair && !!pairPeers?.length && !finalText.trimStart().startsWith('/');
    let groupPendingId: string | null = null;
    try {
      if (sendToPairNow && pairPeers?.length) {
        const recipients = [name, ...pairPeers];
        groupPendingId = `pending-group-${Date.now()}`;
        chat.use.setState((current) => ({ pending: [...current.pending, { id: groupPendingId!, text: finalText }] }));
        const results = await broadcast(recipients, finalText);
        const failedRecipients = recipients.filter((recipient) => !results[recipient]?.ok);
        if (failedRecipients.length) {
          const delivered = recipients.filter((recipient) => results[recipient]?.ok);
          const detail = failedRecipients.map((recipient) => formataErro(results[recipient]?.error)).find(Boolean);
          throw new Error(
            `${delivered.length ? m.chat_chegou_mas({ n: delivered.join(', ') }) : ''}${m.chat_nao_chegou_em()}${failedRecipients.join(', ')} (${detail ?? m.board_falha_envio()})`,
          );
        }
      } else {
        await chat.send(finalText);
      }
    } catch (e) {
      if (groupPendingId) {
        chat.use.setState((current) => ({ pending: current.pending.filter((item) => item.id !== groupPendingId) }));
      }
      const msg = e instanceof Error ? e.message : m.composer_falha_envio();
      setError(msg);
      setText((prev) => (prev.trim() ? prev : finalText));
      if (toClearAttach) {
        // mantém o anexo pra tentar de novo? recoloca se falhou o envio mas upload já foi
        // upload já ocorreu, path está em finalText; recolocar pending seria duplicar
      }
    } finally {
      setSending(false);
    }
  }, [text, sending, uploading, chat, limparUndo, cancelarAuto, pendingAttach, name, pairPeers, sendToPair]);

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
    setAttachMenuOpen(false);
    setError('');
    try {
      const res = await ImagePicker.launchImageLibraryAsync({
        mediaTypes: ImagePicker.MediaTypeOptions.Images,
        quality: 0.8,
      });
      if (res.canceled || !res.assets?.[0]) return;
      const asset = res.assets[0];
      setPendingAttach({
        uri: asset.uri,
        name: asset.fileName ?? 'imagem.jpg',
        mime: asset.mimeType ?? 'image/jpeg',
        kind: 'image',
        size: asset.fileSize,
      });
    } catch (e) {
      setError(e instanceof Error ? e.message : m.board_falha_upload());
    }
  }, []);

  const handlePickFile = useCallback(async () => {
    setAttachMenuOpen(false);
    setError('');
    try {
      const res = await DocumentPicker.getDocumentAsync({ type: '*/*', copyToCacheDirectory: true });
      if (res.canceled) return;
      const asset = (res as unknown as { assets: { uri: string; name: string; mimeType?: string; size?: number }[] }).assets?.[0];
      if (!asset) {
        const single = res as unknown as { uri: string; name: string; mimeType?: string; size?: number };
        if (!single.uri) return;
        const isImg = /\.(png|jpe?g|gif|webp|bmp|svg|avif)$/i.test(single.name ?? '');
        setPendingAttach({
          uri: single.uri,
          name: single.name ?? 'arquivo',
          mime: single.mimeType ?? 'application/octet-stream',
          kind: isImg ? 'image' : 'file',
          size: single.size,
        });
        return;
      }
      const isImg = /\.(png|jpe?g|gif|webp|bmp|svg|avif)$/i.test(asset.name ?? '');
      setPendingAttach({
        uri: asset.uri,
        name: asset.name ?? 'arquivo',
        mime: asset.mimeType ?? 'application/octet-stream',
        kind: isImg ? 'image' : 'file',
        size: asset.size,
      });
    } catch (e) {
      setError(e instanceof Error ? e.message : m.board_falha_upload());
    }
  }, []);

  const handleRemoveAttach = useCallback(() => {
    setPendingAttach(null);
  }, []);

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
          {pairPeers?.length ? (
            <Pressable
              onPress={() => setSendToPair((current) => !current)}
              style={[styles.pairChip, { borderColor: sendToPair ? theme.tokens.accent.base : theme.tokens.border.subtle, backgroundColor: sendToPair ? theme.tokens.bg.elevated : 'transparent' }]}
              accessibilityRole="switch"
              accessibilityState={{ checked: sendToPair }}
              accessibilityLabel={m.composer_mandar_grupo()}
              accessibilityHint={sendToPair ? m.composer_mandando_grupo() : m.composer_mandar_tambem({ n: pairPeers.join(', ') })}
            >
              <Text style={[styles.pairChipText, { color: sendToPair ? theme.tokens.accent.base : theme.tokens.text.secondary }]}>⇄</Text>
              <Text style={[styles.pairChipLabel, { color: sendToPair ? theme.tokens.accent.base : theme.tokens.text.secondary }]} numberOfLines={1}>
                {sendToPair ? (pairPeers.length === 1 ? m.composer_pros_dois() : m.composer_pro_grupo()) : m.composer_mandar_tambem({ n: pairPeers.join(', ') })}
              </Text>
            </Pressable>
          ) : null}
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

        {pendingAttach ? (
          <View style={[styles.attachPreview, { backgroundColor: theme.tokens.bg.elevated, borderColor: theme.tokens.border.subtle }]}>
            {pendingAttach.kind === 'image' ? (
              <Image source={{ uri: pendingAttach.uri }} style={styles.attachThumb} contentFit="cover" transition={150} />
            ) : (
              <View style={[styles.attachFileIcon, { backgroundColor: theme.tokens.bg.surface }]}>
                <Text style={styles.attachFileIco}>📎</Text>
              </View>
            )}
            <View style={styles.attachInfo}>
              <Text style={[styles.attachName, { color: theme.tokens.text.primary }]} numberOfLines={1}>
                {pendingAttach.name}
              </Text>
              {pendingAttach.size ? (
                <Text style={[styles.attachMeta, { color: theme.tokens.text.muted }]}>{Math.round(pendingAttach.size / 1024)} KB</Text>
              ) : null}
            </View>
            <Pressable
              onPress={handleRemoveAttach}
              style={[styles.attachRemove, { borderColor: theme.tokens.border.subtle }]}
              accessibilityLabel={m.board_remover_anexo()}
              accessibilityRole="button"
            >
              <Text style={[styles.attachRemoveTxt, { color: theme.tokens.text.secondary }]}>✕</Text>
            </Pressable>
          </View>
        ) : null}

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
            onPress={() => setAttachMenuOpen(true)}
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

        <PillMenu
          open={attachMenuOpen}
          onClose={() => setAttachMenuOpen(false)}
          title={m.composer_anexar_arquivo()}
          items={[{ label: m.board_imagem() }, { label: m.board_arquivo() }]}
          onSelect={(it) => {
            if (it.label === m.board_imagem()) void handlePickImage();
            else void handlePickFile();
          }}
        />
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
  pairChip: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: theme.base.space[1],
    minHeight: 32,
    maxWidth: 220,
    borderWidth: 1,
    borderRadius: theme.base.radius.full,
    paddingHorizontal: theme.base.space[2],
  },
  pairChipText: {
    fontSize: theme.base.text.sm,
    fontWeight: '700',
  },
  pairChipLabel: {
    flexShrink: 1,
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
  attachPreview: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: theme.base.space[2],
    padding: theme.base.space[2],
    borderRadius: theme.base.radius.md,
    borderWidth: 1,
  },
  attachThumb: {
    width: 48,
    height: 48,
    borderRadius: theme.base.radius.sm,
    backgroundColor: theme.tokens.bg.surface,
  },
  attachFileIcon: {
    width: 48,
    height: 48,
    borderRadius: theme.base.radius.sm,
    alignItems: 'center',
    justifyContent: 'center',
  },
  attachFileIco: {
    fontSize: 22,
  },
  attachInfo: {
    flex: 1,
    gap: 2,
  },
  attachName: {
    fontSize: theme.base.text.sm,
    fontWeight: '600',
  },
  attachMeta: {
    fontSize: theme.base.text.xs,
  },
  attachRemove: {
    width: 32,
    height: 32,
    borderRadius: 16,
    borderWidth: 1,
    alignItems: 'center',
    justifyContent: 'center',
  },
  attachRemoveTxt: {
    fontSize: 14,
    fontWeight: '700',
  },
}));
