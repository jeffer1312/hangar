import { useEffect, useState } from 'react';
import { Pressable, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { useAudioPlayer, useAudioPlayerStatus } from 'expo-audio';
import * as m from '../paraglide/messages';
import { Icon } from '../ui/Icon';
import { superficie } from '../theme/superficie';
import { useVoiceAudioState } from '../features/voice/audioSession';

const mmss = (s: number) => `${Math.floor(s / 60)}:${String(Math.floor(s % 60)).padStart(2, '0')}`;

// Tocador na própria bolha, como o <audio controls> do web. A fonte só entra no primeiro toque:
// a lista monta várias bolhas, e cada uma baixaria o áudio sem ninguém pedir.
export function AudioChip({ uri, headers, name, onRemove, disabled = false, meta }: {
  uri: string; headers?: Record<string, string>; name: string; onRemove?: () => void; disabled?: boolean;
  /** Linha de detalhe (tamanho · origem); depois do primeiro toque o tempo de reprodução vem na frente. */
  meta?: string;
}) {
  const { theme } = useUnistyles();
  const player = useAudioPlayer(null);
  const status = useAudioPlayerStatus(player);
  const [carregado, setCarregado] = useState(false);
  const voiceActive = useVoiceAudioState((state) => state.voiceActive);
  const fim = status.duration > 0 && status.currentTime >= status.duration - 0.1;

  useEffect(() => {
    if (useVoiceAudioState.getState().voiceActive) player.pause();
    return useVoiceAudioState.subscribe((state, previous) => {
      if (state.voiceActive && !previous.voiceActive) player.pause();
    });
  }, [player]);

  useEffect(() => {
    if (disabled) player.pause();
  }, [disabled, player]);

  const alternar = () => {
    if (disabled || useVoiceAudioState.getState().voiceActive) return;
    if (status.playing) return player.pause();
    if (!carregado) {
      player.replace({ uri, headers });
      setCarregado(true);
    } else if (fim) {
      void player.seekTo(0);
    }
    player.play();
  };

  return (
    <View style={styles.wrap}>
      <Pressable
        onPress={alternar}
        style={styles.btn}
        accessibilityRole="button"
        disabled={voiceActive || disabled}
        accessibilityState={{ disabled: voiceActive || disabled }}
        accessibilityLabel={status.playing ? m.anexos_pausar({ nome: name }) : m.anexos_tocar({ nome: name })}
      >
        <Icon name={status.playing ? 'Pause' : 'Play'} size={16} color={theme.tokens.accent.base} />
      </Pressable>
      <View style={styles.info}>
        <Text style={[styles.name, { color: theme.tokens.text.primary }]} numberOfLines={1}>{name}</Text>
        {status.error ? (
          <Text style={[styles.meta, { color: theme.tokens.status.error }]} accessibilityRole="alert" numberOfLines={2}>
            {`${m.visor_nao_carregou()} (${status.error})`}
          </Text>
        ) : carregado ? (
          <Text style={[styles.meta, { color: theme.tokens.text.muted }]} numberOfLines={1}>
            {[status.duration > 0 ? `${mmss(status.currentTime)} / ${mmss(status.duration)}` : m.comum_carregando(), meta].filter(Boolean).join(' · ')}
          </Text>
        ) : meta ? (
          <Text style={[styles.meta, { color: theme.tokens.text.muted }]} numberOfLines={1}>{meta}</Text>
        ) : null}
      </View>
      {onRemove ? (
        <Pressable onPress={() => { player.pause(); onRemove(); }} disabled={disabled} style={styles.btn}
          accessibilityRole="button" accessibilityLabel={`${m.board_remover_anexo()}: ${name}`}
          accessibilityState={{ disabled }}>
          <Icon name="X" size={16} color={theme.tokens.text.secondary} />
        </Pressable>
      ) : null}
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  wrap: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: theme.base.space[2],
    paddingRight: theme.base.space[1],
    flexShrink: 1,
    borderRadius: theme.base.radius.md,
    borderWidth: 1,
    borderColor: theme.tokens.border.subtle,
    backgroundColor: superficie(theme),
    maxWidth: 320,
    minWidth: 180,
  },
  btn: { width: 44, height: 44, alignItems: 'center', justifyContent: 'center' },
  info: { flex: 1, minWidth: 0, paddingVertical: theme.base.space[1] },
  name: { fontSize: theme.base.text.sm, fontFamily: theme.base.fontMono },
  meta: { fontSize: theme.base.text.xs },
}));
