export type VerificarStepState = "pendente" | "rodando" | "ok" | "falhou";

export type VerificarStep = {
  so: string;
  name: string;
  state: VerificarStepState;
  startedAt?: number;
  seconds?: number;
  tail?: string[];
};

export type VerificarRun = {
  id: string;
  sha: string;
  vm: string;
  owner?: string;
  startedAt: number;
  isQueued: boolean;
  isAlive: boolean;
  endedAt?: number;
  rc?: number;
  steps: VerificarStep[];
  windowsTail?: string[];
};

export type BackgroundShell = {
  id: string;
  command: string;
  startedAt: number;
  endedAt?: number;
  status?: string;
  pids?: number[];
  path?: string;
  tail?: string[];
  current?: string;
};

export type VerificarBoard = { now: number; runs: VerificarRun[] };

declare module "claude-code" {
  interface PluginState {
    hangar: { verificarBoard: VerificarBoard; backgroundShells: BackgroundShell[]; lastState: string };
  }
}
