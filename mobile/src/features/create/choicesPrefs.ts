import { prefs } from '../../stores/prefs';

// Só a máquina fica no aparelho. Provedor, conta, modelo, nível e modo não: o padrão deles é do
// servidor, e uma cópia aqui iria no pedido no lugar dele.
const MACHINE = 'create.machine.v1';

// A máquina da Nova conversa não é o servidor ativo do app: trocar aqui não mexe nas Configurações.
export const readMachine = () => prefs.getString(MACHINE) ?? null;
export function rememberMachine(serverId: string): void {
  try { prefs.set(MACHINE, serverId); } catch { /* a máquina só não volta na próxima abertura */ }
}
