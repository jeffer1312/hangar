import { lazy, Suspense, useContext, useMemo, useState } from 'react';
import { Pressable, StyleSheet as RNStyleSheet, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { Image } from 'expo-image';
import { useReducedMotion } from 'react-native-reanimated';
import * as m from '../paraglide/messages';
import type { GalleryEntry } from '../features/attachments/gallery';
import { GalleryContext } from './galleryContext';
import { Icon } from '../ui/Icon';
import { superficie } from '../theme/superficie';
import { RevealFrame } from './RevealFrame';
import { useVideoThumb } from '../features/attachments/videoThumb';

// O visor puxa os módulos nativos de arquivo: a bolha não os carrega, só o toque na miniatura.
const GalleryViewer = lazy(() => import('../features/attachments/gallery').then((mod) => ({ default: mod.GalleryViewer })));

export interface MediaItem {
  uri: string;
  headers?: Record<string, string>;
  name: string;
  kind: 'image' | 'video';
  /** Versão reduzida pelo servidor para a miniatura; o visor abre sempre o `uri` original. */
  thumbUri?: string;
}

interface Props {
  /** Todas as mídias da mensagem: o visor abre nesta e passa entre elas, como o do web. */
  items: MediaItem[];
  index: number;
  style: object;
  /** Hora da mensagem (segundos) de imagem citada pelo agente: a que acabou de chegar brilha enquanto carrega e abre do centro. */
  revealTs?: number | null;
  /** Chave desta mídia na conversa (`<id do evento>#<posição>`): com ela o visor passa por toda a conversa. */
  galleryKey?: string;
}

// Mensagem mais velha que isto (segundos) é histórico: a imagem dela aparece sem a revelação.
const JANELA_AO_VIVO = 30;

// Uma vez por imagem enquanto o app vive: a lista desmonta a linha ao rolar, e voltar não abre de novo.
const revelados = new Set<string>();

function RevealImage({ uri, headers, ts, style, onError }: { uri: string; headers?: Record<string, string>; ts: number | null; style: object; onError: () => void }) {
  const reduced = useReducedMotion();
  // Decidido uma vez: a mensagem envelhecer depois de montada não troca o componente no meio.
  const [animar] = useState(() => !reduced && !revelados.has(uri) && ts != null && Date.now() / 1000 - ts < JANELA_AO_VIVO);
  const [chegou, setChegou] = useState(false);
  if (!animar) {
    return <Image source={{ uri, headers }} style={style} contentFit="cover" transition={150} onError={onError} />;
  }
  const { width: w, height: h } = RNStyleSheet.flatten(style) as { width: number; height: number };
  const onLoad = () => {
    revelados.add(uri);
    setChegou(true);
  };
  return (
    <RevealFrame style={style} chegou={chegou}>
      <Image source={{ uri, headers }} style={{ width: w, height: h }} contentFit="cover" transition={0} onLoad={onLoad} onError={onError} />
    </RevealFrame>
  );
}

// O primeiro quadro chega depois (a lista só monta as linhas à vista); até lá, e se não vier, fica o ▶ com o nome.
function VideoThumb({ uri, headers, name, style }: { uri: string; headers?: Record<string, string>; name: string; style: object }) {
  const thumb = useVideoThumb(uri, headers);
  return (
    <View style={[style, styles.video]}>
      {thumb ? <Image source={thumb} style={styles.videoFrame} contentFit="cover" transition={150} /> : null}
      <View style={styles.playBadge}>
        <Icon name="Play" size={thumb ? 20 : 26} color="#fff" />
      </View>
      {thumb ? null : <Text style={styles.videoName} numberOfLines={1}>{name}</Text>}
    </View>
  );
}

export function ImageThumb({ items, index, style, revealTs, galleryKey }: Props) {
  const { theme } = useUnistyles();
  const openConversation = useContext(GalleryContext);
  // Sem a galeria da conversa (eco ainda sem evento, lista sem sessão): só as mídias desta mensagem.
  const [open, setOpen] = useState<string | null>(null);
  const item = items[index];
  // A miniatura reduzida que falha dá lugar ao original uma vez; só então a falha aparece.
  const candidates = item.thumbUri && item.thumbUri !== item.uri ? [item.thumbUri, item.uri] : [item.uri];
  // As falhas valem para estes endereços: a rota que muda (rede local ↔ Tailscale), mesmo de volta à anterior, tenta de novo.
  const candidatesKey = candidates.join('\n');
  const [fails, setFails] = useState<{ key: string; uris: readonly string[] }>({ key: candidatesKey, uris: [] });
  if (fails.key !== candidatesKey) setFails({ key: candidatesKey, uris: [] });
  const failedUris = fails.key === candidatesKey ? fails.uris : [];
  const shown = candidates.find((uri) => !failedUris.includes(uri));
  const setFalhou = () => {
    if (shown === undefined) return;
    setFails((prev) => {
      const uris = prev.key === candidatesKey ? prev.uris : [];
      return uris.includes(shown) ? prev : { key: candidatesKey, uris: [...uris, shown] };
    });
  };
  const local = useMemo<GalleryEntry[]>(
    () => items.map((x, i) => ({ key: String(i), uri: x.uri, thumbUri: x.thumbUri, headers: x.headers, name: x.name, kind: x.kind })),
    [items],
  );
  const press = () => {
    if (galleryKey && openConversation?.(galleryKey)) return;
    setOpen(String(index));
  };

  // Falha não some calada nem vira quadrado vazio: fica o nome, como no web.
  if (shown === undefined) {
    return (
      <View style={styles.broken}>
        <Text style={[styles.brokenTxt, { color: theme.tokens.text.muted }]} numberOfLines={1}>
          {`⚠ ${m.anexos_nao_carregou({ nome: item.name })}`}
        </Text>
      </View>
    );
  }

  return (
    <>
      <Pressable
        onPress={press}
        accessibilityRole="imagebutton"
        accessibilityLabel={item.kind === 'video' ? m.anexos_tocar({ nome: item.name }) : m.anexos_ver({ n: item.name })}
      >
        {item.kind === 'image' && revealTs !== undefined ? (
          <RevealImage uri={shown} headers={item.headers} ts={revealTs} style={style} onError={setFalhou} />
        ) : item.kind === 'image' ? (
          <Image source={{ uri: shown, headers: item.headers }} style={style} contentFit="cover" transition={150} onError={setFalhou} />
        ) : (
          <VideoThumb uri={item.uri} headers={item.headers} name={item.name} style={style} />
        )}
      </Pressable>
      {open !== null ? (
        <Suspense fallback={null}>
          <GalleryViewer entries={local} openKey={open} onOpenKey={setOpen} onClose={() => setOpen(null)} />
        </Suspense>
      ) : null}
    </>
  );
}

const styles = StyleSheet.create((theme) => ({
  video: {
    backgroundColor: '#000',
    alignItems: 'center',
    justifyContent: 'center',
    gap: theme.base.space[1],
    padding: theme.base.space[1],
    overflow: 'hidden',
  },
  videoFrame: {
    position: 'absolute',
    top: 0,
    right: 0,
    bottom: 0,
    left: 0,
  },
  // Círculo escuro: o ▶ branco some sobre quadro claro.
  playBadge: {
    width: 44,
    height: 44,
    borderRadius: theme.base.radius.full,
    alignItems: 'center',
    justifyContent: 'center',
    backgroundColor: 'rgba(0,0,0,0.45)',
  },
  videoName: {
    color: '#fff',
    fontSize: theme.base.text.xxxs,
    maxWidth: '100%',
  },
  broken: {
    height: 30,
    maxWidth: 220,
    justifyContent: 'center',
    paddingHorizontal: theme.base.space[2],
    borderRadius: theme.base.radius.sm,
    borderWidth: 1,
    borderColor: theme.tokens.border.subtle,
    backgroundColor: superficie(theme),
  },
  brokenTxt: {
    fontSize: theme.base.text.xs,
    fontFamily: theme.base.fontMono,
  },
}));
