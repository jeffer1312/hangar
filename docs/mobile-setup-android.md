# Ambiente Android pra desenvolver o app mobile (Arch/CachyOS)

Feito em 21/08/2026 nesta máquina (Task 0 do plano `docs/superpowers/plans/2026-08-21-mobile-expo-fundacao.md`).
Sem Android Studio: só JDK, cmdline-tools, platform-tools e emulador, pelo pacman/AUR.

## Parte que pede `sudo` (uma vez)

```bash
sudo pacman -S --needed jdk17-openjdk
paru -S --needed android-sdk-cmdline-tools-latest android-sdk-platform-tools android-emulator
sudo usermod -aG kvm "$USER"                      # /dev/kvm — sem isso o emulador rasteja
sudo chown -R "$USER:$USER" /opt/android-sdk      # o sdkmanager escreve lá (o pacote AUR deixa root)
# relogar (ou `sg kvm -c '...'` na sessão atual) pra o grupo valer
```

Os pacotes do AUR já criam `/etc/profile.d/android-*.sh` com `ANDROID_HOME=/opt/android-sdk` pra shells
POSIX. No fish (universal, vale pra todo shell novo):

```fish
set -Ux ANDROID_HOME /opt/android-sdk
set -Ux JAVA_HOME /usr/lib/jvm/java-17-openjdk
set -U fish_user_paths /opt/android-sdk/platform-tools /opt/android-sdk/emulator /opt/android-sdk/cmdline-tools/latest/bin $fish_user_paths
```

(`fish_add_path` dentro de `fish -c` não persistiu aqui; `set -U fish_user_paths` persistiu.)

## Parte sem `sudo`

```bash
fish -l -c 'yes | sdkmanager --licenses >/dev/null; sdkmanager "platforms;android-36" "build-tools;36.0.0" "system-images;android-36;google_apis;x86_64"'
fish -l -c 'echo no | avdmanager create avd -n hangar -k "system-images;android-36;google_apis;x86_64" -d pixel_7'
fish -l -c 'emulator -list-avds'     # → hangar
```

Instalado hoje: build-tools 36.0.0 · emulator 37.1.11 · platform-tools 37.0.1 · platforms/android-36 · system-image android-36 google_apis x86_64.

## Subir o emulador e provar

```bash
fish -l -c 'emulator -avd hangar -no-snapshot -no-audio -gpu swiftshader_indirect' &   # -no-window pra rodar sem tela
fish -l -c 'adb wait-for-device; adb shell getprop sys.boot_completed'               # → 1 (primeiro boot ~36 s aqui)
fish -l -c 'adb exec-out screencap -p' > /caminho/print.png
fish -l -c 'adb emu kill'
```

O emulador enxerga o host como `10.0.2.2`: o backend precisa escutar num IP alcançável
(`CP_LAN_BIND_IP=auto` ou IP da LAN), não só em `127.0.0.1`.
