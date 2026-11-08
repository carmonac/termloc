---
name: tui-verify-change
description: Verify a change to Warp's headless TUI front-end (crates/warp_tui) by running it via ./script/run-tui and reading the rendered screen back (under tmux when it's installed, otherwise directly). Use whenever you change TUI UI, rendering, input, or behavior and need to confirm the real on-screen result.
---

# tui-verify-change

Verify a change to Warp's **headless TUI** front-end (`crates/warp_tui`, and the
cell-grid element library in `crates/warpui_core/src/elements/tui`) by running it
and reading back the actual rendered screen.

The whole point: **the TUI is a console program, so you can run it in a real
terminal and read the actual rendered screen straight back.** When `tmux` is
available it's the ideal driver — you run the TUI in a tmux pane, drive it with
`tmux send-keys`, and read the frame back with `tmux capture-pane` — but tmux is
**not required**: if it isn't installed you can still run and observe the TUI
directly (see **If tmux isn't installed** under Step 2). Either way you (the
agent) see the actual screen text — no `computer_use`, no real display, no cloud
screenshot agent, and no relying on a separate watcher's description of what it
saw. (Contrast the GUI-only `gui-onboarding-verification-skill`,
`gui-integration-test`, `gui-integration-test-video`, and the `computer_use` /
`verify-ui-change-in-cloud` flow.) This is the fast, preferred way to confirm a
TUI change end-to-end.

This skill covers the **manual live-run** verification. For a durable regression
that runs in CI, also add a render-to-lines unit test per `tui-testing` — the two
are complementary: the live run confirms real behavior; the snapshot test locks
it in.

## When to use

- You changed anything a TUI user sees or interacts with: an element/layout in
  `crates/warpui_core/src/elements/tui`, a TUI view/screen in `crates/warp_tui`
  (transcript, input, zero state, etc.), TUI keybindings, or
  TUI rendering/behavior.
- You want to confirm the *actual* rendered result, not just that it compiles or
  a unit test passes.

If your change is GUI-only (`app/`, WarpUI pixel `Element`/`View`), this is the
wrong skill — use the GUI verification path instead.

## Signed-in state

termloc has no login: the TUI starts signed in as the built-in local user and goes
straight to `Starting terminal…` and then the **zero state**. Run
**`./script/run-tui`** (or the `termloc-tui` binary) the same way locally and in a
headless runner.

## Step 1 — Build the TUI

Always build with a small job count so the large `warp` dependency tree doesn't
OOM the machine:

```bash
cd <warp-repo-root>
CARGO_BUILD_JOBS=2 cargo build -p warp_tui --bin termloc-tui
```

- `termloc-tui` is the OSS channel binary and the safest default (no internal
  `warp-channel-config` generator required). `./script/run-tui` does the
  equivalent, selecting the internal `local` binary when the generator is
  available and falling back to `termloc-tui` otherwise.
- Fix all compile errors before running (see `fix-errors`). The first build of
  this tree takes a while; **incremental rebuilds after a one-line change are
  fast (~10s)**, so the edit → rebuild → re-capture loop below is quick.

## Step 2 — Run under tmux and read the frame back

The TUI needs a **real interactive PTY** — a tmux pane is exactly that, and it
lets you both send input and read the rendered screen back as text. Start the TUI
in a detached session with an explicit size (don't skip `-x`/`-y`; a degenerate
1-row pane renders nothing useful):

```bash
cd <warp-repo-root>
tmux kill-session -t tuicheck 2>/dev/null   # clear any prior run
tmux new-session -d -s tuicheck -x 120 -y 40 './target/debug/termloc-tui'
sleep 1                                       # let it draw + probe the terminal
tmux capture-pane -t tuicheck -p              # <-- the rendered screen, as text
```

`tmux capture-pane -p` prints the pane's current contents (the TUI's alternate
screen) to stdout, so **you read the real frame directly** and assert on it. Add
`-e` to include ANSI escape sequences when you need to check colors/styles:

```bash
tmux capture-pane -t tuicheck -p -e          # includes color/style escapes
```

Drive interactions with `tmux send-keys`, sleeping to let the UI settle, then
capture again. For example (type a line, submit it, wait, then read the screen):

```bash
tmux send-keys -t tuicheck "What is 2+2? Answer in one short sentence." && \
  sleep 1 && tmux send-keys -t tuicheck Enter && sleep 5 && \
  tmux capture-pane -t tuicheck -p -e
```

Send special keys by name (`Enter`, `Escape`, `C-c` for Ctrl-C, `Up`/`Down`).
When done, tear the session down: `tmux kill-session -t tuicheck`.

### If tmux isn't installed

`tmux` is the preferred driver because it gives you programmatic `send-keys` +
`capture-pane`, but it is **not** a hard requirement — never block verification
just because tmux is missing. Check with `command -v tmux`; if it's absent, fall
back:

- **Local context:** the simplest path is to run `./script/run-tui` directly in a
  real terminal and read the rendered output yourself — you already have a PTY.
  When you're working alongside the user, you can also have them run it and report
  what renders. Installing tmux is optional, not a prerequisite.
- **Cloud / no-tmux context:** run the built binary inside another PTY wrapper so
  you can still capture output — e.g. `script` (util-linux):
  `script -qe -c './target/debug/termloc-tui' /tmp/tui.log`, then read
  `/tmp/tui.log`. If tmux is installable in your environment
  (`apt-get install -y tmux`) and that's cheaper, do that and use the flow above
  instead. If none of these work, run the binary directly, capture whatever
  output you can, and **say so** in the PR/thread rather than implying a
  tmux-driven capture.

Everything else in this skill (what to look for, the snapshot test, the evidence)
is identical whether or not tmux drove the run.

### Iterate loop

Because incremental rebuilds are ~10s, iterate tightly: edit the TUI code →
`cargo build -p warp_tui --bin termloc-tui` → `tmux kill-session` + restart the
session → `capture-pane` and compare.

## Step 3 — Check the captured frame against your change

You have the real screen text, so verify it yourself: grep/scan the
`capture-pane` output for the **exact string or layout** your change should
produce, and diff the before/after captures. No watcher interpretation needed —
if the expected text isn't in the capture, the change isn't rendering.

## Pitfalls (learned hands-on)

- **Run it in a real terminal / tmux pane** (a PTY), and drive + capture it via
  tmux as above.
- **If it exits (code 101) right after the first frame instead of staying up:**
  don't assume it's a terminal/stdin problem — **check the TUI log first**:
  `tail -40 ~/.local/state/warp-terminal-tui/warp-cli/warp-tui.log`. One cause seen in
  the headless sandbox build is a debug-only binding-validation
  panic: `crates/warpui_core/src/keymap/matcher.rs` (`validate_bindings`, gated on
  `#[cfg(debug_assertions)]`) panics with `Bindings failed validation` when a
  *keystroke* binding matches a TUI keymap context without being TUI-owned (it was
  `app:reopen_closed_session`, Ctrl+Alt+T). It does **not** reproduce in every
  setup — it depends on which keystroke bindings the running config loads, and the
  validator exempts non-keystroke (palette/custom) triggers — so treat this as one
  thing to check, not a guarantee. If you hit it, two ways to still verify a
  change:
  - **Build `--release`** — the validator is compiled out, so the TUI stays up
    and you can `send-keys`/`capture-pane` freely:
    `cargo build --release -p warp_tui --bin termloc-tui` then run
    `./target/release/termloc-tui`.
  - **Or poll `capture-pane` right after launch** on the debug build to grab the
    first frame before the panic:

    ```bash
    tmux new-session -d -s tuicheck -x 120 -y 40 './target/debug/termloc-tui'
    for i in $(seq 1 15); do
      frame=$(tmux capture-pane -t tuicheck -p | sed 's/[[:space:]]*$//' | grep .)
      [ -n "$frame" ] && { echo "$frame"; break; }
      sleep 0.2
    done
    tmux kill-session -t tuicheck 2>/dev/null
    ```

  If the debug build stays up on your machine, you can `send-keys`/`capture-pane`
  repeatedly without racing.
- **Alt screen is handled for you.** `capture-pane` reads the alternate screen,
  so you don't need to fight escape-sequence soup the way piping stdout would.
- **Startup probe.** On launch the TUI emits terminal probes (OSC `10`/`11` +
  a device-attributes query) to pick a theme; a normal tmux pane answers them.
  Give it ~0.5–1s (a `sleep`) before the first capture.
- **Quitting.** Ctrl-C is `tmux send-keys -t tuicheck C-c`. In a live session it's
  press-again-to-exit (first press cancels/clears input and arms a ~1s window; a
  second within the window exits). Always `tmux kill-session` at the end so a stray session doesn't linger.

## Step 4 — Capture screenshots and video (asciinema + agg)

`capture-pane` text is the fast inner-loop check (Step 3) and enough to *assert*
on a change. But for PR evidence — and for attaching durable image/video
artifacts — you often want an actual **screenshot** or a short **video** of the
rendered TUI. Because the TUI is a console program, capture it by recording its
PTY session with **asciinema**, rendering that recording with **agg**, and
transcoding it to an **MP4** (the same format Warp's `computer_use` screen
recording produces) with `ffmpeg`; pull a still frame out with `ffmpeg` too.

**Install the tooling (cloud runner — one-time).** asciinema and ffmpeg are
packaged; `agg` ships as a prebuilt binary rather than in apt:

```bash
sudo apt-get update && sudo apt-get install -y asciinema ffmpeg tmux
# agg is not in apt — install a PINNED release binary and verify its checksum before
# installing as root (don't pull an unpinned `latest`). The checksum below is for the
# x86_64 build; on another arch use that asset's published checksum from the release.
AGG_VERSION=v1.9.0
AGG_SHA256=f111e315cd71056b116302342553dd765b7297579ed511f111d0cedb442aeda6
curl -fsSL -o /tmp/agg \
  "https://github.com/asciinema/agg/releases/download/${AGG_VERSION}/agg-$(uname -m)-unknown-linux-gnu"
echo "${AGG_SHA256}  /tmp/agg" | sha256sum -c -   # aborts on mismatch
sudo install -m 0755 /tmp/agg /usr/local/bin/agg
```

**Record the session.** asciinema needs a real PTY, so run it **inside tmux** (a
bare `asciinema rec` in a non-interactive runner shell fails with "not a
terminal"). Drive the TUI with `tmux send-keys` exactly as in Step 2 — the keys
reach the binary running under asciinema:

```bash
cd <warp-repo-root>
tmux kill-session -t tuicap 2>/dev/null     # clear only THIS capture session (not kill-server)
# asciinema records the TUI's PTY; -c runs the binary; --overwrite replaces a prior cast.
tmux new-session -d -s tuicap -x 120 -y 40 \
  'asciinema rec --overwrite -c "./target/debug/termloc-tui" /tmp/tui.cast'
sleep 1                                     # let it draw + answer the theme probe
# ...drive the interaction you want to show, e.g.:
# tmux send-keys -t tuicap "hello" Enter && sleep 3
# Quit the TUI so asciinema finalizes the cast. A live session needs a SECOND Ctrl-C
# within its ~1s window (see "Quitting" under Pitfalls) — so send two; the extra press
# is a harmless no-op if it already exited (the session is gone, hence 2>/dev/null).
tmux send-keys -t tuicap C-c && sleep 0.5 && tmux send-keys -t tuicap C-c 2>/dev/null
sleep 1
```

**Render the video (MP4).** Match the format Warp's `computer_use` screen
recording uses — an **H.264 / yuv420p MP4** with `+faststart` (see
`crates/computer_use/src/linux/recording.rs`) — so TUI captures are consistent
with GUI/computer-use recordings. `agg` only emits GIF, so render to GIF and then
transcode to MP4 with those settings. libx264 + yuv420p require **even**
dimensions, so pad up by a pixel when the terminal render is odd-sized:

```bash
agg --cols 120 --rows 40 /tmp/tui.cast /tmp/tui.gif
ffmpeg -y -i /tmp/tui.gif -vf "pad=ceil(iw/2)*2:ceil(ih/2)*2" \
  -c:v libx264 -pix_fmt yuv420p -movflags +faststart /tmp/tui.mp4
# /tmp/tui.gif is just the intermediate; /tmp/tui.mp4 is the artifact you keep.
```

**Pull a still (PNG)** from a frame *while the surface is on screen* — see the
frame-timing pitfall below:

```bash
ffmpeg -y -ss 1.5 -i /tmp/tui.mp4 -frames:v 1 /tmp/tui.png
```

**Attach the capture as conversation artifacts (required, not an afterthought).**
Once you have the still and/or the recording, attach *each* to the run as a
**conversation artifact** so the proof persists beyond `/tmp`, travels with the
task, and can surface into the PR description — don't leave
it sitting in a temp file. Call the `upload_artifact` tool once per file, passing
the local `file_path` and a short `description` (e.g. `file_path=/tmp/tui.png`,
`description="TUI <surface> after <change> — verification screenshot"`, and
likewise `/tmp/tui.mp4` for the recording). For any user-visible TUI change you
verified here, attaching the screenshot and any recording is **expected**. These
are FILE artifacts capped at 25 MB each, so keep recordings short (see below). If
you're running somewhere the `upload_artifact` tool isn't available (a plain
local dev shell rather than a cloud/ambient agent), keep the files and reference
them in the PR instead.

Capture pitfalls:
- **asciinema must run under a PTY.** Wrap it in tmux (above) or `script`; a
  bare `asciinema rec` in a non-interactive runner shell errors out.
- **Don't take the still from the first or last video frame.** The first frame
  is the blank terminal *before* the TUI draws, and once you quit the TUI the alt
  screen is restored — so the final frames show the normal terminal, **not** the
  TUI surface.
  Extract a mid-recording timestamp (when the surface is up), or stop recording
  while the surface is still displayed so the last frame *is* the surface.
- **Keep it short.** A few seconds at 120x40 renders to tens of KB; downstream
  sinks (Slack, and conversation FILE artifacts) cap uploads at 25 MB, so don't
  record minutes of idle.

Keep `capture-pane` text as the fast inner loop; reach for asciinema+agg when you
need the image/video to attach.

## Step 5 — Lock it in with a snapshot test

A live run proves the change works now; it is not a regression guard. For any
non-trivial TUI rendering/behavior change, add or update a render-to-lines unit
test (`warpui_core::elements::tui::test_support::render_to_lines` /
`TuiBuffer::to_lines`) per `tui-testing`, and run:

```bash
cargo nextest run -p warp_tui
cargo nextest run -p warpui_core
```

## Evidence for the PR

For a user-visible TUI change, **prefer a screenshot or short video as the
primary evidence** — an actual image/clip of the rendered surface is always more
convincing to a reviewer than raw text. Capture it per Step 4 (a still, or an
H.264 MP4 matching computer-use recordings), attach it to the run as a
conversation artifact, and reference it in the PR. Include the `tmux
capture-pane` lines (and/or a `render_to_lines` snapshot diff) as a
**supplement** — handy for asserting on exact text — not as the main proof. Only
fall back to text alone when an image/clip genuinely can't be produced, and say
so. This is the TUI equivalent of the GUI's `computer_use` screenshot (see the
TUI caveat in `review-pr-local`).

## Related skills

`tui-ui-guidelines` (the `TuiElement` cell-grid library) and `tui-testing`
(render-to-lines unit tests) are companion TUI skills added alongside this one;
land them together. This skill's build/run/capture workflow stands on its own —
those cover authoring TUI UI and writing durable tests.

GUI-only counterparts (do **not** use for TUI work): `gui-integration-test`,
`gui-integration-test-video`, `gui-onboarding-verification-skill`.

## Improving this skill over time (self-improvement loop)

The aim is for this skill to get better **over time** — **not** for every run to
end in an edit. Most runs should need no change here; don't manufacture trivial
wording tweaks just to have improved something, and never let this step turn into
busywork.

Act only when a run surfaces a **genuine, notable gap** — a step that **didn't
work** as written, a command that failed, a path that moved, missing
local-vs-cloud or tmux handling, or an assumption that didn't hold. When that
happens, don't just work around it silently: capture the specific problem (what
you expected vs. what actually happened) and **propose the fix in a _separate_
PR** — separate from the change you were verifying, so the skill improvement is
reviewable on its own and the original PR stays scoped. Make the smallest correct
edit to `.agents/skills/tui-verify-change/SKILL.md` (follow the `update-skill`
conventions) that would have made the run go smoothly.
