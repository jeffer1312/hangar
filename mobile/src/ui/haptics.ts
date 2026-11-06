import * as Haptics from 'expo-haptics';
import { Platform } from 'react-native';

// Só no iOS (HIG): seleção ao marcar, notificação no resultado. Falha do motor de vibração não é erro do app.
function ios(run: () => Promise<void>) {
  if (Platform.OS === 'ios') void run().catch(() => {});
}

export const haptic = {
  selection: () => ios(() => Haptics.selectionAsync()),
  success: () => ios(() => Haptics.notificationAsync(Haptics.NotificationFeedbackType.Success)),
  error: () => ios(() => Haptics.notificationAsync(Haptics.NotificationFeedbackType.Error)),
  // A chamada de voz abre calada: sem isto só a tela diz que conectou, por isso vale também no Android.
  connected: () => void Haptics.notificationAsync(Haptics.NotificationFeedbackType.Success).catch(() => {}),
  // Quem está falando olha para outro lado: a tela sozinha não avisa que a resposta travou.
  stalled: () => void Haptics.notificationAsync(Haptics.NotificationFeedbackType.Error).catch(() => {}),
};
