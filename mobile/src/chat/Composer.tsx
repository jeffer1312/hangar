import { useCallback, useEffect, useRef, useState } from 'react';
import { ActivityIndicator, Platform, Pressable, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import * as ImagePicker from 'expo-image-picker';
import { uploadFile } from '@hangar/core';
import { Glass } from '../ui/Glass';
import { MultiTextInput, type MultiTextInputHandle } from '../vendor/happy/components/MultiTextInput';
import * as m from '../paraglide/messages';
import { chatStore, filaCount as filaCountOf } from '../stores/chat';

interface Props {
  serverId: string;
  name: string;
  draft?: string;
}

export function Composer({ serverId, name, draft }: Props) {
  const { theme } = useUnistyles();
  const chat = chatStore(serverId, name);
  const pending = chat.use((s) => s.pending);
  const events = chat.use((s) => s.events);
  const state = chat.use((s) => s.stateEvent?.state ?? 'idle');
  // fila total = ecos locais + sintéticos queued-* da fila durável (até o real chegar)
  const filaCount = filaCountOf({ events, pending });

  const [text, setText] = useState('');
  const [sending, setSending] = useState(false);
  const [uploading, setUploading] = useState(false);
  const [error, setError] = useState('');
  const inputRef = useRef<MultiTextInputHandle>(null);

  // draft devolvido pelo cancelar do picker (Task 3): adota quando muda
  useEffect(() => {
    if (draft !== undefined && draft !== text) {
      setText(draft);
      // move caret pro fim após o value controlado ser aplicado
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
    setSending(true);
    setError('');
    // limpa otimista pra tirar sensação de lag — só via `value` controlado; sem
    // setTextAndSelection pra não brigar com o Enter que ainda vai inserir quebra
    setText('');
    try {
      await chat.send(trimmed);
    } catch (e) {
      // falhou -> devolve o texto se a caixa segue vazia (só controlled, sem imperativo)
      const msg = e instanceof Error ? e.message : m.composer_falha_envio();
      setError(msg);
      setText((prev) => (prev.trim() ? prev : trimmed));
    } finally {
      setSending(false);
    }
  }, [text, sending, uploading, chat]);

  const handleKeyPress = useCallback(
    (ev: { key: string; shiftKey: boolean }) => {
      // Enter envia só no desktop/web com teclado físico; no celular Enter quebra linha
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
      // asset.uri = file:// no nativo -> fetch pra blob
      const blobRes = await fetch(asset.uri);
      const blob = await blobRes.blob();
      const fileName = asset.fileName ?? 'imagem.jpg';
      const mime = asset.mimeType ?? blob.type ?? 'image/jpeg';
      const file = new File([blob], fileName, { type: mime });
      const { path } = await uploadFile(name, file);
      const insert = `📎 ${m.board_imagem()}: ${path}`;
      const next = text.trim() ? `${text.trim()} — ${insert}` : insert;
      setText(next);
      // move caret pro fim
      requestAnimationFrame(() => {
        inputRef.current?.setTextAndSelection(next, { start: next.length, end: next.length });
      });
    } catch (e) {
      setError(e instanceof Error ? e.message : m.board_falha_upload());
    } finally {
      setUploading(false);
    }
  }, [name, text]);

  return (
    <Glass variant="chrome" style={styles.glass}>
        {/* chip de fila: pending local + queued-* do SSE (contado no store como pending até chegar o real) */}
        {filaCount > 0 ? (
          <View style={styles.filaChip}>
            <Text style={[styles.filaText, { color: theme.tokens.text.secondary }]}>
              ⏳ {m.composer_fila_contagem({ n: filaCount })}
            </Text>
          </View>
        ) : null}

        <View style={styles.row}>
          <View style={styles.inputWrap}>
            <MultiTextInput
              ref={inputRef}
              value={text}
              onChangeText={setText}
              placeholder={m.composer_mensagem()}
              maxHeight={120}
              onKeyPress={handleKeyPress}
            />
          </View>

          <Pressable
            onPress={handlePickImage}
            disabled={uploading || sending}
            style={[styles.iconBtn, uploading && styles.iconBtnDisabled]}
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

        {error ? (
          <Text style={[styles.error, { color: theme.tokens.status.error }]}>{error}</Text>
        ) : null}

        {/* hint sutil do estado working: quando há pending/queued, já há chip; este texto só aparece em working sem fila */}
        {state === 'working' && filaCount === 0 ? (
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
  error: {
    fontSize: theme.base.text.xs,
    paddingHorizontal: theme.base.space[1],
  },
  hint: {
    fontSize: theme.base.text.xs,
    paddingHorizontal: theme.base.space[1],
  },
}));
