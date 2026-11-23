# Muso

A desktop music player for Windows and Linux that lets you browse your local collection like a crate of records. Flip through the covers, put on an album and listen while its artwork dissolves into a visual atmosphere.

## Download

Download the Windows x64 installer, the Linux x86_64 AppImage or the Debian package from [GitHub Releases](https://github.com/Hubertoink/Muso/releases/latest). On Linux, make the AppImage executable before launching it. Linux release packages are built on Ubuntu 24.04.

Built with Rust, Tauri 2, SQLite, Symphonia/Rodio and the `image` crate. It works fully offline. OpenAI is only used for the optional visual atmospheres.

## Using Muso

| Where | Action |
| --- | --- |
| Library | Scroll, swipe, drag or use ← → to flip through records. PgUp/PgDn jumps 10, Home/End jumps to the ends. |
| Library | Click the centred sleeve or press Enter to put the record on. |
| Library | Just start typing to search, or use Ctrl+F. |
| Corner ● | Opens Search · Settings · Refresh Library · AI Features. |
| Album | Space plays or pauses, ← → change tracks, F or F11 toggles fullscreen, Esc goes back. |
| Album track row | Swipe, drag or scroll through the album. Click a title to play it. The playing track is centered with earlier tracks to the left and later tracks to the right. |
| Album ⋯ | Enter or exit fullscreen, show saved Visual Atmospheres, generate new images or find a higher-resolution cover. |

Supported formats are FLAC, MP3, AAC/M4A (including ALAC), OGG Vorbis, WAV and AIFF. Artwork comes from embedded covers first, then from `cover`, `folder` or `front` images in the album folder.

**Settings → Find better covers** searches MusicBrainz by artist and album title and downloads matching front covers from the Cover Art Archive. Muso compares the image dimensions with its current display copy. It keeps an upgrade when at least one dimension increases and neither decreases. Covers are cached locally at up to 3000 pixels on the longest side. The search can also fill missing covers. It only writes to Muso’s app data directory.

**Settings → Open my OpenAI API keys** opens the OpenAI Platform in your browser. Sign in with your existing account and paste a project API key into Muso for Visual Atmospheres. Muso can also use an existing `OPENAI_API_KEY` environment variable. Image generation uses OpenAI API billing. Cover searches are independent of OpenAI.

**AI Features → Research the artist and album before generating** adds one OpenAI web search for documented details about the recording and its tracks. Muso also measures the dynamics of short excerpts from up to three local tracks. Audio stays on the device. Each scene translates a distinct musical cue into composition, light and texture, using the selected style as its medium. **Show Visual Atmospheres** in the album menu displays the saved images across the player window. Select it again to return to the cover.

Each track opens with its cover for the first ten seconds of playback. When an atmosphere is ready, the cover dissolves into it. Moving the pointer brings back the track details and controls. In **Album ⋯ → Atmosphere style**, choose Cinematic, Dreamscape, Ambient Drift, Tape Decay or Silver Photogram. Muso previews the look on available images and remembers the choice per album. Generating creates a set in the selected style and replaces any previous set in that style. Older atmospheres are available under Cinematic.

**Album ⋯ → Open image folder** opens the generated images in your file manager. On Windows, Explorer selects an image from the current album when one is available.

Under **Settings → Atmosphere effects**, choose drifting light, film weave, colour haze or off. Adjust the strength and toggle ripples for mouse movement and touch. The effects run locally over the playing atmosphere. The system's reduced-motion setting disables the animated filters and ripples.

Your data (the SQLite index, cached artwork and atmospheres) is stored in the app data directory:
- Windows: `%APPDATA%\app.muso.player`
- Linux: `~/.local/share/app.muso.player`

## Development

```sh
npm install
npm run dev                      # run with hot reload of the UI
npx tauri build --no-bundle      # release binary in src-tauri/target/release/
npx tauri build                  # installers / packages
```

### Windows

Install [Rust](https://rustup.rs), Node.js and the Visual Studio C++ Build Tools. WebView2 is preinstalled on Windows 10 and 11.

### Arch Linux / Omarchy

```sh
sudo pacman -S --needed base-devel rustup nodejs npm webkit2gtk-4.1 \
  libappindicator-gtk3 librsvg openssl alsa-lib pipewire-alsa xdotool
rustup default stable
npm install
npm run dev
```

To install it as a normal desktop app, build a package with `npx tauri build --bundles appimage` (or `deb` / `rpm`). You can also just run `src-tauri/target/release/muso`.

On some NVIDIA or Hyprland setups, WebKitGTK renders a blank window. If that happens, start Muso with:

```sh
WEBKIT_DISABLE_DMABUF_RENDERER=1 ./muso
```

## Architecture

```
ui/                  Vanilla HTML/CSS/JS (no bundler) – crate, overlay, album view
src-tauri/src/
  main.rs            Tauri commands, muso:// protocol for cached images
  db.rs              SQLite schema + queries (folders, albums, tracks, atmospheres, settings)
  scanner.rs         Incremental recursive scan (mtime based), tags via lofty, artwork → JPEG cache
  audio.rs           Dedicated audio thread (rodio + symphonia), emits track-ended
  ai.rs              OpenAI: gpt-4o-mini writes 3 scene prompts → gpt-image-2.5-flare paints them
```

The crate only renders about 19 sleeves around the focused record, so scrolling stays smooth with more than 10,000 albums.

## License

Muso is available under the [MIT License](LICENSE). The bundled Lucide icons retain their [original license notices](ui/icons/LICENSE-Lucide.txt).
