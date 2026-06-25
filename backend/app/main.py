import uvicorn
from app.config import settings

LOOPBACK = {"127.0.0.1", "localhost", "::1"}


def startup_guard(settings) -> None:
    """Refuse to bind a non-loopback interface with the default token."""
    if settings.auth_token == "change-me" and settings.lan_bind_ip not in LOOPBACK:
        raise SystemExit(
            "Refusing to start: CP_AUTH_TOKEN is still the default 'change-me' "
            f"while binding {settings.lan_bind_ip}. Set CP_AUTH_TOKEN to a strong "
            "secret, or bind 127.0.0.1 for local dev."
        )
    if settings.auth_token == "change-me":
        print("WARNING: using the default 'change-me' token on loopback. "
              "Set CP_AUTH_TOKEN before exposing this on your LAN.")


def main():
    startup_guard(settings)
    uvicorn.run("app.api:app", host=settings.lan_bind_ip, port=settings.port)


if __name__ == "__main__":
    main()
