# Medição da C1 (estado do Claude sem terminal no feed do runtime): roteiro

Sem números. Os números saem do uso real do dono e entram aqui quando existirem. Mesmo método da
medida de 08/10 que motivou a C1 (uma sessão gerando, Python +33 ms/s contra Rust +14 ms/s;
`state` 1,3/s contra `preview` 0,7/s).

## Montagem

- Antes: backend da `main`. Depois: backend da `hangar-server-parte5-c1` (contrato 41). Os dois no
  backend de uso do dono, com o Rust de pé.
- Uma sessão Claude **sem terminal**, aberta, pedindo uma resposta longa (algo que gere por 1–2
  minutos). Nenhuma outra sessão trabalhando.
- Descobrir os pids: o Python do backend e o `hangar-server` (`pgrep -af hangar-server`).

## Comando

```
scripts/medir-claude-sem-terminal.py PID_PYTHON PID_RUST SESSAO 90 antes.json [ENV] [URL]
```

Dispare o pedido na sessão logo depois de iniciar o script. `ENV` (padrão `backend/.env` do
repositório) é o `.env` com `CP_AUTH_TOKEN`; `URL` (padrão `http://127.0.0.1:8765`) é o backend.
A saída agrupa por estado da sessão (`working`, `idle`): segundos, ms/s de CPU do Python e do
Rust, eventos por segundo e bytes por segundo de cada tipo.

## O que comparar (estado `working`)

| Medida | Antes (08/10) | Esperado depois |
|---|---|---|
| `python_ms_s` | 33 | perto do repouso (o Python não produz mais os seis eventos) |
| `rust_ms_s` | 14 | igual ou pouco acima |
| `eventos_por_s.state` | 1,3 | menor ou igual (o feed só publica o que mudou) |
| `eventos_por_s.preview` | 0,7 | igual |

## Conferir junto (uso real)

Cartão de pergunta aparece e some; sugestão do plugin aparece e some; sessão parada mostra modelo,
esforço e contexto; fechar e reabrir no meio de um turno continua `working` até o fim; convidado
(8766) vê o mesmo estado.
