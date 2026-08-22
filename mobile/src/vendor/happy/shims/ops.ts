// Shim para '@/sync/ops' — permissão do Happy não existe no Hangar
// AskUserQuestion responde por endpoint próprio (plano 2)

export async function sessionAllow(): Promise<void> {
  // no-op
}

export async function sessionDeny(): Promise<void> {
  // no-op
}

export async function sessionSetAgentModes(): Promise<void> {
  // no-op
}

export async function sessionRipgrep(): Promise<{ success: boolean; stdout?: string; stderr?: string; error?: string }> {
  return { success: false, error: 'not implemented in hangar' };
}
