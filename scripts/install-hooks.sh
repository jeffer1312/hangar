#!/usr/bin/env bash
# Liga os hooks versionados deste repo (scripts/hooks/) apontando core.hooksPath pra lá.
#
# Por que hooksPath e não symlink em .git/hooks: hooksPath é UMA config e vale pra pasta inteira,
# então um hook novo entra sem ninguém reinstalar nada. E como o caminho é relativo à raiz do
# repo, funciona igual em worktree.
#
# Idempotente. Desligar:  git config --unset core.hooksPath
set -euo pipefail

RAIZ="$(cd "$(dirname "$(realpath "$0")")/.." && pwd)"
cd "$RAIZ"

chmod +x scripts/hooks/* 2>/dev/null || true
git config core.hooksPath scripts/hooks

echo "hooks ligados: $(git config core.hooksPath)"
# Falha ALTO se algum hook ficou sem +x. O `|| true` do chmod acima é de propósito (FS que não
# guarda o bit, permissão), mas sem este exit o script terminava em 0 dizendo "hooks ligados" com a
# trava de vazamento DESLIGADA — e quem encadeia `install-hooks.sh && ...` só olha o código de saída,
# não o "SEM +x" perdido no meio do stdout.
sem_x=0
for h in scripts/hooks/*; do
    [[ -f "$h" ]] || continue
    if [[ -x "$h" ]]; then
        printf '  ok %s\n' "$(basename "$h")"
    else
        printf '  SEM +x %s\n' "$(basename "$h")"
        sem_x=1
    fi
done
if (( sem_x )); then
    echo
    echo "FALHOU: hook sem permissão de execução — o git NÃO vai rodá-lo. Confira o filesystem." >&2
    exit 1
fi
# Padrões privados (domínio do trabalho, prefixo de ticket, e-mail corporativo): moram fora do
# versionamento, senão o repo público publica o que a trava existe pra esconder. Criado vazio, com
# o formato comentado — quem tem o que esconder preenche uma vez; quem não tem, ignora.
#
# No diretório da MÁQUINA, não no `.git`: o hook lê os dois, mas o do repo morre no `git clone` —
# criar o modelo lá mandava o usuário preencher o arquivo que some justamente no clone seguinte, e
# a trava nasceria sem os padrões internos sem nada avisar. O do repo continua valendo pra quem
# quiser um padrão só daquele checkout; só não é o que o instalador oferece.
PRIVADOS="${XDG_CONFIG_HOME:-$HOME/.config}/git/hooks-padroes-privados"
mkdir -p "$(dirname "$PRIVADOS")"
# `-s` (existe E tem conteúdo), não `-f`: uma escrita interrompida no meio — disco cheio, quota do
# $HOME, processo morto — deixaria um arquivo vazio que todas as execuções seguintes aceitariam como
# "já existe", e a trava rodaria pra sempre só com os padrões genéricos, sem nada avisando.
if [[ ! -s "$PRIVADOS" ]]; then
    # Escreve em temporário, tranca em 600 e só então move: o arquivo guarda domínio do empregador e
    # prefixo de ticket, e com o umask normal nasceria 644 — legível por qualquer conta da máquina,
    # trocando um vazamento público por um local. O `mv` é atômico, então nunca há arquivo parcial
    # no destino final.
    tmp_privados="$PRIVADOS.tmp.$$"
    cat > "$tmp_privados" <<'MODELO'
# Padrões privados do pre-commit, um por linha:  nome::regex(ERE)
# Este arquivo NÃO é versionado — é o lugar do que não pode aparecer num repo público.
# Exemplos (descomente e troque pelos seus):
# dominio do trabalho::empresa\.(com|dev)
# prefixo de ticket::ABC-[0-9]{4,}
# e-mail corporativo::seu_usuario@
MODELO
    chmod 600 "$tmp_privados"
    mv "$tmp_privados" "$PRIVADOS"
    echo "criado (fora do versionamento, 600): $PRIVADOS"
fi

echo
echo "teste rápido:  echo 'AKIAIOSFODNN7EXAMPLE' >> /tmp/x && git add -f /tmp/x  (deve recusar)"
echo "pular numa emergência:  git commit --no-verify"
