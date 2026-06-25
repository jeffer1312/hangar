from pathlib import Path
from pydantic_settings import BaseSettings, SettingsConfigDict


class Settings(BaseSettings):
    model_config = SettingsConfigDict(env_prefix="CP_", env_file=".env")

    lan_bind_ip: str = "192.168.77.23"
    port: int = 8765
    auth_token: str = "change-me"
    projects_dir: Path = Path.home() / ".claude" / "projects"
    poll_interval: float = 0.75


settings = Settings()
