export function parseCodexOutput(data: string): Record<string, string> {
  const value: unknown = JSON.parse(data);
  if (value === null || typeof value !== 'object' || Array.isArray(value)
      || Object.values(value).some(text => typeof text !== 'string')) {
    throw new Error('invalid_codex_output');
  }
  return value as Record<string, string>;
}
