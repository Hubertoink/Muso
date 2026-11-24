# Muso

A desktop music player for Windows and Linux that lets you browse your local collection like a crate of records. Flip through the covers, put on an album and listen while its artwork dissolves into a visual atmosphere.

## Download

Download the Windows x64 installer, the Linux x86_64 AppImage or the Debian package from [GitHub Releases](https://github.com/Hubertoink/Muso/releases/latest). On Linux, make the AppImage executable before launching it. Linux release packages are built on Ubuntu 24.04.

Built with Rust, Tauri 2, SQLite, Symphonia/Rodio and the `image` crate. Local playback, saved articles and saved images work offline. New AI articles and visual atmospheres use the OpenAI API.

Click the version number at the bottom of the side menu to check for a release. When an update is available, click **Install** to download the signed package for your installation.

Pause on a sleeve to see the beginning of its article. **Read album article** opens the full text. **Write album article with AI** requests an original German music feature for that album with GPT-6.1 Terra and stores it in the local database. Browsing reads from the local collection. The optional research switch in AI Features adds one billed web search to new articles.

In **Settings → Collection archive**, export a ZIP containing local articles, covers, generated paintings and their styles. On another computer, first scan the same music, then import the ZIP. The importer keeps existing articles and style sets, fills missing covers and accepts higher-resolution covers. It reports unmatched albums, so you can scan additional folders and import again. Music files, folder paths and credentials stay on their original computer.

The **•••** menu in the article footer lets you delete the saved text or commission a new version. Regeneration follows your current AI research setting and replaces the article after the new text has been saved.

## Using Muso

| Where | Action |
| --- | --- |
| Library | Swipe left on the touchscreen to move the covers right, or swipe right to move them left. Choose **Settings → Touch → Library swipe direction** to make the covers follow your finger. You can also scroll, drag with the mouse or use ← →. PgUp/PgDn jumps 10, Home/End jumps to the ends. |
| Library | Click **Artist** at the left of the alphabet to switch to **Album** and browse by title. Click a letter to jump to its first match. In Artist order, “The Beatles” appears under B. Muso remembers the order and selected album on launch. |
| Compilations | Album order shows one sleeve for tracks with the same album title in the same release folder, including numbered CD or disc subfolders. Open it to play all artists in disc and track order. Artist order keeps the individual artist entries. |
| Library | Click the centred sleeve or press Enter to put the record on. |
| Crates | Pull a sleeve downward and your crates rise from the bottom edge as cover stacks. Drop the sleeve on a stack, or on **New crate** to start another. Open **Menu → Crates** to see all crates on a shelf. Click a stack to spread it out: drag covers to change the order, drag one down to take it out, click a cover to play from there, and use the icons to play, rename or delete the crate. |
| Library | Just start typing to search, or use Ctrl+F. |
| Corner ● | Opens Search · Settings · Refresh Library · AI Features. |
| Album | Space plays or pauses, ← → change tracks, F or F11 toggles fullscreen, Esc goes back. |
| Album track row | Swipe, drag or scroll through the album. Click a title to play it. The playing track is centered with earlier tracks to the left and later tracks to the right. |
| Album ⋯ | Enter or exit fullscreen, show saved Visual Atmospheres, generate new images or find a higher-resolution cover. |

Supported formats are FLAC, MP3, AAC/M4A (including ALAC), OGG Vorbis, WAV and AIFF. Artwork comes from embedded covers first, then from `cover`, `folder` or `front` images in the album folder.

**Settings → Find better covers** searches MusicBrainz by artist and album title and downloads matching front covers from the Cover Art Archive. Muso compares the image dimensions with its current display copy. It keeps an upgrade when at least one dimension increases and neither decreases. Covers are cached locally at up to 3000 pixels on the longest side. The search can also fill missing covers. It only writes to Muso’s app data directory.

**Settings → Open my OpenAI API keys** opens the OpenAI Platform in your browser. Sign in with your existing account and paste a project API key into Muso for Visual Atmospheres. Muso can also use an existing `OPENAI_API_KEY` environment variable. Image generation uses OpenAI API billing. Cover searches are independent of OpenAI.

**AI Features → Research the artist and album before generating** adds one OpenAI web search for the way artists, labels and critics describe the album and its genre influences. These descriptions and the library genre tag guide the palette, setting and materials shared by all three images. Details of the recording and dynamics measured from short excerpts of up to three local tracks give each scene its own composition. Audio stays on the device. The selected style determines the medium. **Show Visual Atmospheres** in the album menu displays the saved images across the player window. Select it again to return to the cover.

Each track opens with its cover for the first ten seconds of playback. When an atmosphere is ready, the cover dissolves into it. Moving the pointer brings back the track details and controls. In **Album ⋯ → Atmosphere style**, choose Cinematic, Dreamscape, Ambient Drift, Tape Decay or Silver Photogram. Muso previews the look on available images and remembers the choice per album. Generating creates a set in the selected style and replaces any previous set in that style. Older atmospheres are available under Cinematic.

The first finished painting fades in as soon as the track’s cover intro has ended. The remaining paintings join the rotation as they are saved. Each JPEG includes the album title, visual style and sequence number in its filename. **Album ⋯ → Open image folder** opens the generated images in your file manager. On Windows, Explorer selects an image from the current album when one is available.

In **AI Features**, turn on **Let AI choose a visual style for each album** to let the scene planner choose a medium from the music, genre and optional research. The next generation saves its images as a separate set and shows the chosen style by name. Turn the option off to use your manual style selections again.

Select **Enter fullscreen** in the main menu, or press **F11** anywhere to toggle fullscreen. Under **Settings → Atmosphere effects**, choose drifting light, film weave, colour haze or off. Pointer and touch ripples respond to speed and changes of direction.

Select **EQ** directly below **Settings** in the main menu, or **EQ Settings** in the album menu. Sixteen frequency bands drive the wave, with bass on the left and treble on the right. Each band rises and falls in place with the music. **Ripple line** bends image details, **Echo lines** adds parallel contours, and **Rising pigment** sends a gradient of your chosen colour up from the wave. Sensitivity, wave height and intensity are saved independently of the other image effects. The system’s reduced-motion setting disables animated effects.

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

To run the library touch regression tests, install the headless test browser once with `npx playwright-core install chromium --only-shell`, then run `npm test`. To use an existing Chromium installation, set `CHROMIUM_PATH` to its executable.

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
