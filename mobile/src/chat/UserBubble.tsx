import { memo, useState } from 'react';
import { Pressable, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { Image } from 'expo-image';
import { parseImageMessage, parseRealtimeDelegation, uploadUrlNative, fileAuthHeader } from '@hangar/core';
import * as m from '../paraglide/messages';
import { BubbleActions } from './BubbleActions';
import { superficie } from '../theme/superficie';

// Bolha do usuário: alinhada à direita, cor bubbleUser do tema (espelho do app.css).
// Se parseImageMessage(text) não nulo → legenda + miniaturas (uploadUrl/fileUrl).
export const UserBubble = memo(function UserBubble({ text, sessionName, ts }: { text: string; sessionName?: string; ts?: number | null }) {
  const { theme } = useUnistyles();
  const voice = parseRealtimeDelegation(text);
  const [showOriginal, setShowOriginal] = useState(false);
  const parsed = parseImageMessage(text);
  const hasImages = !!parsed && !!sessionName;
  const caption = hasImages ? parsed!.caption : '';
  const filenames = hasImages ? parsed!.filenames : [];

  if (voice) {
    return (
      <View style={[styles.bubble, styles.voice]}>
        <Text style={styles.voiceLabel}>{m.voice_message_origin()}</Text>
        <Text style={[styles.txt, { color: theme.tokens.text.primary }]} selectable>{voice.input}</Text>
        <Pressable accessibilityRole="button" accessibilityState={{ expanded: showOriginal }}
          onPress={() => setShowOriginal(value => !value)} style={styles.detailsButton}>
          <Text style={{ color: theme.tokens.text.secondary }}>{m.voice_message_original()}</Text>
        </Pressable>
        {showOriginal ? <Text style={[styles.txt, { color: theme.tokens.text.secondary }]} selectable>{text}</Text> : null}
        <BubbleActions text={voice.input} ts={ts} ouvir={false} />
      </View>
    );
  }

  // Se há imagens válidas, exibe legenda + thumbnails; senão fallback texto cru
  if (hasImages) {
    // filenames vazios = foto única absorvida como anexo real: mostra só legenda (fallback)
    if (filenames.length === 0) {
      // Ainda é mensagem de imagem, mas sem miniatura disponível: mostra legenda se houver, senão texto original
      const display = caption || text;
      return (
        <View style={styles.bubble}>
          <Text style={[styles.txt, { color: theme.tokens.text.primary }]} selectable>
            {display}
          </Text>
          <BubbleActions text={display} ts={ts} ouvir={false} />
        </View>
      );
    }
    return (
      <View style={styles.bubble}>
        {caption ? (
          <Text style={[styles.txt, { color: theme.tokens.text.primary }]} selectable>
            {caption}
          </Text>
        ) : null}
        <View style={styles.thumbs}>
          {filenames.map((fn) => {
            const uri = uploadUrlNative(sessionName!, fn);
            const headers = fileAuthHeader();
            return <Image key={fn} source={{ uri, headers }} style={styles.thumb} contentFit="cover" transition={150} />;
          })}
        </View>
        {/* Se a legenda estava vazia e o texto original era só marcador, já mostramos thumbs */}
        <BubbleActions text={caption || text} ts={ts} ouvir={false} />
      </View>
    );
  }

  return (
    <View style={styles.bubble}>
      <Text style={[styles.txt, { color: theme.tokens.text.primary }]} selectable>
        {text}
      </Text>
      <BubbleActions text={text} ts={ts} ouvir={false} />
    </View>
  );
});

const styles = StyleSheet.create((theme) => ({
  bubble: {
    alignSelf: 'flex-end',
    maxWidth: '85%',
    backgroundColor: theme.tokens.bubbleUser,
    borderRadius: theme.base.radius.lg,
    paddingHorizontal: theme.base.space[3],
    paddingVertical: theme.base.space[2],
    gap: theme.base.space[2],
  },
  txt: {
    fontSize: theme.base.text.base,
  },
  voice: { alignSelf: 'flex-start', borderWidth: 1, borderColor: theme.tokens.accent.base },
  voiceLabel: { color: theme.tokens.accent.base, fontWeight: '600', fontSize: theme.base.text.sm },
  detailsButton: { minHeight: 44, justifyContent: 'center' },
  thumbs: {
    flexDirection: 'row',
    flexWrap: 'wrap',
    gap: theme.base.space[2],
  },
  thumb: {
    width: 96,
    height: 96,
    borderRadius: theme.base.radius.md,
    backgroundColor: superficie(theme, 0.8),
  },
}));
