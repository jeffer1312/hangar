"""Fixture SINTÉTICA da Task 23 (markdown da conversa e copiar código): a da Task 15d com uma sessão a mais, cuja
resposta traz todo bloco que o estilo da conversa desenha. Nenhuma sessão, pasta ou conta daqui existe de verdade; nada
sai deste processo.

Carrega o código da fixture da Task 15d sem o bloco que sobe o servidor (o arquivo dela não muda). Continuam valendo a
sessão sintetica-stream (resposta em streaming, GET /control/t22stream), para o esmaecimento e o movimento reduzido, e a
sintetica-bastao (resumo do bastão), para a prévia do dossiê no diálogo Nova sessão.

Sessão sintetica-markdown: pensamento com lista e código inline; resposta longa com títulos h1–h3, lista com marcadores
aninhada em três níveis, lista numerada até 10 com parágrafo de continuação, código com linguagem, código sem
linguagem, código inline, citação e duas tabelas (uma de texto; uma de números, que vira gráfico).
"""
import pathlib
import time

HERE = pathlib.Path(__file__).resolve().parent
SOURCE = (HERE / "task15_cards_fixture.py").read_text(encoding="utf-8")
T15NS = {"__name__": "task15_base", "__file__": str(HERE / "task15_cards_fixture.py")}
exec(compile(SOURCE[:SOURCE.index("\nserver = BASE[")], "task15_cards_fixture.py", "exec"), T15NS)

BASE = T15NS["BASE"]
SESSIONS, bump, info, state, msg = (BASE[k] for k in ("SESSIONS", "bump", "info", "state", "msg"))

NAME = "sintetica-markdown"
NOW = time.time()

THOUGHT = ("Preciso conferir o parser antes de responder:\n\n- ler `src/parser.rs`;\n"
           "- ver se o `Token::Fence` guarda a linguagem.")

ANSWER = """# Revisão do parser (sintético)

O parser lê o markdown em uma passada. Abaixo, o que mudou e o que falta, com o `Token::Fence` como exemplo.

## O que mudou

- Blocos de código guardam a linguagem da cerca.
  - A cerca sem linguagem cai no rótulo padrão.
    - Terceiro nível, para conferir o recuo do marcador.
  - Um segundo item no nível dois, comprido o bastante para quebrar em duas linhas quando a coluna da conversa fica estreita.
- Tabelas sem grade: só os filetes entre as linhas.

### Passos, em ordem

1. Ler a cerca.
2. Guardar a linguagem.

   Parágrafo de continuação do passo 2: fica alinhado com o texto, não com o número.
3. Pintar a faixa.
4. Ligar o copiar.
5. Conferir o recuo.
6. Conferir o vão entre blocos.
7. Conferir os títulos.
8. Conferir a tabela.
9. Conferir o código inline.
10. Fechar a rodada.

```rust
fn fence(line: &str) -> Option<&str> {
    let lang = line.strip_prefix("```")?.trim();
    (!lang.is_empty()).then_some(lang)
}
```

```
saída sem linguagem: 3 blocos, 0 erros
```

> Citação: o estilo vale só para a conversa; o diálogo Nova sessão fica como era.

| Arquivo | Papel | Estado |
|---|---|---|
| `src/parser.rs` | lê a cerca | pronto |
| `src/render.rs` | pinta a faixa | em revisão |
| `src/copy.rs` | copia o bloco | falta |

| Mês | Blocos |
|---|---|
| Julho | 12 |
| Agosto | 30 |
| Setembro | 21 |

Fim da resposta (sintético)."""

EVENTS = [
    msg("user_msg", "m0", "Revisa o parser do markdown e me mostra o que mudou.", ts=NOW - 600),
    msg("thinking", "m1", THOUGHT, ts=NOW - 590),
    msg("assistant_msg", "m2", ANSWER, ts=NOW - 560),
]


# Sem gancho no /control/t14reset: a prova desta Task não reinicia a lista.
data = info(NAME, "claude", state="idle", branch="main")
data["cwd"] = "/sintetica/projetos/hangar-sintetico"
SESSIONS[NAME] = {"info": data, "state": state("idle"), "events": [dict(e) for e in EVENTS], "stats": None, "modes": []}
bump()

server = BASE["ThreadingHTTPServer"](("127.0.0.1", 0), T15NS["Handler"])
print(f"Fixture URL: http://127.0.0.1:{server.server_port}", flush=True)
try:
    server.serve_forever()
except KeyboardInterrupt:
    pass
