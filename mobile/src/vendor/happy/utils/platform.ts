import { Platform } from 'react-native';
export function isRunningOnMac(): boolean {
  if (Platform.OS !== 'ios') return false;
  // @ts-ignore isPad runtime on iOS
  return (Platform as any).isPad && typeof Platform.Version === 'string' && (Platform.Version as string).includes('Mac');
}
