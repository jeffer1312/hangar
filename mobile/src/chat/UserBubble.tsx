import { memo, useMemo, useState, type ReactNode, type Ref } from 'react';
import { Pressable, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { ImageThumb, type MediaItem } from './ImageThumb';
import { FileCard } from './FileCard';
import { fileKind, hasContextRefs, parseImageMessage, parsePeerMessage, parseRealtimeDelegation, transcriptImageUrlNative, uploadUrlNative, fileAuthHeader, type QuoteRef } from '@hangar/core';
import { ContextRefView } from './ContextRefView';
import * as m from '../paraglide/messages';
import { BubbleActions } from './BubbleActions';
import { superficie } from '../theme/superficie';
import { useServers } from '../stores/servers';
import { useRouteBase } from '../net/routeBase';
import { thumbWidthFor } from './thumbs';

// Bolha do usuário: pequena, à direita, no fundo superficie(0.8) — o mesmo degrau da bolha do nativo,
// e por isso acompanha Transparência e Solidez. Se parseImageMessage(text) não nulo → legenda +
// miniaturas. As ações (copiar, compartilhar) não ficam à mostra: o toque longo abre a linha embaixo.
export const UserBubble = memo(function UserBubble({ text, sessionName, serverId, ts, eventId, imageCount = 0, onQuoteClick, bubbleRef }: {
  text: string;
  sessionName?: string;
  serverId?: string;
  ts?: number | null;
  eventId?: string;
  /** Imagens coladas no TERMINAL: vêm do transcript por /transcript-image, não do texto. */
  imageCount?: number;
  /** Toque numa citação: rola até a resposta de origem. */
  onQuoteClick?: (ref: QuoteRef) => void;
  /** A caixa da bolha, para quem precisa saber onde ela está na tela. */
  bubbleRef?: Ref<View>;
}) {
  const { theme } = useUnistyles();
  const server = useServers((s) => s.servers.find((x) => x.id === serverId) ?? null);
  // Mesmo objeto entre renders: o `FileCard` é memo e o `run` dele depende dos headers.
  const authHeaders = useMemo(() => (server ? fileAuthHeader(server) : undefined), [server]);
  const voice = parseRealtimeDelegation(text);
  const [showOriginal, setShowOriginal] = useState(false);
  const [acoes, setAcoes] = useState(false);
  const parsed = parseImageMessage(text);
  const peer = parsePeerMessage(parsed?.caption ?? text);
  const peerColor = peer?.scope === 'external' ? theme.tokens.status.error
    : peer?.scope === 'group' ? theme.tokens.status.warning
      : peer?.scope === 'panel' ? theme.tokens.text.muted : theme.tokens.accent.base;
  const peerLabel = peer ? (peer.scope === 'group' ? m.board_peer_grupo({ n: peer.from })
    : peer.scope === 'panel' ? m.board_peer_painel({ n: peer.from })
      : peer.scope === 'external' ? m.board_peer_fora({ n: peer.from }) : m.board_peer_de({ n: peer.from })) : null;
  const peerAccessibilityLabel = peer ? (peer.scope === 'group' ? m.peer_message_group_sender({ name: peer.from })
    : peer.scope === 'panel' ? m.peer_message_panel_sender({ name: peer.from })
      : peer.scope === 'external' ? m.peer_message_external_sender({ name: peer.from }) : m.peer_message_sender({ name: peer.from })) : undefined;
  const emptyPeer = !!peer && !peer.text.trim();
  const body = peer ? (emptyPeer ? m.peer_message_empty() : peer.text) : text;
  const doTerminal = imageCount > 0 && !!sessionName && !!eventId;
  const hasImages = (!!parsed && !!sessionName) || doTerminal;
  const caption = peer ? body : (parsed ? parsed.caption : doTerminal ? text : '');
  // Como no web: com o caminho escrito pra TODAS as fotos, as últimas `imageCount` já vêm do
  // transcript, e mostrar as duas fontes duplicava cada imagem.
  const filenames = !parsed || !sessionName ? []
    : doTerminal && parsed.marcadores === parsed.filenames.length
      ? parsed.filenames.slice(0, Math.max(0, parsed.filenames.length - imageCount))
      : parsed.filenames;
  // Recalcula as URLs quando a rota do servidor muda; o valor em si sai do `baseOf` dentro delas.
  useRouteBase(server);
  // Vídeo anexado vai como `arquivo:` (com os quadros como `imagem:`), mas é mídia: toca no visor.
  const videos = parsed?.files.filter((f) => fileKind(f) === 'video') ?? [];
  const otherFiles = parsed?.files.filter((f) => fileKind(f) !== 'video') ?? [];
  const galeria: MediaItem[] = hasImages && server ? [
    ...videos.map((fn): MediaItem => ({ uri: uploadUrlNative(sessionName!, fn, server), headers: fileAuthHeader(server), name: fn, kind: 'video' })),
    ...filenames.map((fn): MediaItem => {
      const width = thumbWidthFor(fn);
      return {
        uri: uploadUrlNative(sessionName!, fn, server),
        thumbUri: width ? uploadUrlNative(sessionName!, fn, server, width) : undefined,
        headers: fileAuthHeader(server), name: fn, kind: 'image',
      };
    }),
    ...(doTerminal ? Array.from({ length: imageCount }, (_, i): MediaItem => ({
      uri: transcriptImageUrlNative(sessionName!, eventId!, i, server), headers: fileAuthHeader(server), name: m.anexos_imagem_enviada(), kind: 'image',
    })) : []),
  ] : [];
  const alternar = () => setAcoes((v) => !v);
  // Legenda com citação ou elemento (bloco <hangar-ref>): segmentos no lugar do texto cru.
  const renderCaption = (t: string) => (hasContextRefs(t)
    ? <ContextRefView text={t} onQuoteClick={onQuoteClick} />
    : <Text style={[styles.txt, { color: theme.tokens.text.primary }]}>{t}</Text>);

  // O texto não é `selectable`: no Android a seleção come o toque longo que abre as ações, e copiar
  // já está lá.
  // `largo`: o card de arquivo tem width 100%; numa bolha que encolhe ao conteúdo, ele a esticava
  // além da coluna e ela vazava pela esquerda. Com card, a bolha ocupa a coluna inteira.
  const bolha = (conteudo: ReactNode, textoAcoes: string, voz = false, largo = false) => (
    <View style={[styles.col, voz && styles.colVoz, largo && styles.colLarga]}>
      <Pressable
        ref={bubbleRef}
        onLongPress={alternar}
        delayLongPress={350}
        style={[styles.bubble, voz && styles.voice, largo && styles.bubbleLarga, peer && { borderWidth: 1, borderColor: peerColor }]}
        accessibilityActions={[{ name: 'longpress', label: m.navbar_mais_acoes() }]}
        onAccessibilityAction={(e) => { if (e.nativeEvent.actionName === 'longpress') alternar(); }}
      >
        {peerLabel ? <View accessible accessibilityLabel={peerAccessibilityLabel} style={[styles.peerChip, { borderColor: peerColor }]}>
          <Text accessible={false} style={[styles.voiceLabel, { color: peerColor }]}>{peerLabel}</Text>
        </View> : null}
        {conteudo}
      </Pressable>
      {acoes ? <BubbleActions text={emptyPeer ? text : textoAcoes} ts={ts} ouvir={false} defaultOpen align={voz ? 'start' : 'end'} /> : null}
    </View>
  );

  if (voice) {
    return bolha(
      <>
        <Text style={[styles.voiceLabel, { color: theme.tokens.text.secondary }]}>{m.voice_message_origin()}</Text>
        <Text style={[styles.txt, { color: theme.tokens.text.primary }]}>{voice.input}</Text>
        <Pressable accessibilityRole="button" accessibilityState={{ expanded: showOriginal }}
          onPress={() => setShowOriginal(value => !value)} style={styles.detailsButton}>
          <Text style={{ color: theme.tokens.text.secondary }}>{m.voice_message_original()}</Text>
        </Pressable>
        {showOriginal ? <Text style={[styles.txt, { color: theme.tokens.text.secondary }]}>{text}</Text> : null}
      </>,
      voice.input,
      true,
    );
  }

  // Se há imagens válidas, exibe legenda + thumbnails; senão fallback texto cru
  if (hasImages) {
    if (!server) return bolha(<>
      {caption ? <Text style={[styles.txt, { color: theme.tokens.text.primary }]}>{caption}</Text> : null}
      <Text style={[styles.txt, { color: theme.tokens.status.error }]} accessibilityRole="alert">{m.chat_servidor_removido()}</Text>
    </>, caption || body);
    // Anexo que não é foto (`📎 arquivo:`): o card de download, embaixo das miniaturas.
    const cards = otherFiles.map((f) => (
      <FileCard key={f} uri={uploadUrlNative(sessionName!, f, server)} headers={authHeaders} name={f} />
    ));
    // galeria vazia = foto única absorvida como anexo real: mostra só a legenda, ou o texto original
    if (galeria.length === 0 && cards.length === 0) {
      const display = caption || body;
      return bolha(renderCaption(display), display);
    }
    return bolha(
      <>
        {caption ? renderCaption(caption) : null}
        {galeria.length ? <View style={styles.thumbs}>
          {galeria.map((g, i) => <ImageThumb key={g.uri} items={galeria} index={i} style={styles.thumb} />)}
        </View> : null}
        {cards}
      </>,
      caption || body,
      false,
      cards.length > 0,
    );
  }

  return bolha(renderCaption(body), body);
});

const styles = StyleSheet.create((theme) => ({
  col: {
    alignSelf: 'flex-end',
    alignItems: 'flex-end',
    maxWidth: '82%',
    gap: theme.base.space[1],
  },
  colVoz: { alignSelf: 'flex-start', alignItems: 'flex-start' },
  colLarga: { width: '82%' },
  bubbleLarga: { alignSelf: 'stretch' },
  bubble: {
    backgroundColor: superficie(theme, 0.8),
    borderRadius: theme.base.radius.md,
    paddingHorizontal: theme.base.space[3],
    paddingVertical: theme.base.space[2],
    gap: theme.base.space[2],
  },
  txt: {
    fontSize: theme.base.text.base * theme.conversa.texto,
  },
  // Delegação por voz: borda fina em vez de cor, que fica para estado e aviso.
  voice: { borderWidth: 1, borderColor: theme.tokens.border.default },
  voiceLabel: { fontWeight: theme.base.weight.semibold, fontSize: theme.base.text.sm },
  peerChip: { alignSelf: 'flex-start', borderWidth: 1, borderRadius: theme.base.radius.full, paddingHorizontal: theme.base.space[2], paddingVertical: theme.base.space[0.5] },
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
