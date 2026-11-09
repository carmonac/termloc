# termloc

termloc is a privacy-focused terminal that runs entirely on your machine. It is a fork of
[Warp](https://github.com/warpdotdev/warp) with every server connection removed: there is no
account, no login, no telemetry and no cloud service. AI features run on local models through
[Ollama](https://ollama.com), and an egress policy blocks outbound network traffic by default.

It ships as a GUI app (`termloc`) and a console front-end (`termloc-tui`).

## How it differs from Warp

termloc keeps Warp's terminal, editor, blocks, workflows, notebooks and agent interface, and
changes what sits behind them.

| | Warp | termloc |
|---|---|---|
| Account | Sign-in required for most features | None. A local user is created on the device |
| AI | Runs on Warp's servers with hosted models | Runs on your machine through Ollama |
| Agent | Prompts and tool definitions live on the server | A local agent engine (`crates/local_agent`) with the same tools |
| Warp Drive | Synced to the cloud, shareable with teams | Stored on the device in SQLite, never synced |
| Codebase search | Embeddings computed server-side | Embeddings computed by an Ollama model and stored locally |
| Telemetry and crash reports | Sent to Warp | Removed |
| Network | Talks to Warp services | Outbound traffic blocked except to this machine and hosts you allow |
| Languages | English | English, Spanish, Portuguese, French, German, Italian, Ukrainian, Japanese and Simplified Chinese |

### What was removed

- Sign-in, accounts, teams, referrals, billing, credits and usage limits
- Telemetry, crash reporting and analytics
- Cloud sync and sharing for Warp Drive objects, sessions and blocks
- The cloud agent platform (Oz), ambient agents and the cloud agent SDK
- Hosted model providers and API-key configuration: models come only from Ollama
- The SSH remote server, GraphQL and REST clients, and server URL overrides
- Autoupdate and the changelog feed
- Background calls to `gh` and links to Warp's documentation and services

### What was added

- **Local agent.** The agent runs a full turn on an Ollama model with tool calling: shell commands,
  reading, searching, creating and editing files, reading long-running command output, asking
  you questions, and MCP servers you configure. Every command it proposes is marked as risky, so
  it waits for your approval unless your execution profile allows it.
- **Single-request AI on Ollama.** Natural-language commands, autosuggestions, conversation
  titles, summaries, workflow metadata, and commit message and pull request text.
- **Local models settings page.** Set the Ollama URL, context window, a model for quick tasks and
  an optional model for code search. The model picker lists installed models that support tools
  and shows the details Ollama reports for each one.
- **Local code search.** With an embeddings model selected, repositories are indexed on the
  device, and the agent can search them.
- **Egress policy.** Every HTTP client is built through a policy that only allows loopback
  addresses, the configured Ollama host and hosts listed in `TERMLOC_EGRESS_ALLOW`.
- **Several interface languages.** Menus, settings, the command palette, the terminal and
  workspace, the agent interface and the TUI are translated.

## Languages

The interface is available in English, Spanish, Portuguese, French, German, Italian, Ukrainian,
Japanese and Simplified Chinese. By default it follows your system language and falls back to
English when that language isn't one of these. To pick a language, use **Interface language**
under **Settings → Appearance**, or set `appearance.language` in the settings file. The change applies
after restarting termloc.

## Requirements

- A Rust toolchain (see `rust-toolchain.toml`)
- [Ollama](https://ollama.com) with at least one model installed, for AI features. The agent needs
  a model with tool calling, such as `qwen3:8b`.

## Build and run

```bash
./script/bootstrap   # platform-specific setup
./script/run         # build and run the GUI app
./script/run-tui     # build and run the console front-end
cargo bundle --bin termloc
```

Settings and user data live in `~/.termloc` on macOS, and in the standard XDG or AppData locations
for `termloc` on Linux and Windows.

termloc connects to Ollama at `http://127.0.0.1:11434` by default; change it on the Local models
settings page. To allow the app to reach other hosts, set `TERMLOC_EGRESS_ALLOW` to a
comma-separated list of hosts. The policy covers the app's own connections, not programs you run
in the terminal.

See [AGENTS.md](AGENTS.md) for the engineering guide.

## Attribution

termloc is an independent fork derived from [Warp](https://github.com/warpdotdev/warp) under the
AGPL v3 and MIT licences. It is not affiliated with, endorsed by or sponsored by Warp or Denver
Technologies, Inc. "Warp" is a mark of its owner and is used here only to describe the origin of
this code.

## Licence

The `warpui_core` and `warpui` crates are licensed under the [MIT license](LICENSE-MIT).

The rest of the code in this repository is licensed under the [AGPL v3](LICENSE-AGPL).

Third-party components keep their own licences. See `about.toml` and the notices shipped with the
source, such as `crates/warp_terminal/src/model/LICENSE-ALACRITTY`.
