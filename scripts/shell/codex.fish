# claude-cockpit — wrapper interativo do Codex para fish.
function codex
    if not isatty stdin; or test (count $argv) -gt 1
        command codex $argv
        return
    end
    if test (count $argv) -eq 1
        if string match -qr '^-' -- $argv[1]; or contains -- $argv[1] exec review resume fork archive delete unarchive login logout mcp plugin app-server remote-control cloud doctor debug features completion update sandbox apply
            command codex $argv
            return
        end
    end
    command cp-codex $argv
end
