import { setVideoCacheSizeAsync } from 'expo-video';

/** Teto do cache de reprodução do expo-video, à parte do dos anexos baixados. */
export const VIDEO_CACHE_LIMIT = 512 * 1024 * 1024;

// Só na abertura do app: o expo-video recusa mudar o teto com algum player vivo.
export function configureVideoCache(): Promise<void> {
  return setVideoCacheSizeAsync(VIDEO_CACHE_LIMIT);
}
