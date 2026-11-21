# Third-party notices

## libmpv (mpv and FFmpeg) — Windows only

On Windows, `scripts/install-libmpv.ps1` places `libmpv-2.dll` next to `Hangar.exe`. The desktop app loads it at
run time to play videos inside the conversation. Hangar does not modify it.

- Binary: `libmpv-2.dll` from the `mpv-dev-x86_64-20261111-git-bcde6b93cc.7z` asset of the
  [shinchiro/mpv-winbuild-cmake release `20261111`](https://github.com/shinchiro/mpv-winbuild-cmake/releases/tag/20261111)
  - archive SHA-256 `273c079b11ea5575822f972756f4c9a71149a5c10130353abeb8586738938452`
  - `libmpv-2.dll` SHA-256 `88504ee5560942a955405e99dfb5971368e4cf851321f36565bf2a667271db1b`
- Build recipe: [shinchiro/mpv-winbuild-cmake at `bda10a2816ce9908d419b3f4b564227330457e76`](https://github.com/shinchiro/mpv-winbuild-cmake/tree/bda10a2816ce9908d419b3f4b564227330457e76),
  built by [workflow run 42927840447](https://github.com/shinchiro/mpv-winbuild-cmake/actions/runs/42927840447).
- mpv `v0.42.0-38-gbcde6b93c`: source at
  [mpv-player/mpv `bcde6b93cca4d128d84e1a60d1a6db2cd81c889e`](https://github.com/mpv-player/mpv/tree/bcde6b93cca4d128d84e1a60d1a6db2cd81c889e).
  mpv is licensed under GPL-2.0-or-later (some parts LGPL-2.1-or-later); see `Copyright` in its source.
- FFmpeg `N-128766-gcd4e96eb3`: source at
  [FFmpeg/FFmpeg `cd4e96eb314630788b7ff2f9820c395788c023e2`](https://github.com/FFmpeg/FFmpeg/tree/cd4e96eb314630788b7ff2f9820c395788c023e2).
  The recipe configures it with `--enable-gpl --enable-version3`, which makes this FFmpeg GPL-3.0-or-later.
- The DLL also statically links the libraries listed in the recipe's `packages/` folder (x264, x265, libass,
  dav1d, libplacebo, OpenSSL and others), each under its own license and fetched by that build.

Taken together, `libmpv-2.dll` is distributed under the
[GNU General Public License, version 3 or later](https://www.gnu.org/licenses/gpl-3.0.html). It comes with no
warranty. If any source link above stops working, ask in
[the Hangar issues](https://github.com/jeffer1312/hangar/issues) and the corresponding source will be provided.

## Linux and macOS

Hangar does not ship libmpv there. The installer asks the system package manager (`mpv`, `libmpv2` or `mpv-libs`)
or Homebrew (`mpv`) to install it, under the license of that package.
