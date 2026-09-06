import { useEffect, useRef } from 'react';
import { Pressable, Share, Text, View } from 'react-native';
import * as Clipboard from 'expo-clipboard';
import { createAudioPlayer, type AudioPlayer } from 'expo-audio';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { sintetizarTts, ttsAudioUrl } from '@hangar/core';
import { Icon } from '../ui/Icon';
import { toast } from '../ui/Toast';
import * as m from '../paraglide/messages';

function hora(ts?: number | null) {
  return ts ? new Date(ts * 1000).toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' }) : '';
}

// UM tocador por vez, no módulo: o AudioPlayer é objeto nativo que só some com `remove()`, e o
// "Ouvir" de outra bolha tem que interromper o anterior em vez de sobrepor as duas vozes.
let tocando: AudioPlayer | null = null;
function pararTocando() {
  tocando?.remove();
  tocando = null;
}

/** Sair da conversa cala a voz: o tocador é do módulo, e sem isto ele seguiria tocando sem dono. */
export function pararTts() {
  pararTocando();
}

// O `addListener` vem do EventEmitter que o AudioPlayer herda, mas o expo-modules-core está
// aninhado em expo/node_modules e o TS não resolve a classe base daqui — só a assinatura que
// usamos, então, em vez do tipo inteiro.
type ComListener = {
  addListener(evento: 'playbackStatusUpdate', fn: (s: { didJustFinish: boolean; error: string | null }) => void): unknown;
};

// Hora + copiar/compartilhar/ouvir. Compartilhar usa o Share do RN (texto puro, sem lib). Ouvir
// chama o mesmo POST /api/tts da PWA e toca a URL devolvida.
export function BubbleActions({ text, ts, ouvir = true }: { text: string; ts?: number | null; ouvir?: boolean }) {
  const { theme } = useUnistyles();
  const c = theme.tokens.text.muted;
  // A lista é virtualizada: a bolha pode sair da tela antes de o /api/tts responder.
  const vivo = useRef(true);
  useEffect(() => () => { vivo.current = false; }, []);

  const copiar = async () => {
    try {
      await Clipboard.setStringAsync(text);
      toast.ok(m.toast_copiado());
    } catch (e) {
      toast.erro(e instanceof Error ? e.message : String(e));
    }
  };
  // Cancelar resolve com `dismissedAction`; rejeição aqui é falha de verdade.
  const compartilhar = async () => {
    try {
      await Share.share({ message: text });
    } catch (e) {
      toast.erro(e instanceof Error ? e.message : String(e));
    }
  };
  const falar = async () => {
    try {
      const r = await sintetizarTts({ text });
      if (!vivo.current) return;
      pararTocando();
      const p = createAudioPlayer(ttsAudioUrl(r.url));
      tocando = p;
      // A URL pode responder 404, ou o áudio não decodificar: sem olhar o `error` do status, a
      // pessoa toca "Ouvir" e não acontece som nenhum, sem aviso.
      (p as unknown as ComListener).addListener('playbackStatusUpdate', (s) => {
        if (tocando !== p) return;
        if (s.error) {
          pararTocando();
          toast.erro(s.error);
        } else if (s.didJustFinish) pararTocando();
      });
      p.play();
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
      <Pressable onPress={() => void compartilhar()} hitSlop={8} accessibilityRole="button" accessibilityLabel={m.bolha_compartilhar()}>
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
