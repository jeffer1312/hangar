#!/usr/bin/env bash
# Confere que a skill `orquestrar` nao se contradiz depois da mudanca de 30/08/2026, em que o
# commit passou a acontecer DEPOIS da revisao e o arbitro saiu do transporte.
#
# Por que existe: a primeira tentativa daquele patch deixou QUATRO contradicoes espalhadas pelas
# paginas, e uma delas INVERTIA um criterio de escolha de ferramenta (o texto reprovava ferramenta
# que le mudanca nao commitada, que passou a ser justamente a certa). Leitura humana nao pegou;
# uma varredura pegou. Sao 16 arquivos e ~5 mil linhas: contradicao aqui e barata de criar e cara
# de descobrir, porque quem descobre e uma execucao real ja em andamento.
#
# Uso:
#   scripts/checar-orquestrar.sh
#
# Sai 0 se tudo bate; 1 listando cada achado com arquivo:linha.
#
# Falso positivo vai para a ALLOWLIST abaixo, com o motivo — nunca afrouxando o padrao. E o
# mesmo criterio do frontend/i18n-allow.json deste repo.

set -uo pipefail

raiz="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
skill="$raiz/skills/orquestrar"

if [ ! -d "$skill" ]; then
  echo "PARE: nao achei $skill" >&2
  exit 1
fi

cd "$skill" || exit 1
# Os 16: o roteador, as 8 paginas de papel e as 7 fichas de modelo. Varrer so as 9 de doutrina
# deixaria passar 5 ocorrencias de "Step" que vivem em references/modelos/.
arquivos=(SKILL.md references/*.md references/modelos/*.md)

falhou=0

# Trechos LITERAIS que podem casar um padrao proibido e mesmo assim estao certos.
# Cada um traz o motivo: sem isso a allowlist vira lixeira. (Skill traduzida pra ingles em
# 31/08/2026 — os padroes e a allowlist acompanham o texto novo.)
allow=(
  '`- [ ] **Step N: …**`'                      # formato literal que o planprog.py casa por regex
  '(the word is literal here)'                 # o mesmo formato, enunciado no plano
  'Task with Steps on one side'                # descreve os DOIS formatos, superpowers x mattpocock
  "it's a Step"                                # a definicao de "step" (camada de baixo)
  'the word `Step` there is literal'           # a excecao declarada da barra de progresso
  'the word `Step` by regex'                   # a mesma excecao, dita no roteador
  'There is no "correction commit"'            # a frase que ANUNCIA que nao existe
  'there is no "correction commit"'
  'Steps:'                                     # rotulo de campo do template da receita (revisor.md)
)

permitido() {  # $1 = linha inteira do grep
  local linha="$1" a
  for a in "${allow[@]}"; do
    case "$linha" in *"$a"*) return 0 ;; esac
  done
  return 1
}

# proibido <padrao-grep> <explicacao>
proibido() {
  local pat="$1" msg="$2" achou=0 linha
  while IFS= read -r linha; do
    [ -z "$linha" ] && continue
    permitido "$linha" && continue
    if [ "$achou" = 0 ]; then
      echo
      echo "✗ $msg"
      achou=1
      falhou=1
    fi
    echo "    $linha"
  done < <(grep -rn -- "$pat" "${arquivos[@]}" 2>/dev/null)
}

# 3. "Step" (maiusculo) nao enuncia mecanica em lugar nenhum — a camada de baixo se chama "step".
proibido 'Step' \
  'A camada de baixo se chama "step" (minusculo). "Step" so vale como formato literal do app.'

# 2. "correction commit" nao e parte do ciclo: a correcao acontece ANTES do commit.
proibido 'correction commit' \
  'Nao existe commit de correcao no ciclo — a rodada reprovada nao vira commit.'

# 1. O executor nao reporta hash ao arbitro fora do fechamento, e o arbitro nao carrega o hash.
proibido 'sends the hash to the reviewer\|relays the hash\|forwards the hash\|relays, the executor applies' \
  'O arbitro saiu do transporte: nao carrega hash nem receita entre executor e revisor.'

# 5. A prova da rodada e `git diff HEAD` — depois do `git add`, `git diff` sai VAZIO.
proibido 'git diff >' \
  'Grave a prova com `git diff HEAD >` — depois do `git add` o `git diff` sai vazio.'

# 5b. `git stash create` sozinho devolve objeto dangling; so o `store` da uma ref que sobrevive ao gc.
for f in "${arquivos[@]}"; do
  while IFS= read -r n; do
    [ -z "$n" ] && continue
    # o `store` tem de aparecer nas 5 linhas seguintes (bloco de comandos ou frase logo abaixo)
    if ! sed -n "${n},$((n + 5))p" "$f" | grep -q 'stash store'; then
      echo
      echo "✗ \`git stash create\` sem \`git stash store\` logo abaixo: o objeto fica dangling."
      echo "    $f:$n"
      falhou=1
    fi
  done < <(grep -n 'stash create' "$f" 2>/dev/null | cut -d: -f1)
done

# 6. Ponteiro para pagina que nao existe. Fatiar uma pagina em varias e a operacao que quebra
#    referencia cruzada em silencio: o texto continua legivel e o leitor abre o arquivo errado.
#    Arquivos que a EXECUCAO cria (nao paginas da skill) nao sao ponteiro e nao entram na conta.
for f in "${arquivos[@]}"; do
  while IFS= read -r alvo; do
    [ -z "$alvo" ] && continue
    case "$alvo" in licoes.md|registro.md|regras.md) continue ;; esac
    if [ ! -f "references/$alvo" ] && [ ! -f "$alvo" ] && [ ! -f "references/modelos/$alvo" ]; then
      echo
      echo "✗ $f aponta para \`$alvo\`, que nao existe."
      falhou=1
    fi
  done < <(grep -oh '`\(references/\)\?[a-z][a-z0-9-]*\.md`' "$f" 2>/dev/null \
             | tr -d '`' | sed 's|^references/||' | sort -u)
done

# 4. As paginas que descrevem o mesmo fluxo em outro contexto usam o fluxo novo.
for f in references/revisao-final.md references/paralelo-worktree.md references/replanejar.md; do
  if ! grep -qi 'round\|dirty tree\|stash' "$f"; then
    echo
    echo "✗ $f nao menciona o fluxo novo (rodada / arvore suja / stash) — provavel pagina esquecida."
    falhou=1
  fi
done

echo
if [ "$falhou" = 0 ]; then
  echo "orquestrar OK — ${#arquivos[@]} arquivos, nenhuma contradicao."
  exit 0
fi
echo "PARE: a skill se contradiz nos pontos acima."
exit 1
