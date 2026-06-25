import uvicorn
from app.config import settings


def main():
    uvicorn.run("app.api:app", host=settings.lan_bind_ip, port=settings.port)


if __name__ == "__main__":
    main()
