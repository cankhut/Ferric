# Ferric

A desktop app for rating the albums you listen to. Look one up, rate them out of five stars.

It's my first complete app I've made using Rust. 
It's just a hobby project so I doubt it will recieve many updates. 
It was mainly done to get better and learn Rust as I've mentioned.


## Getting it

You can either get it from [Releases](../../releases)

Or build it yourself with [Rust](https://rustup.rs):

```
cargo run --release
```

## Your files

Just two JSON files, in `%APPDATA%\ferric\` on Windows and `~/.local/share/ferric/`
elsewhere. `library.json` is your albums and ratings and `settings.json` is the
window size, the sort order and your Discogs token. 


## For better genres

Genres work with no setup since it uses from MusicBrainz and Deezer. Discogs' are
usually better as I've seen so far styles like "metalcore, 
post-hardcore" rather than just "rock".

Although it requires a free token:
1. Open <https://www.discogs.com/settings/developers> and sign in.
2. Press **Generate token**.
3. Paste it into the cog, then press **Look up every album's tags again**.

Album covers and searches come from **Deezer** and **iTunes**.

## Building

```
cargo build --release   # target/release/ferric.exe is what you ship
cargo test
cargo clippy
```

## Licence

MIT. See [LICENSE](LICENSE).
