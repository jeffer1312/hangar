# hangar — wrapper interativo do Codex para fish.
function codex
    if not isatty stdin; or test (count $argv) -gt 1
        command codex $argv
        return
    end
    if test (count $argv) -eq 1
        if string match -qr '^-' -- $argv[1]; or contains -- $argv[1] exec review resume fork archive delete unarchive login logout mcp plugin app-server app remote-control cloud doctor debug features completion update sandbox apply
            command codex $argv
            return
        end
    end
    # Sem o lançador, ou sem sessão aberta (sai 3: nem servidor nem `hangar-server`), o oficial abre.
    if command -q hangar-codex
        command hangar-codex $argv
        set -l rc $status
        test $rc -ne 3; and return $rc
    end
    command codex $argv
end
