import { prefs } from '../stores/prefs';

type ErrorHandler = (error: unknown, fatal?: boolean) => void;
type Exception = {
  isFatal: boolean;
  name?: string | null;
  message: string;
  originalMessage?: string | null;
  componentStack?: string | null;
  stack?: unknown;
  extraData?: { rawStack?: string };
};
export type FatalErrorMetadata = { platform: string; version?: string; runtime?: string; updateId?: string; embedded?: boolean };
export type FatalErrorReport = {
  timestamp: string;
  name: string;
  message: string;
  route: string;
  stack: string;
  componentStack: string;
  metadata: FatalErrorMetadata;
};
type StoredError = { report: FatalErrorReport; pending: boolean; noticed?: boolean };
type Runtime = {
  errorUtils?: { getGlobalHandler(): ErrorHandler; setGlobalHandler(handler: ErrorHandler): void };
  exceptions: { unstable_setExceptionDecorator(decorator: (data: Exception) => Exception): void };
  registerExceptionListener?: (listener: (data: Exception) => void) => void;
  metadata: FatalErrorMetadata;
};
const KEY = 'diagnostics.lastFatalError';
let installed = false;
let capturing = false;
let route = '/startup';
let replaying = false;

// A rota é a do arquivo do Router: nunca leva nome da sessão, servidor ou parâmetros.
export function setFatalErrorRoute(segments: readonly string[]): void {
  route = `/${segments.join('/')}`;
}

function safeMessage(value: unknown): string {
  if (typeof value !== 'string') return '[redacted]';
  const line = value.split('\n')[0].replace(/^(?:TypeError|Error|Invariant Violation): /, '');
  if (/^[\w.$]+ is not a function(?:\b|$)/.test(line)) return '[redacted] is not a function';
  // Texto arbitrário de throw/servidor pode ser conversa; só diagnósticos de runtime passam.
  const patterns = [
    /^Cannot read (?:properties|property) of (?:undefined|null)(?: \(reading ['"][^'"]*['"]\))?/,
    /^Cannot read property ['"][^'"]*['"] of (?:undefined|null)/,
    /^(?:undefined|null) is not (?:an object|a function)(?: \(evaluating ['"][^'"]*['"]\))?/,
    /^Property ['"][^'"]*['"] doesn't exist/,
    /^Cannot convert (?:undefined|null)(?: value)? to object/,
    /^Maximum (?:update depth|call stack size) exceeded\.?/,
    /^Out of memory/,
    /^Rendered more hooks than during the previous render\./,
    /^Rendered fewer hooks than expected\./,
    /^Invalid hook call\. Hooks can only be called inside of the body of a function component\./,
    /^Couldn't find (?:the header height|a navigation context|a route object)\./,
    /^Text strings must be rendered within a <Text> component\.?/,
    /^Objects are not valid as a React child/,
    /^Element type is invalid/,
    /^Minified React error #\d+/,
    /^TurboModuleRegistry\.getEnforcing\(\.\.\.\): ['"][^'"]*['"] could not be found/,
  ];
  const diagnostic = patterns.map(pattern => line.match(pattern)?.[0]).find(Boolean);
  return diagnostic?.replace(/(["'`]).*?\1/g, '[redacted]').slice(0, 1500) ?? '[redacted]';
}

function safeFrames(value: unknown): string {
  if (Array.isArray(value)) {
    return value.slice(0, 80).map((frame: Record<string, unknown>) => {
      const name = typeof frame?.methodName === 'string' && /^[\w.$<> -]{1,120}$/.test(frame.methodName) ? frame.methodName : '<anonymous>';
      const base = typeof frame?.file === 'string' ? frame.file.split(/[?#]/)[0].split(/[\\/]/).pop() : '';
      const file = base && /^[\w.-]{1,160}$/.test(base) ? base : 'bundle';
      return `${name}@${file}:${Number(frame?.lineNumber) || 0}:${Number(frame?.column) || 0}`;
    }).join('\n');
  }
  if (typeof value !== 'string') return '';
  return value.split('\n').filter((line) => /^\s*at\s/.test(line) || /^[\w.$<> -]+@/.test(line))
    .slice(0, 80).map((line) => {
      // Linha longa (mensagem embutida na pilha) faria a busca de caminho custar segundos no handler fatal.
      const cleaned = line.slice(0, 1000)
        // Pasta com espaço é real: o OTA do iOS fica em "Library/Application Support".
        .replace(/(?:https?|file):\/\/[^)\n]+/g, (url) => {
          const position = url.match(/:\d+:\d+$/)?.[0] ?? '';
          const file = url.replace(/:\d+:\d+$/, '').split(/[?#]/)[0].split('/').pop() ?? 'bundle';
          return `${file}${position}`;
        })
        .replace(/\/(?:[^():\/\n]+\/)+([^\s():\/]+)/g, '$1');
      return cleaned.match(/^\s*at [\w.$<> -]{1,120} \((?:address at )?[\w.-]+:\d+:\d+\)/)?.[0]
        ?? cleaned.match(/^[\w.$<> -]{1,120}@[\w.-]+:\d+:\d+/)?.[0]
        ?? cleaned.match(/^[\w.$<> -]{1,120}@\d+:\d+/)?.[0]
        ?? cleaned.match(/^\s*at [\w.$<> -]{1,120} \((?:native|<anonymous>)\)/)?.[0]
        ?? '';
    }).filter(Boolean).join('\n').trimEnd().replace(/^\n+/, '').slice(0, 16000);
}

function storeFatal(error: unknown, metadata: FatalErrorMetadata): void {
  if (capturing) return;
  capturing = true;
  try {
    const e = error && typeof error === 'object' ? error as Partial<Exception> : {};
    const name = typeof e.name === 'string' && /^(?:Error|TypeError|ReferenceError|SyntaxError|RangeError|URIError|EvalError|Invariant Violation)$/.test(e.name) ? e.name : Error.name;
    const report: FatalErrorReport = {
      timestamp: new Date().toISOString(), name,
      message: safeMessage(e.originalMessage ?? e.message), route,
      stack: safeFrames(e.extraData?.rawStack ?? e.stack),
      componentStack: safeFrames(e.componentStack), metadata,
    };
    // Síncrono: Promise/arquivo assíncrono não termina antes do RCTFatal.
    prefs.set(KEY, JSON.stringify({ report, pending: true } satisfies StoredError));
  } catch {
    console.warn('fatal_error_capture_failed');
  } finally { capturing = false; }
}

export function installFatalErrorCapture(runtime: Runtime): void {
  if (installed) return;
  installed = true;
  const capture = (error: unknown) => storeFatal(error, runtime.metadata);
  const original = runtime.errorUtils?.getGlobalHandler();
  if (original) runtime.errorUtils?.setGlobalHandler((error, fatal) => {
    if (fatal) capture(error);
    original(error, fatal);
  });
  // React chama ExceptionsManager diretamente; o runtime novo publica pelo ouvinte nativo.
  runtime.exceptions.unstable_setExceptionDecorator((data) => {
    if (data.isFatal) capture(data);
    return data;
  });
  runtime.registerExceptionListener?.((data) => { if (data.isFatal) capture(data); });
}

function readStored(): StoredError | null {
  const raw = prefs.getString(KEY);
  if (!raw) return null;
  const stored = JSON.parse(raw) as StoredError;
  if (!stored?.report || typeof stored.report.message !== 'string' || typeof stored.report.timestamp !== 'string') {
    throw Error('fatal_error_record_invalid');
  }
  return stored;
}

export function readFatalError(): FatalErrorReport | null {
  return readStored()?.report ?? null;
}

// O aviso sai uma vez por erro; o relatório continua em Ajustes > Sobre.
export function takeFatalErrorNotice(): boolean {
  const stored = readStored();
  if (!stored || stored.noticed) return false;
  prefs.set(KEY, JSON.stringify({ ...stored, noticed: true } satisfies StoredError));
  return true;
}

export async function replayFatalError(send: (report: FatalErrorReport) => Promise<boolean>): Promise<void> {
  if (replaying) return;
  replaying = true;
  try {
    const stored = readStored();
    if (!stored?.pending) return;
    if (await send(stored.report)) {
      const current = readStored();
      if (current && JSON.stringify(current.report) === JSON.stringify(stored.report)) prefs.set(KEY, JSON.stringify({ ...current, pending: false }));
    }
  } finally { replaying = false; }
}
