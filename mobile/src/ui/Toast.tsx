import { toast as sonner, Toaster } from 'sonner-native';

export const toast = {
  ok: (msg: string) => sonner.success(msg),
  erro: (msg: string) => sonner.error(msg),
};
export { Toaster };
