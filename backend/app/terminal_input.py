from app.tmux import send_keys


class TerminalInput:
    def send_prompt(self, name: str, text: str) -> None:
        if any(ord(c) < 32 and c != "\t" for c in text):
            raise ValueError("control characters not allowed in prompt")
        send_keys(name, text, literal=True)
        send_keys(name, "Enter")

    def select(self, name: str, option: int) -> None:
        if option < 1:
            raise ValueError("option must be >= 1")
        for _ in range(option - 1):
            send_keys(name, "Down")
        send_keys(name, "Enter")

    def interrupt(self, name: str) -> None:
        send_keys(name, "Escape")
