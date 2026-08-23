import { Pressable, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { Image } from 'expo-image';
import { parseImageMessage, uploadUrl } from '@hangar/core';

// Bolha do usuário: alinhada à direita, cor bubbleUser do tema (espelho do app.css).
// Se parseImageMessage(text) não nulo → legenda + miniaturas (uploadUrl/fileUrl).
export function UserBubble({ text, sessionName }: { text: string; sessionName?: string }) {
  const { theme } = useUnistyles();
  const parsed = parseImageMessage(text);
  const hasImages = !!parsed && !!sessionName;
  const caption = hasImages ? parsed!.caption : '';
  const filenames = hasImages ? parsed!.filenames : [];

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
            // uploadUrl já inclui ?token pra <img> sem header
            const uri = uploadUrl(sessionName!, fn);
            return <Image key={fn} source={{ uri }} style={styles.thumb} contentFit="cover" transition={150} />;
          })}
        </View>
        {/* Se a legenda estava vazia e o texto original era só marcador, já mostramos thumbs */}
      </View>
    );
  }

  return (
    <View style={styles.bubble}>
      <Text style={[styles.txt, { color: theme.tokens.text.primary }]} selectable>
        {text}
      </Text>
    </View>
  );
}

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
  thumbs: {
    flexDirection: 'row',
    flexWrap: 'wrap',
    gap: theme.base.space[2],
  },
  thumb: {
    width: 96,
    height: 96,
    borderRadius: theme.base.radius.md,
    backgroundColor: theme.tokens.bg.elevated,
  },
}));
