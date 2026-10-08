// Transporte equivalente ao DisconnectedNative, sem inicialização de configuração.
const readline = require('node:readline');
const input = readline.createInterface({ input: process.stdin });
input.on('line', (line) => {
  const request = JSON.parse(line);
  if (request.id === undefined) return;
  if (!['initialize', 'account/read'].includes(request.method)) {
    throw new Error(`Método inesperado: ${request.method}`);
  }
  const result = request.method === 'account/read' ? { account: null } : {};
  process.stdout.write(JSON.stringify({ id: request.id, result }) + '\n');
});
