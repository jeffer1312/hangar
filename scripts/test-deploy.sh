#!/usr/bin/env bash
# Executa o deploy real (scripts/deploy.sh) em clones descartáveis; serviços, builds e o hangar-server
# são falsos. Cada cenário confere o commit final, o dist, os reinícios e que nada chama uv ou python3.
#
#   ./scripts/test-deploy.sh            # todos os cenários
#   ./scripts/test-deploy.sh success    # só os dados
set -u

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SCRIPT="$HERE/deploy.sh"
LOOKUP="$HERE/hangar-server-lookup"
SCENARIOS=(success delayed boot_fail wrong_body nested_body same_pid flapping restart_fail rollback_fail build_fail
  npm_ci_fail dirty front_fail self_update self_update_fail rollback_restart_fail)
[ "$#" -gt 0 ] && SCENARIOS=("$@")

FAILED=0
fail() { printf 'FALHOU [%s]: %s\n' "$SCENARIO" "$*"; FAILED=1; }

# Um executável só para todos os falsos; o nome com que é chamado decide o que faz.
write_fake() {
  cat > "$1" <<'FAKE'
#!/bin/sh
name=$(basename "$0")
root=$DEPLOY_TEST_ROOT
repo=$root/repo
scenario=$DEPLOY_TEST_SCENARIO
echo "$name $*" >> "$root/calls"
candidate=0
[ "$(cat "$repo/revision")" = new ] && candidate=1
get() { cat "$root/$1"; }
put() { printf '%s' "$2" > "$root/$1"; }
has() { word=$1; shift; for arg; do [ "$arg" = "$word" ] && return 0; done; return 1; }
case $name in
  sleep) exit 0 ;;
  uv|python3) exit 97 ;;
  journalctl) echo 'Traceback: SyntaxError in rejected candidate' ;;
  hangar-server)
    [ "$1" = deploy-port ] && [ "$2" = --backend ] && [ "$3" = "$repo/backend" ] || exit 9
    echo 8766 ;;
  npm)
    if has build "$@"; then
      mkdir -p "$repo/frontend/dist"
      printf 'new dist' > "$repo/frontend/dist/index.html"
      [ "$scenario" = build_fail ] && exit 1
    elif has ci "$@" && [ "$scenario" = npm_ci_fail ] && [ "$candidate" = 1 ]; then
      exit 1
    fi ;;
  systemctl)
    if has list-unit-files "$@"; then
      [ "$scenario" = front_fail ] && exit 0 || exit 1
    elif has show "$@"; then
      if [ "$scenario" = flapping ] && [ "$candidate" = 1 ]; then put pid $(( $(get pid) + 1 )); fi
      get pid; echo
    elif has is-active "$@"; then
      if [ "$scenario" = front_fail ] && [ "$candidate" = 1 ] && has hangar-frontend.service "$@"; then exit 1; fi
      [ "$(get pid)" != 0 ] && exit 0 || exit 1
    elif has stop "$@"; then
      put pid 0
    elif has restart "$@"; then
      put restarts $(( $(get restarts) + 1 ))
      if [ "$scenario" = same_pid ] && [ "$candidate" = 1 ]; then put pid 101; else put pid $(( 200 + $(get restarts) )); fi
      [ "$scenario" = restart_fail ] && [ "$candidate" = 1 ] && exit 1
      [ "$scenario" = rollback_restart_fail ] && [ "$candidate" = 0 ] && exit 1
    fi
    exit 0 ;;
  curl)
    case " $* " in *127.0.0.1:8766/api/peers/ping*) ;; *) exit 99 ;; esac
    put probes $(( $(get probes) + 1 ))
    failed=0
    [ "$scenario" = rollback_fail ] && failed=1
    if [ "$candidate" = 1 ]; then
      case $scenario in boot_fail|rollback_restart_fail|self_update_fail) failed=1 ;; esac
      [ "$scenario" = delayed ] && [ "$(get probes)" -lt 3 ] && failed=1
    fi
    if [ "$failed" = 1 ]; then echo 000; exit 7; fi
    out=
    while [ "$#" -gt 0 ]; do [ "$1" = -o ] && out=$2; shift; done
    if [ "$candidate" = 1 ] && [ "$scenario" = wrong_body ]; then printf '<html>wrong server</html>' > "$out"
    # Outro programa com um `"hangar": true` aninhado: só o do topo é a saúde do Hangar.
    elif [ "$candidate" = 1 ] && [ "$scenario" = nested_body ]; then printf '{"app": {"hangar": true}}' > "$out"
    else printf '{"hangar": true, "version": "x"}' > "$out"; fi
    echo 200 ;;
esac
exit 0
FAKE
  chmod 755 "$1"
}

for SCENARIO in "${SCENARIOS[@]}"; do
  root="$(mktemp -d -t hangar-deploy-test-XXXXXX)"
  repo="$root/repo" origin="$root/origin" home="$root/home"
  bin="$home/.local/bin"
  mkdir -p "$bin" "$origin" "$repo"
  (
    unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GIT_PREFIX
    export HOME="$home" DEPLOY_TEST_ROOT="$root" DEPLOY_TEST_SCENARIO="$SCENARIO" TMPDIR="$root"
    g() { git -c core.hooksPath=/dev/null "$@" >/dev/null 2>&1; }
    g -C "$origin" init --bare
    cd "$repo"
    g init -b main
    g config user.name "Deploy test"
    g config user.email deploy@example.invalid
    mkdir -p scripts backend frontend
    printf 'frontend/dist*\nbackend/.env\n' > .gitignore
    cp "$SCRIPT" scripts/deploy.sh
    cp "$LOOKUP" scripts/hangar-server-lookup
    printf old > revision
    printf 'old lock' > package-lock.json
    g add scripts .gitignore revision package-lock.json
    g commit -m old
    old="$(git rev-parse HEAD)"
    g remote add origin "$origin"
    g push origin main
    printf new > revision
    printf 'new lock' > package-lock.json
    case $SCENARIO in self_update*)
      { printf '#!/usr/bin/env bash\n'; for _ in $(seq 1000); do echo '# candidate'; done
        tail -n +2 "$SCRIPT"; printf '\nprintf unexpected > "$REPO/candidate-script-executed"\n'; } > scripts/deploy.sh
      g add scripts/deploy.sh ;;
    esac
    g add revision package-lock.json
    g commit -m new
    new="$(git rev-parse HEAD)"
    g push origin main
    g reset --keep "$old"
    mkdir -p frontend/dist
    printf 'old dist' > frontend/dist/index.html
    printf 'CP_PORT=8766\n' > backend/.env
    [ "$SCENARIO" = dirty ] && printf 'user edit' > revision
    printf 101 > "$root/pid"; printf 0 > "$root/restarts"; printf 0 > "$root/probes"
    write_fake "$bin/fake"
    for name in systemctl curl npm uv python3 sleep journalctl hangar-server; do ln -s fake "$bin/$name"; done
    export CP_RUST_SERVER_BIN="$bin/hangar-server"
    out="$(timeout 60 bash "$repo/scripts/deploy.sh" 2>&1)"; code=$?
    head="$(git rev-parse HEAD)"
    restarts="$(cat "$root/restarts")"
    calls="$(cat "$root/calls" 2>/dev/null)"
    printf '%s\n' "$out" > "$root/out"
    {
      echo "code=$code"; echo "head_new=$([ "$head" = "$new" ] && echo 1 || echo 0)"
      echo "head_old=$([ "$head" = "$old" ] && echo 1 || echo 0)"; echo "restarts=$restarts"
      echo "dist=$(cat frontend/dist/index.html)"; echo "branch=$(git branch --show-current)"
      echo "revision=$(cat revision)"; echo "pid=$(cat "$root/pid")"
      echo "clean=$([ -z "$(git status --porcelain --untracked-files=no)" ] && echo 1 || echo 0)"
      echo "self=$([ -e candidate-script-executed ] && echo 1 || echo 0)"
    } > "$root/facts"
    printf '%s\n' "$calls" > "$root/calls.txt"
  )
  fact() { sed -n "s/^$1=//p" "$root/facts"; }
  out="$(cat "$root/out")"
  calls="$(cat "$root/calls.txt")"
  case $SCENARIO in
    success|delayed|self_update)
      [ "$(fact code)" = 0 ] || fail "saiu $(fact code): $out"
      [ "$(fact head_new)" = 1 ] || fail "não ficou no commit novo"
      [ "$(fact dist)" = "new dist" ] || fail "dist: $(fact dist)"
      [ "$(fact restarts)" = 1 ] || fail "reinícios: $(fact restarts)" ;;
    dirty)
      [ "$(fact code)" != 0 ] || fail "devia recusar"
      [ "$(fact head_old)" = 1 ] || fail "mexeu no commit"
      [ "$(fact revision)" = "user edit" ] || fail "perdeu a edição local"
      [ "$(fact restarts)" = 0 ] || fail "reiniciou" ;;
    *)
      [ "$(fact code)" != 0 ] || fail "devia falhar: $out"
      [ "$(fact head_old)" = 1 ] || fail "não voltou ao commit antigo: $out"
      [ "$(fact branch)" = main ] || fail "branch: $(fact branch)"
      [ "$(fact dist)" = "old dist" ] || fail "dist não voltou: $(fact dist)"
      case $SCENARIO in build_fail|npm_ci_fail) want=0 ;; *) want=2 ;; esac
      [ "$(fact restarts)" = "$want" ] || fail "reinícios $(fact restarts), esperado $want"
      printf '%s' "$out" | grep -qi rollback || fail "sem rollback na saída"
      printf '%s\n' "$calls" | grep -q '^npm ci' || fail "o front não foi reinstalado na volta"
      case $SCENARIO in rollback_fail|rollback_restart_fail)
        printf '%s' "$out" | grep -q CRITICO || fail "sem CRITICO"
        [ "$(fact pid)" = 0 ] || fail "serviço não parou" ;;
      esac
      case $SCENARIO in build_fail|npm_ci_fail) ;; *)
        printf '%s\n' "$calls" | grep -q '^journalctl' || fail "journal não capturado" ;;
      esac ;;
  esac
  [ "$SCENARIO" = dirty ] || [ "$(fact clean)" = 1 ] || fail "árvore suja no fim"
  [ "$(fact self)" = 0 ] || fail "rodou o deploy.sh candidato"
  if printf '%s\n' "$calls" | grep -Eq '^(uv|python3) '; then fail "chamou uv ou python3: $calls"; fi
  case $SCENARIO in dirty) ;; *)
    printf '%s\n' "$calls" | grep -q "^hangar-server deploy-port --backend $root/repo/backend\$" || fail "porta não veio do hangar-server" ;;
  esac
  rm -rf -- "$root"
done

if [ "$FAILED" = 0 ]; then echo "ok: ${#SCENARIOS[@]} cenários do deploy"; else exit 1; fi
