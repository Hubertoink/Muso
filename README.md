# Muso

A desktop music player for Windows and Linux that lets you browse your local collection like a crate of records. Flip through the covers, put on an album and listen while its artwork dissolves into a visual atmosphere.

## Download

Download the Windows x64 installer, the Linux x86_64 AppImage or the Debian package from [GitHub Releases](https://github.com/Hubertoink/Muso/releases/latest). On Linux, make the AppImage executable before launching it. Linux release packages are built on Ubuntu 24.04.

Built with Rust, Tauri 2, SQLite, Symphonia/Rodio and the `image` crate. Local playback, saved articles and saved images work offline. New AI articles and visual atmospheres use the OpenAI API.

Click the version number at the bottom of the side menu to check for a release. When an update is available, click **Install** to download the signed package for your installation.

Pause on a sleeve to see the beginning of its article. **Read album article** opens the full text. **Write album article with AI** requests an original German music feature for that album with GPT-6.2 Sol and stores it in the local database. For ambient albums, the editorial brief draws on [Headphone Commute](https://headphonecommute.com/)’s attention to changes in sound and space. For other music it combines track-level analysis in the tradition of Pitchfork with the cultural and biographical context of The New Yorker. With research enabled, generating atmospheres also writes a missing article. Muso saves the research locally and includes it in collection archives so later sets can reuse it. Explicitly rewriting an article with research enabled requests a fresh search.

In **Settings → Collection archive**, export a ZIP containing local articles, covers, generated paintings and their styles, plus your genre list, tag mapping and album genres. On another computer, first scan the same music, then import the ZIP. The importer keeps existing articles, style sets and genre choices, fills missing covers and accepts higher-resolution covers. It reports unmatched albums, so you can scan additional folders and import again. Music files, folder paths and credentials stay on their original computer.

The **•••** menu in the article footer lets you delete the saved text or commission a new version. Regeneration follows your current AI research setting and replaces the article after the new text has been saved.

## Using Muso

| Where | Action |
| --- | --- |
| Library | Swipe left on the touchscreen to move the covers right, or swipe right to move them left. Choose **Settings → Touch → Library swipe direction** to make the covers follow your finger. You can also scroll, drag with the mouse or use ← →. PgUp/PgDn jumps 10, Home/End jumps to the ends. |
| Library | Click **Artist** at the left of the alphabet to switch to **Album** (browse by title) and then **Genre**. Click a letter to jump to its first match. In Artist order, “The Beatles” appears under B. In Genre order the bar lists your top-level genres; click the current one again to show its subgenres. Muso remembers the order and selected album on launch. |
| Genres | **AI Features → Organise genres…** opens three steps. **Tidy tags** shows every genre tag from your files as a chip with its album count. Drag one chip onto another (for example Score onto Soundtrack) to merge them into a Muso genre; click a genre chip to rename it, nest it one level deep or take tags out again. Sort the chips by album count or A–Z. **Suggest genres with AI** plays its proposal back as the tags fly into their genres. **Sort albums** asks the AI to place albums from broad categories (for example Dance & DJ) or without a genre into your genres, optionally checking uncertain ones on the web. The optional sorting instructions are saved on this device and accompany both batch classification and web research. They initially ask for Ambient as a separate genre. With web research enabled, Muso counts uncertain albums as it sorts. More than 50 require confirmation before the first lookup. Skipping research keeps the initial suggestions available for review. **Review** shows every suggestion with confidence and reason; nothing changes until you accept. In the player, **⋯ → Genre** sets a genre by hand, which always wins. Muso stores genre changes in its own database and includes them in collection archives. |
| Albums with guests | Both browsing modes keep the complete release under its tagged album artist when the available album-artist credits agree. Without those tags, an artist credited on more than two thirds of the tracks leads the release. Guest performers remain visible in the track row. |
| Compilations | Album order groups tracks with the same album title and release folder, including numbered disc subfolders. Compilations without a principal artist retain their individual artist entries in Artist order. |
| Library | Click the centred sleeve or press Enter to put the record on. |
| While a record plays | Select another album to queue it. Up to five sleeves wait behind the playing record in selection order. Swipe or drag a waiting sleeve up to play it immediately, or down to remove it. On the keyboard, focus the sleeve and use ↑ or ↓. The other records keep their places. |
| New arrivals | Select **NEU** before **#** to browse albums first found in the past 28 days, newest first. Select it again to return to your previous sleeve, or choose a letter to jump into the full collection. Albums already indexed when upgrading count as existing records. |
| Crates | Tap the cover stacks peeking up from the bottom of the library to choose a crate. Browse, search and play its records with the usual library controls. The × beside the crate name returns to your place in the full collection. Pull a sleeve downward and your crates rise from the bottom edge as cover stacks. Drop the sleeve on a stack, or on **New crate** to start another. In the player, drag the current cover onto a crate on the left to file it during playback. Open **Menu → Crates** to see all crates on a shelf. Click a stack to spread it out: drag covers to change the order, drag one down to take it out, click a cover to play from there, and use the icons to play, rename or delete the crate. |
| Library | Just start typing to search, or use Ctrl+F. |
| Corner ● | Opens Search · Settings · Refresh Library · AI Features. |
| Album | Space plays or pauses, ← → change tracks, F or F11 toggles fullscreen, Esc goes back. Visible fullscreen keeps the display awake until you minimize Muso or leave fullscreen. |
| Album track row | Swipe, drag or scroll through the album. Click a title to play it. The playing track is centered with earlier tracks to the left and later tracks to the right. |
| Album ⋯ | Enter or exit fullscreen, show saved Visual Atmospheres, generate new images or find a higher-resolution cover. |

Supported formats are FLAC, MP3, AAC/M4A (including ALAC), OGG Vorbis, WAV and AIFF. Artwork comes from embedded covers first, then from `cover`, `folder` or `front` images in the album folder.

**Settings → Find better covers** searches MusicBrainz and retrieves front covers from the Cover Art Archive. If the lookup is unavailable, ambiguous or missing artwork, Muso also searches [TheAudioDB](https://www.theaudiodb.com/) by artist and album title. Muso fills missing covers and keeps an upgrade when at least one dimension increases and neither decreases. Covers are cached in Muso at up to 3000 pixels on the longest side. Enable **Also write updated front covers to the album’s MP3 files** to embed each accepted cover in the MP3 tracks during an individual or library-wide cover search. This setting is off by default. Newly retrieved covers carry their source and album link across collection exports and imports.

In an individual album search, choose between plausible editions using their previews and provider credits. If both catalogues come up empty, Muso can offer one paid OpenAI web search using your configured API key. It sends the artist, title, year and up to twelve track titles to identify the release using artist and label discographies, then checks both catalogues as needed. You confirm the match before applying it. Library-wide searches skip ambiguous editions and use only the free catalogues.

**Settings → Open my OpenAI API keys** opens the OpenAI Platform in your browser. Sign in with your existing account and paste a project API key into Muso for Visual Atmospheres. Muso can also use an existing `OPENAI_API_KEY` environment variable. Image generation uses OpenAI API billing. Cover searches are independent of OpenAI.

**AI Features → Research the artist and album before generating** prepares a source brief for the exact release, using up to five web searches. Muso reuses the saved brief. Turning research off still lets it use existing notes, credits and the track sequence. No audio is analysed. The [genre taxonomy](GENRE-TAXONOMIE.md) supplies vocabulary for genre suggestions and helps the scene planner distinguish voice, tempo, energy and visual abstraction. Your own genre assignments and sorting preferences guide classification.

Muso plans and reviews all three to five scenes together before painting. Cover language can be prominent in an individual scene and plays a supporting role across the set. Every image request includes the ordered scene descriptions and identifies the single scene to render. GPT-6.1 Terra researches the album. GPT-6.2 Sol plans the series and reviews each image painted by GPT-Image-2.6 Flare. Enable **AI Features → Allow one automatic repaint per scene** to permit one high-quality correction and a second review per scene. This option is off by default. To qualify for publication, a version must have a visible musical connection, respect explicit image constraints and avoid restaging the cover. Muso saves qualifying scenes with their rendering prompts.

New atmospheres are composed for widescreen playback and saved as **1920 × 1080 JPEGs**. The image API accepts dimensions in multiples of 16. Muso requests 1920 × 1088 and crops four pixels from the top and bottom. **Show Visual Atmospheres** in the album menu displays the saved images across the player window.

**Menu → Images** indexes JPEG, PNG and WebP files in selected folders and their subfolders. Use **Rescan folders** after adding, changing or deleting files. Removing a folder releases its entries in Muso and keeps your originals on disk. Connect external drives when playing their images.

The player and TV share a random selection of up to five personal images when an album has no saved atmospheres. Enable **Mix up to 5 folder images** in **Images** to include that selection alongside an album’s saved atmospheres. Folder images are spaced throughout the series on both screens. **Classify images** sends a reduced preview of each pending image to GPT-6 Luna using your OpenAI key. You can stop after the current image and continue later. **Match classified images to the playing album** uses saved album profiles offline. Mood and visual density carry extra weight alongside energy and abstraction. Muso draws at random among the closest available pictures. Older profiles still match on their known dimensions. Without a saved profile, Muso uses broad visual cues for recognised genres and otherwise chooses freely. To create an album-specific profile, enable **Allow paid analysis** and play an album that uses personal images. GPT-6.1 Terra evaluates its metadata and available research. After the profile is saved, paid analysis can be turned off again.

**Settings → Vinyl playback** adds the sound of dropping the needle at the start of an album, changing the record when another album takes over, and the end groove when the last record finishes. The groove runs for two minutes by default. Tap **−** or **+** to adjust it by half a minute, or enter a duration in minutes. A waiting record takes over directly.

**Menu → Audio output** lets you switch between this computer, a Denon HEOS room and an existing multi-room group. **Create group** lets you choose the main room and tick the rooms to play with it. **Edit group** changes its members or separates all rooms. Group changes apply to your HEOS system. Use **Refresh** to pick up changes made in the HEOS app. Muso looks on the local network and also accepts a speaker’s IPv4 address.

In the player, move the pointer or tap to reveal the volume button at the bottom right. Select it and drag up or down to adjust Muso on this computer, the selected HEOS room, or the whole active group. Muso remembers the local volume. During HEOS playback, **⋯ → HEOS volume** also gives each room in the group its own slider.

Muso sends the playback mix, including the vinyl sounds, as a continuous 320 kbps stereo MP3 stream over the home network. The computer must stay awake and reachable from the speakers. Pause and resume also control the speaker directly. Receivers that only support stopping a live stream need to buffer it again on resume. Initial playback and TV synchronization depend on the receiver’s buffer.

**Menu → TV display** opens a separate fullscreen window on the selected screen. Use an extended desktop on Windows or Omarchy. The controller shares the album queue. Swipe a waiting sleeve up to switch and return to the library, or go back and let it play in turn. The TV shows the playing track’s artist and title for the first ten seconds of playback and cycles through the album’s saved atmospheres. The cover fills in when no paintings are available. Enable the visualizer in **EQ Settings** to show the live spectrum over the TV artwork. Changes to the EQ style, colour, sensitivity, height and intensity also apply to the TV. After one minute of inactivity in the library, the controller fades in the artwork currently on the TV. Move the mouse or touch the controller to fade back to the same spot in the library. Close the TV from the same menu or press Escape in the TV window.

Each track opens with its cover for the first ten seconds of playback. When an atmosphere is ready, the cover dissolves into it. Moving the pointer brings back the track details and controls. In **Album ⋯ → Atmosphere style**, choose from 22 visual styles, including Postmodern Realism, Surreal Realism, Urban Photography, Landscape Photography and Pixel Art. Pixel Art uses crisp silhouettes and flat pixel clusters; Dithered Worlds builds tonal transitions with patterned dots. Modern Street Art paints one continuous mural. Bauhaus Study is the preset for cut-paper compositions. Muso previews the look on available images and remembers the choice per album. Generating creates a set in the selected style and replaces any previous set in that style. Older atmospheres are available under Cinematic.

The first finished painting fades in as soon as the track’s cover intro has ended. The remaining paintings join the rotation as they are saved. Each JPEG includes the album title, visual style and sequence number in its filename. **Album ⋯ → Open image folder** opens the generated images in your file manager. On Windows, Explorer selects an image from the current album when one is available.

In **AI Features**, enable **Generate on playback for albums without atmospheres** and choose 3, 4 or 5 images. This is off by default. When playback starts, Muso checks for saved images across all styles before using your API key. The chosen style and research settings apply to the new set.

In **AI Features**, turn on **Use AI choice as the default atmosphere style** to let the scene planner develop a medium for the whole album from the music, optional research and the visual language of its cover. It can adapt or combine techniques and choose outside the catalogue. The planner sees the most frequent underlying techniques and the last eight automatic choices in order. When several techniques suit the music equally well, it favours variety. Each series keeps its own style name. A manually selected preset uses its specified medium and surface. Open **Album ⋯ → Atmosphere style** or **AI Features → Style for this album** to flip through the style covers with a swipe, drag, mouse wheel or arrow keys. Click a side cover to centre it. Click the centre cover or **Use this style** to apply it to this album. Muso remembers it for that album. Use the default button below the covers to restore the default. Images generated with AI choice are saved as a separate set and the menu shows the chosen medium by name.

Enable **AI Features → Commission a random artist persona for each series** to give each new atmosphere series one of six artistic perspectives. The documentary photographer looks for a revealing moment, the material artist explores a physical process, the illustrator follows gestures and shapes, the scenographer works with space and sightlines, the painter observes colour and edges, and the graphic designer uses rhythm and spacing. Each has an equal chance. You can see the assigned role while Muso plans the series. With AI choice, the role helps select the technique. With a preset, it guides composition within your chosen medium.

Select **Enter fullscreen** in the main menu, or press **F11** anywhere to toggle fullscreen. Under **Settings → Atmosphere effects**, choose drifting light, film grain, colour haze or off for the images shown in the player after the cover intro. Pointer and touch ripples respond to speed and changes of direction.

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
  ai.rs              OpenAI: GPT-6.1 Terra researches, GPT-6.2 Sol writes, plans and reviews, GPT-Image-2.6 Flare paints
```

The crate only renders about 19 sleeves around the focused record, so scrolling stays smooth with more than 10,000 albums.

## License

Muso is available under the [MIT License](LICENSE). The bundled Lucide icons retain their [original license notices](ui/icons/LICENSE-Lucide.txt).
