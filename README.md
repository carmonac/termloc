# termloc

termloc is a privacy-focused terminal that runs entirely on your machine. There is no account, no
login, no telemetry and no cloud service. AI features run on local models through
[Ollama](https://ollama.com), and network access is limited by an egress policy.

It ships as a GUI app (`termloc`) and a console front-end (`termloc-tui`).

## Attribution

termloc is an independent fork derived from [Warp](https://github.com/warpdotdev/warp) under the
AGPL v3 and MIT licences. It is not affiliated with, endorsed by or sponsored by Warp or Denver
Technologies, Inc. "Warp" is a mark of its owner and is used here only to describe the origin of
this code.

## Requirements

- A Rust toolchain (see `rust-toolchain.toml`)
- [Ollama](https://ollama.com) with at least one model installed, for AI features

## Build and run

```bash
./script/bootstrap   # platform-specific setup
./script/run         # build and run the GUI app
./script/run-tui     # build and run the console front-end
cargo bundle --bin termloc
```

Settings and user data live in `~/.termloc` on macOS, and in the standard XDG or AppData locations
for `termloc` on Linux and Windows.

To allow the app to reach hosts other than this machine, set `TERMLOC_EGRESS_ALLOW` to a
comma-separated list of hosts.

See [AGENTS.md](AGENTS.md) for the engineering guide.

## Licence

The `warpui_core` and `warpui` crates are licensed under the [MIT license](LICENSE-MIT).

The rest of the code in this repository is licensed under the [AGPL v3](LICENSE-AGPL).

Third-party components keep their own licences. See `about.toml` and the notices shipped with the
source, such as `crates/warp_terminal/src/model/LICENSE-ALACRITTY`.
