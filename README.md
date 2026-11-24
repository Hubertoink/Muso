# Muso

A desktop music player for Windows and Linux that lets you browse your local collection like a crate of records. Flip through the covers, put on an album and listen while its artwork dissolves into a visual atmosphere.

## Download

Download the Windows x64 installer, the Linux x86_64 AppImage or the Debian package from [GitHub Releases](https://github.com/Hubertoink/Muso/releases/latest). On Linux, make the AppImage executable before launching it. Linux release packages are built on Ubuntu 24.04.

Built with Rust, Tauri 2, SQLite, Symphonia/Rodio and the `image` crate. Local playback, saved articles and saved images work offline. New AI articles and visual atmospheres use the OpenAI API.

Click the version number at the bottom of the side menu to check for a release. When an update is available, click **Install** to download the signed package for your installation.

Pause on a sleeve to see the beginning of its article. **Read album article** opens the full text. **Write album article with AI** requests an original German music feature for that album with GPT-6.1 Terra and stores it in the local database. With research enabled, generating atmospheres also writes a missing article. Muso saves the research locally and includes it in collection archives so later sets can reuse it. Explicitly rewriting an article with research enabled requests a fresh search.

In **Settings → Collection archive**, export a ZIP containing local articles, covers, generated paintings and their styles, plus your genre list, tag mapping and album genres. On another computer, first scan the same music, then import the ZIP. The importer keeps existing articles, style sets and genre choices, fills missing covers and accepts higher-resolution covers. It reports unmatched albums, so you can scan additional folders and import again. Music files, folder paths and credentials stay on their original computer.

The **•••** menu in the article footer lets you delete the saved text or commission a new version. Regeneration follows your current AI research setting and replaces the article after the new text has been saved.

## Using Muso

| Where | Action |
| --- | --- |
| Library | Swipe left on the touchscreen to move the covers right, or swipe right to move them left. Choose **Settings → Touch → Library swipe direction** to make the covers follow your finger. You can also scroll, drag with the mouse or use ← →. PgUp/PgDn jumps 10, Home/End jumps to the ends. |
| Library | Click **Artist** at the left of the alphabet to switch to **Album** (browse by title) and then **Genre**. Click a letter to jump to its first match. In Artist order, “The Beatles” appears under B. In Genre order the bar lists your top-level genres; click the current one again to show its subgenres. Muso remembers the order and selected album on launch. |
| Genres | **AI Features → Organise genres…** opens three steps. **Tidy tags** shows every genre tag from your files as a chip with its album count. Drag one chip onto another (for example Score onto Soundtrack) to merge them into a Muso genre; click a genre chip to rename it, nest it one level deep or take tags out again. Sort the chips by album count or A–Z. **Suggest genres with AI** plays its proposal back as the tags fly into their genres. **Sort albums** asks the AI to place albums from broad categories (for example Dance & DJ) or without a genre into your genres, optionally checking uncertain ones on the web. The optional sorting instructions are saved on this device and accompany both batch classification and web research. They initially ask for Ambient as a separate genre. With web research enabled, Muso counts uncertain albums as it sorts. More than 50 require confirmation before the first lookup. Skipping research keeps the initial suggestions available for review. **Review** shows every suggestion with confidence and reason; nothing changes until you accept. In the player, **⋯ → Genre** sets a genre by hand, which always wins. Muso stores genres in its own database and never writes to your music files; collection archives carry them to another Muso. |
| Albums with guests | Both browsing modes keep the complete release under its tagged album artist when the available album-artist credits agree. Without those tags, an artist credited on more than two thirds of the tracks leads the release. Guest performers remain visible in the track row. |
| Compilations | Album order groups tracks with the same album title and release folder, including numbered disc subfolders. Compilations without a principal artist retain their individual artist entries in Artist order. |
| Library | Click the centred sleeve or press Enter to put the record on. |
| New arrivals | Select **NEU** before **#** to browse albums first found in the past 28 days, newest first. Select it again to return to your previous sleeve, or choose a letter to jump into the full collection. Albums already indexed when upgrading count as existing records. |
| Crates | Pull a sleeve downward and your crates rise from the bottom edge as cover stacks. Drop the sleeve on a stack, or on **New crate** to start another. In the player, drag the current cover onto a crate on the left to file it during playback. Open **Menu → Crates** to see all crates on a shelf. Click a stack to spread it out: drag covers to change the order, drag one down to take it out, click a cover to play from there, and use the icons to play, rename or delete the crate. |
| Library | Just start typing to search, or use Ctrl+F. |
| Corner ● | Opens Search · Settings · Refresh Library · AI Features. |
| Album | Space plays or pauses, ← → change tracks, F or F11 toggles fullscreen, Esc goes back. Visible fullscreen keeps the display awake until you minimize Muso or leave fullscreen. |
| Album track row | Swipe, drag or scroll through the album. Click a title to play it. The playing track is centered with earlier tracks to the left and later tracks to the right. |
| Album ⋯ | Enter or exit fullscreen, show saved Visual Atmospheres, generate new images or find a higher-resolution cover. |

Supported formats are FLAC, MP3, AAC/M4A (including ALAC), OGG Vorbis, WAV and AIFF. Artwork comes from embedded covers first, then from `cover`, `folder` or `front` images in the album folder.

**Settings → Find better covers** searches MusicBrainz by artist and album title and downloads matching front covers from the Cover Art Archive. Muso compares the image dimensions with its current display copy. It keeps an upgrade when at least one dimension increases and neither decreases. Covers are cached locally at up to 3000 pixels on the longest side. The search can also fill missing covers. It only writes to Muso’s app data directory.

If an individual album search cannot identify the release, Muso can offer one paid OpenAI web search using your configured API key. It sends the artist, title, year and up to twelve track titles. You review the matching MusicBrainz credit before Muso retrieves its front cover. Library-wide cover searches use MusicBrainz and the Cover Art Archive only.

**Settings → Open my OpenAI API keys** opens the OpenAI Platform in your browser. Sign in with your existing account and paste a project API key into Muso for Visual Atmospheres. Muso can also use an existing `OPENAI_API_KEY` environment variable. Image generation uses OpenAI API billing. Cover searches are independent of OpenAI.

**AI Features → Research the artist and album before generating** uses one OpenAI web search when an album has no saved research. The brief captures how artists, labels and critics describe the recording and its genre influences. The scene planner uses this brief and the album article for the palette, setting, materials and differences between tracks. With web research off, the planner still uses previously saved research and articles. The planner compares the assigned library genre, the original tag and a small cover preview. A contact sheet of up to six saved paintings, sampled across albums, helps it choose different subjects, framing and light. Missing artwork is optional. No audio is analysed for scene planning. The final prompt for each new painting is saved locally. The selected style determines the medium. **Show Visual Atmospheres** in the album menu displays the saved images across the player window. Select it again to return to the cover.

Each track opens with its cover for the first ten seconds of playback. When an atmosphere is ready, the cover dissolves into it. Moving the pointer brings back the track details and controls. In **Album ⋯ → Atmosphere style**, choose from 22 visual styles, including Postmodern Realism, Surreal Realism, Urban Photography, Landscape Photography and Pixel Art. Pixel Art uses crisp silhouettes and flat pixel clusters; Dithered Worlds builds tonal transitions with patterned dots. Modern Street Art paints one continuous mural. Bauhaus Study is the preset for cut-paper compositions. Muso previews the look on available images and remembers the choice per album. Generating creates a set in the selected style and replaces any previous set in that style. Older atmospheres are available under Cinematic.

The first finished painting fades in as soon as the track’s cover intro has ended. The remaining paintings join the rotation as they are saved. Each JPEG includes the album title, visual style and sequence number in its filename. **Album ⋯ → Open image folder** opens the generated images in your file manager. On Windows, Explorer selects an image from the current album when one is available.

In **AI Features**, enable **Generate on playback for albums without atmospheres** and choose 3, 4 or 5 images. This is off by default. When playback starts, Muso checks for saved images across all styles before using your API key. The chosen style and research settings apply to the new set.

In **AI Features**, turn on **Use AI choice as the default atmosphere style** to let the scene planner choose a medium from the music, genre and optional research. Open **Album ⋯ → Atmosphere style** or **AI Features → Style for this album** to flip through the style covers with a swipe, drag, mouse wheel or arrow keys. Click a side cover to centre it. Click the centre cover or **Use this style** to apply it to this album. Muso remembers it for that album. Use the default button below the covers to restore the default. Images generated with AI choice are saved as a separate set and the menu shows the chosen medium by name.

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
  ai.rs              OpenAI: gpt-5-mini plans scenes from the album and visual references → gpt-image-2.5-flare paints them
```

The crate only renders about 19 sleeves around the focused record, so scrolling stays smooth with more than 10,000 albums.

## License

Muso is available under the [MIT License](LICENSE). The bundled Lucide icons retain their [original license notices](ui/icons/LICENSE-Lucide.txt).
