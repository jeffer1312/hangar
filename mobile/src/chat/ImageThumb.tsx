import { useState } from 'react';
import { Pressable, StyleSheet as RNStyleSheet, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { Image } from 'expo-image';
import { useReducedMotion } from 'react-native-reanimated';
import * as m from '../paraglide/messages';
import { LightboxModal } from '../features/attachments/LightboxModal';
import { Icon } from '../ui/Icon';
import { superficie } from '../theme/superficie';
import { RevealFrame } from './RevealFrame';

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

export function ImageThumb({ items, index, style, revealTs }: Props) {
  const { theme } = useUnistyles();
  const [open, setOpen] = useState<number | null>(null);
  const item = items[index];
  // A miniatura reduzida que falha dá lugar ao original uma vez; só então a falha aparece.
  const candidates = item.thumbUri && item.thumbUri !== item.uri ? [item.thumbUri, item.uri] : [item.uri];
  // As falhas valem para estes endereços: a rota que muda (rede local ↔ Tailscale), mesmo de volta à anterior, tenta de novo.
  const candidatesKey = candidates.join('\n');
  const [fails, setFails] = useState<{ key: string; uris: readonly string[] }>({ key: candidatesKey, uris: [] });
  const failedUris = fails.key === candidatesKey ? fails.uris : [];
  const shown = candidates.find((uri) => !failedUris.includes(uri));
  const setFalhou = () => {
    if (shown === undefined) return;
    setFails((prev) => {
      const uris = prev.key === candidatesKey ? prev.uris : [];
      return uris.includes(shown) ? prev : { key: candidatesKey, uris: [...uris, shown] };
    });
  };
  const cur = open !== null ? items[open] : null;
  const step = (dir: 1 | -1) => setOpen((i) => (i === null ? i : Math.min(items.length - 1, Math.max(0, i + dir))));

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
        onPress={() => setOpen(index)}
        accessibilityRole="imagebutton"
        accessibilityLabel={item.kind === 'video' ? m.anexos_tocar({ nome: item.name }) : m.anexos_ver({ n: item.name })}
      >
        {item.kind === 'image' && revealTs !== undefined ? (
          <RevealImage uri={shown} headers={item.headers} ts={revealTs} style={style} onError={setFalhou} />
        ) : item.kind === 'image' ? (
          <Image source={{ uri: shown, headers: item.headers }} style={style} contentFit="cover" transition={150} onError={setFalhou} />
        ) : (
          // ponytail: sem expo-video não há quadro do vídeo; a miniatura é o ▶ e o nome.
          <View style={[style, styles.video]}>
            <Icon name="Play" size={26} color="#fff" />
            <Text style={styles.videoName} numberOfLines={1}>{item.name}</Text>
          </View>
        )}
      </Pressable>
      <LightboxModal
        media={cur ? {
          uri: cur.uri,
          headers: cur.headers,
          filename: cur.name,
          kind: cur.kind,
          counter: items.length > 1 ? `${open! + 1}/${items.length}` : undefined,
          onStep: items.length > 1 ? step : undefined,
        } : null}
        onClose={() => setOpen(null)}
      />
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
