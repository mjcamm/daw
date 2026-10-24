# Third-party licences

This project's own code is licensed under MIT OR Apache-2.0 (`LICENSE-MIT`, `LICENSE-APACHE`). It is built with open-source libraries under their own licences. Most are permissive (MIT, Apache-2.0, BSD, ISC, Zlib, Unicode); the ones below come with extra conditions and are listed here so they are met. The full texts are in `licenses/`.

## MP3 encoding: LAME (LGPL)

MP3 export uses **LAME**, the MP3 encoder (<https://lame.sourceforge.io>, formerly www.mp3dev.org), through two Rust crates:

| Component | Version | Licence | Source |
|---|---|---|---|
| LAME | 3.100 | GNU LGPL 2 (`licenses/LGPL-2.0.txt`) | <https://lame.sourceforge.io>, also bundled in the `mp3lame-sys` crate |
| `mp3lame-sys` (builds LAME, raw bindings) | 0.1.11 | GNU LGPL 3 (`licenses/LGPL-3.0.txt`) | <https://crates.io/crates/mp3lame-sys> |
| `mp3lame-encoder` (Rust bindings) | 0.2.5 | GNU LGPL 3 (`licenses/LGPL-3.0.txt`) | <https://github.com/DoumanAsh/mp3lame-encoder> |

This software uses LAME. All three are used unmodified. The LGPL covers them only, not the rest of this project.

**Replacing or rebuilding them.** LAME is compiled into the app. The LGPL lets you use your own version of it instead: the whole of this project's source is public, so you can rebuild the app against a modified LAME or bindings (for example with a `[patch.crates-io]` entry in `Cargo.toml`), using the build commands in `CLAUDE.md`.

**Building without them.** MP3 export is the `mp3` feature of the `daw-host` crate, on by default. Turn it off (add `default-features = false` to `daw-host` under `[workspace.dependencies]` in `Cargo.toml`) and the app contains no LGPL code; export then offers WAV only. See `docs/design/export.md`.

## MPL-2.0 libraries

These are under the Mozilla Public License 2.0 (`licenses/MPL-2.0.txt`), used unmodified. Their source is at the links below.

| Library | Version | Used for | Source |
|---|---|---|---|
| `audio_thread_priority` | 0.35.1 | Audio thread priority | <https://crates.io/crates/audio_thread_priority> |
| `dwrote` | 0.11.5 | Text on Windows (through GPUI) | <https://crates.io/crates/dwrote> |
| `option-ext` | 0.2.0 | Through `dirs` | <https://crates.io/crates/option-ext> |

`cbindgen` (MPL-2.0) is used only as a build tool and is not part of the app.

Two libraries offer a choice of licence that includes a copyleft one: `r-efi` (MIT OR Apache-2.0 OR LGPL-2.1-or-later) and `self_cell` (Apache-2.0 OR GPL-2.0-only); this project uses them under MIT / Apache-2.0.

## Before a release

Permissive licences still ask for their copyright notices to ship with the app. Before publishing a release, generate the full list of every dependency's notice (for example with `cargo about`) and ship it with this file, and check this list against `cargo metadata` again, since dependencies change.
