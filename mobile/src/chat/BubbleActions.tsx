import { Pressable, Share, Text, View } from 'react-native';
import * as Clipboard from 'expo-clipboard';
import { createAudioPlayer } from 'expo-audio';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { sintetizarTts, ttsAudioUrl } from '@hangar/core';
import { Icon } from '../ui/Icon';
import { toast } from '../ui/Toast';
import * as m from '../paraglide/messages';

function hora(ts?: number | null) {
  return ts ? new Date(ts * 1000).toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' }) : '';
}

// Hora + copiar/compartilhar/ouvir. Compartilhar usa o Share do RN (texto puro, sem lib). Ouvir
// chama o mesmo POST /api/tts da PWA e toca a URL devolvida.
export function BubbleActions({ text, ts, ouvir = true }: { text: string; ts?: number | null; ouvir?: boolean }) {
  const { theme } = useUnistyles();
  const c = theme.tokens.text.muted;
  const copiar = async () => {
    await Clipboard.setStringAsync(text);
    toast.ok(m.toast_copiado());
  };
  const compartilhar = () => Share.share({ message: text }).catch(() => {});
  const falar = async () => {
    try {
      const r = await sintetizarTts({ text });
      createAudioPlayer(ttsAudioUrl(r.url)).play();
    } catch (e) {
      toast.erro(e instanceof Error ? e.message : String(e));
    }
  };
  return (
    <View style={styles.row}>
      <Text style={[styles.hora, { color: c }]}>{hora(ts)}</Text>
      <Pressable onPress={() => void copiar()} hitSlop={8} accessibilityRole="button" accessibilityLabel={m.bubble_copiar()}>
        <Icon name="Copy" size={13} color={c} />
      </Pressable>
      <Pressable onPress={compartilhar} hitSlop={8} accessibilityRole="button" accessibilityLabel={m.bolha_compartilhar()}>
        <Icon name="Share2" size={13} color={c} />
      </Pressable>
      {ouvir ? (
        <Pressable onPress={() => void falar()} hitSlop={8} accessibilityRole="button" accessibilityLabel={m.bubble_ouvir()}>
          <Icon name="Volume2" size={13} color={c} />
        </Pressable>
      ) : null}
    </View>
  );
}

const styles = StyleSheet.create(() => ({
  row: { flexDirection: 'row', alignItems: 'center', gap: 14, paddingHorizontal: 6, paddingTop: 4 },
  hora: { fontSize: 11, marginRight: 'auto' },
}));
