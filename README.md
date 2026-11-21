<p align="center"><img src="native/assets/crate.svg" width="96" alt="Crate icon"></p>

<h1 align="center">Crate</h1>

<p align="center">A desktop music player for your own music library on Linux.<br>
It plays from <a href="https://www.navidrome.org/">Navidrome</a>, finds songs you don't have yet, and feels like the streaming apps you're used to.</p>

---

## What this is (and isn't)

Crate is a **personal project**. I made it for myself because self-hosting music meant juggling
a server, a downloader, a tagger and a player, and none of them talked to each other. Crate
puts them in one window.

I'm sharing it because I have no reason not to. **There are no promises:** no support, no
roadmap, no guarantee it works on your setup. If it breaks, you get to keep both pieces. Issues
and fixes are welcome, but I may be slow to respond.

### About AI

I'll be upfront: **most of Crate's code was written with an AI assistant** (Anthropic's Claude).
I described what I wanted, tested it, and pushed back until it worked the way I wanted. I
did make sure it runs efficiently: it idles at well under 1% CPU and plays music in about
50 MB of memory.

I'm largely against AI use in most subjects. This was a small thing I wanted for my own
convenience, and that's the context it was made in.

What matters for you as a user:

- **There is no AI in the app.** Crate doesn't generate, recommend, "enhance" or analyse
  anything with AI, and it never sends your music or listening anywhere for that.
- **Nothing in it is made up.** Your songs, covers, lyrics and catalog data all come from
  services and people listed below. Crate just puts them together.

## Features

- **Your library, Spotify-style.** Playlists, albums and artists, search, a queue, shuffle and
  repeat, gapless playback and media keys. It also shows up in your desktop's "now playing"
  widget (MPRIS).
- **Find what you don't have.** Search the Deezer catalog and click a song to get it. Crate
  downloads it from Soulseek (or YouTube if Soulseek doesn't have it), tags it with proper
  artist, album, cover and track details, files it into your library, and adds it to the
  playlist you picked.
- **Import from Spotify.** Export your playlists with [Exportify](https://exportify.net) and give
  Crate the CSV files. Songs you already have go straight in; the rest are downloaded into the
  playlist.
- **Fix library.** Finds songs you have twice by how they *sound* (audio fingerprints), merges
  albums split across two spellings, and fills in missing covers and details. It shows you
  everything first, moves duplicates to a review folder instead of deleting them, and can be
  undone.
- **Synced lyrics**, with romaji under Japanese lyrics if you want it.
- **Volume normalization** while playing. Your files are never changed for this.
- **Playlist editing**: drag to reorder, multi-select, rename, and undo.
- **Themes** (dark, OLED black, dim, light), accent colours (including one taken from the album
  art), text size, row density, and optional animations.
- **Native and light.** Written in Rust with a GPU-drawn interface (gpui), with no web browser
  inside.

## Requirements

- **Linux**, on Wayland or X11, with a working GPU driver (Vulkan).
- **Navidrome.** Crate can install and run one for you, or sign in to one you already have.
- **ffmpeg** and **fpcalc** (Chromaprint), from your package manager. If they're missing, setup
  installs them for you and your system asks for your password.
- **sockseek** and **yt-dlp** for downloads. Crate can download these itself during setup.
- A **Soulseek account** for downloads. Setup creates one for you, since Soulseek has no
  sign-up page.
- The [Inter](https://rsms.me/inter/) font looks best (most distros package it).

## Install

You need a Rust toolchain ([rustup.rs](https://rustup.rs)).

```sh
git clone https://github.com/mossriff/crate-desktop
cd crate-desktop
native/tools/install.sh
```

This builds Crate (the first build takes a while) and installs `~/.local/bin/crate`, its icon,
and a "Crate" entry in your app launcher.

The first time you open it, setup walks you through:

1. checking the helper programs,
2. Navidrome: "set it up for me" or "I already have one",
3. your Soulseek account,
4. optionally, importing your Spotify playlists.

You can redo any of these later in **Settings → Accounts**.

## Where things are kept

| What | Where |
| --- | --- |
| Navidrome sign-in (a salted token, never your password) | `~/.config/crate/config.json` |
| Soulseek sign-in (read by sockseek) | `~/.config/sockseek/sockseek.conf` |
| Settings | `~/.config/crate/desktop-settings.json` |
| A Navidrome that Crate set up | `~/.local/share/crate/navidrome/` (runs as the user service `crate-navidrome`) |
| Downloaded helper programs | `~/.local/share/crate/bin/` |
| Audio, cover, lyrics and loudness caches | `~/.cache/crate/` (audio is capped at 1 GB) |
| Duplicates moved by Fix library | `~/music-dupes-review/` |

Both sign-in files are readable only by you.

## Who actually does the work

Crate is glue. Everything below belongs to other people:

- [Navidrome](https://www.navidrome.org/) stores and serves your library.
- [Soulseek](https://www.slsknet.org/), and the people sharing their collections on it, are where
  the music comes from. [sockseek](https://github.com/fiso64/sockseek) does the searching and
  downloading.
- [yt-dlp](https://github.com/yt-dlp/yt-dlp) is the YouTube fallback.
- The [Deezer API](https://developers.deezer.com/) provides catalog search, tags and covers.
  Crate doesn't stream anything from Deezer.
- [LRCLIB](https://lrclib.net/) provides lyrics.
- [Chromaprint](https://acoustid.org/chromaprint) does the audio fingerprints, and
  [FFmpeg](https://ffmpeg.org/) handles conversion and loudness.
- [Exportify](https://exportify.net) gets your playlists out of Spotify.
- [gpui](https://www.gpui.rs/), [gpui-component](https://github.com/longbridge/gpui-component),
  [rodio](https://github.com/RustAudio/rodio), [lindera](https://github.com/lindera/lindera)
  (IPADIC dictionary) and kakasi are the libraries Crate is built on.

## Please be decent

Crate downloads from a peer-to-peer network and from YouTube. **What you download, and whether
you're allowed to, is your responsibility** under the laws where you live. If you love an
artist, buy their music, go to their shows, and support them directly. Bandcamp is great for
that.

## For developers

Crate is a Cargo workspace:

- `core/` has everything that isn't UI: the Navidrome client, audio engine, downloads, the
  library fixer, lyrics and romaji, setup and import. Its `examples/` folder has small test
  programs.
- `native/` is the app (gpui).

`native/tools/shot.sh` runs Crate inside an invisible, private KWin session and screenshots it,
so you can check the UI without a window opening on your desktop. `native/tools/measure.sh`
reports its CPU and memory use the same way.

## License

[GPL-3.0-or-later](LICENSE). You can use, study, share and change Crate. If you share a changed
version, share its source under the same license.
