// Camada do voo da mensagem enviada e as peças de destino. A regra da viagem mora em `sendFlight.ts`.
import { useCallback, useEffect, useRef, type ReactNode, type RefObject } from 'react';
import { View, type StyleProp, type ViewStyle } from 'react-native';
import Animated, { Easing, useAnimatedStyle, useSharedValue, withTiming } from 'react-native-reanimated';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { contextRefsPlainText } from '@hangar/core';
import { superficie } from '../theme/superficie';
import { FLIGHT_MS, flying, land, setFlightHost, useSendFlight, type Flight, type FlightTarget } from './sendFlight';

export const flightRoute = (serverId: string, name: string) => `${serverId}::${name}`;

/** Destino de um envio: invisível enquanto a cópia viaja até ele, e é ele quem diz onde ela pousa. */
export function FlightDestination({ route, text, target, dim = false, measured, style, children }: {
  route: string | null; text: string; target: FlightTarget; dim?: boolean;
  /** A peça de dentro que tem o tamanho do destino (a bolha), quando o invólucro ocupa a linha inteira. */
  measured?: RefObject<View | null>;
  style?: StyleProp<ViewStyle>; children: ReactNode;
}) {
  const node = useRef<View>(null);
  const awaited = useSendFlight((s) => !!route && flying(s.flight, route, text));
  // Dois quadros depois do layout: a lista ainda rola até o fim no quadro seguinte ao da bolha nova.
  const report = useCallback(() => {
    if (!route || !flying(useSendFlight.getState().flight, route, text)) return;
    requestAnimationFrame(() => requestAnimationFrame(() => land(route, text, target, (measured ?? node).current, dim)));
  }, [route, text, target, dim, measured]);
  useEffect(report, [report, awaited]);
  return <View ref={node} collapsable={false} onLayout={report} style={[style, awaited && styles.awaited]}>{children}</View>;
}

function FlyingCopy({ flight }: { flight: Flight }) {
  const { theme } = useUnistyles();
  const progress = useSharedValue(0);
  const { from, to, target, dim } = flight;
  useEffect(() => {
    if (to) progress.value = withTiming(1, { duration: FLIGHT_MS, easing: Easing.out(Easing.cubic) });
  }, [to, progress]);
  const end = to ?? from;
  const inputSize = theme.type.body.fontSize;
  const endSize = target === 'queue' ? theme.base.text.xs : theme.base.text.base * theme.conversa.texto;
  const inputPad = theme.base.space[2];
  const endPad = target === 'queue' ? theme.base.space[2] : theme.base.space[3];
  const box = useAnimatedStyle(() => {
    const t = progress.value;
    return {
      left: from.left + (end.left - from.left) * t,
      bottom: from.bottom + (end.bottom - from.bottom) * t,
      width: from.width + (end.width - from.width) * t,
      height: from.height + (end.height - from.height) * t,
      paddingHorizontal: inputPad + (endPad - inputPad) * t,
      opacity: dim ? 1 - 0.5 * t : 1,
    };
  });
  // A bolha ganha o fundo no caminho; a linha da fila não tem fundo próprio.
  const fill = useAnimatedStyle(() => ({ opacity: target === 'queue' ? 0 : progress.value }));
  const type = useAnimatedStyle(() => ({ fontSize: inputSize + (endSize - inputSize) * progress.value }));
  return (
    <Animated.View style={[styles.copy, box]}>
      <Animated.View style={[styles.fill, { backgroundColor: superficie(theme, 0.8), borderRadius: theme.base.radius.md }, fill]} />
      <Animated.Text style={[{ color: theme.tokens.text.primary }, type]} numberOfLines={target === 'queue' ? 1 : undefined}>
        {contextRefsPlainText(flight.text)}
      </Animated.Text>
    </Animated.View>
  );
}

/** Cobre a área do chat sem pegar toque: é nela que a cópia da mensagem viaja. */
export function SendFlightLayer({ route }: { route: string }) {
  const host = useCallback((node: View | null) => setFlightHost(route, node), [route]);
  const flight = useSendFlight((s) => (s.flight?.route === route ? s.flight : null));
  return (
    <View ref={host} collapsable={false} pointerEvents="none" style={styles.layer}>
      {flight ? <FlyingCopy key={flight.id} flight={flight} /> : null}
    </View>
  );
}

const styles = StyleSheet.create(() => ({
  layer: { position: 'absolute', top: 0, left: 0, right: 0, bottom: 0 },
  copy: { position: 'absolute', justifyContent: 'center', overflow: 'hidden' },
  fill: { position: 'absolute', top: 0, left: 0, right: 0, bottom: 0 },
  awaited: { opacity: 0 },
}));
