import { Platform } from 'react-native';
import * as SecureStore from 'expo-secure-store';

const KEY = 'cp_device_id_v1';
const VALID = /^[A-Za-z0-9-]{1,64}$/;

let deviceId: string | null = null;

function newId(): string {
  const uuid = globalThis.crypto?.randomUUID?.();
  if (uuid) return uuid;
  return Array.from({ length: 32 }, () => Math.floor(Math.random() * 16).toString(16)).join('');
}

// O cofre é assíncrono e o core pede a identidade na hora: até carregar, a lista abre sem o
// parâmetro e o próximo stream manda. Id só entra em uso depois de gravado, senão cada abertura
// do app pareceria um aparelho novo.
export function loadDeviceId(): Promise<void> {
  return SecureStore.getItemAsync(KEY).then(async (saved) => {
    if (saved && VALID.test(saved)) { deviceId = saved; return; }
    const id = newId();
    await SecureStore.setItemAsync(KEY, id);
    deviceId = id;
  }).catch(() => {
    console.warn('device_identity_unavailable');
  });
}

export function deviceIdentity(): { kind: 'ios' | 'android'; id: string } | null {
  if (!deviceId) return null;
  if (Platform.OS === 'ios') return { kind: 'ios', id: deviceId };
  if (Platform.OS === 'android') return { kind: 'android', id: deviceId };
  return null;
}
