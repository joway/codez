# CodeZ

A fast, native macOS desktop app for browsing/editing code, reviewing git diffs,
and driving coding agents (Codex / Claude Code) from one window. Built with
`egui`/`eframe` on the `wgpu` backend (→ Metal on macOS), rendered Zed-style.

> The Cargo `description` still says "git diff viewer" — that's historical; the
> app now has three modes (Editor / Diff / Agent).

## Build & run

```sh
cargo run                 # opens the current directory
cargo run -- <path>       # open a folder, or a file (opens its parent + the file)
cargo build --release
```

- Rust, edition 2021. macOS-focused (native menu bar, dark titlebar, keychain).
- `cargo test` exists only for ad-hoc checks; there is no real test suite.
- After editing, always `cargo build`. The app is a long-running GUI process —
  source edits require a restart (`cargo run`) to take effect.

## Three modes

The top bar has a segmented switch (`Agent | Editor | Diff`); the active mode is
`app::Mode`. Native menu also switches: ⌘1 Agent, ⌘2 Editor, ⌘3 Diff.

- **Editor** — file tree + a custom code editor (multi-cursor, syntax highlight,
  find/replace, IME). Find-in-files and a ⌘P/⌘⇧P palette overlay live here too.
- **Diff** — git history & working-tree changes via libgit2, per-file patches,
  staging checkboxes, commit (multiline message), and Push.
- **Agent** — left: Codex/Claude Code sessions for the open folder + usage
  readouts; right: tabbed interactive terminals (resume a session or run a shell).

## Module map (`src/`)

| Module | Responsibility |
|---|---|
| `main.rs` | eframe bootstrap, dark window chrome, startup path resolution. |
| `app.rs` | The `eframe::App`. Owns all state, the mode bar, each mode's UI, and most view helpers. **Largest file — start here.** |
| `editor.rs` | Custom rope-backed code editor: carets/selection, key handling, IME, rendering, find bar. |
| `gitmodel.rs` | Thin libgit2 wrappers: workdir/commit changes, patches, `current_branch`, `commit_paths`, `push_origin` (shells out to `git`). |
| `terminal.rs` | PTY-backed interactive terminal pane. `alacritty_terminal` does PTY + VT parsing + grid; this draws it with egui and maps input. |
| `agent.rs` | Discover Codex (`~/.codex/sessions`) and Claude Code (`~/.claude/projects`) sessions for a cwd; build resume commands. |
| `usage.rs` | Codex/Claude usage % (session + weekly). Codex: parse newest session's `rate_limits`. Claude: OAuth token from keychain → `curl` the `/usage` endpoint. |
| `search.rs` | Find-in-Files: threaded streaming search, clickable results. |
| `palette.rs` | ⌘P file quick-open, `:N` go-to-line, ⌘⇧P command palette (VS Code-style). |
| `fstree.rs` | File-tree sidebar + right-click actions (new/rename/delete/copy/paste). |
| `settings.rs` | Settings window: fonts (with CJK fallback), font size, shortcuts. |
| `menu.rs` | Native macOS menu bar (`muda`). |
| `highlight.rs` | Lightweight syntax span highlighter. |
| `textview.rs` | Read-only virtualized diff renderer. |
| `theme.rs` | GitHub-dark palette + `Visuals`, plus shared widgets `icon_button` / `pill_button`. |

## Conventions & patterns

- **egui is pinned to 0.29** (eframe/egui/egui-winit). Widgets must use this
  app's egui version; do not pull crates that bundle a different egui.
- **`alacritty_terminal` pinned to 0.25.1** — `terminal.rs` is written against
  that exact API (`tty`, `event_loop`, `term::test::TermSize`, etc.).
- **Custom painter rendering is the house style.** Tabs, the mode switch, rows,
  bars, the help icon, and the terminal grid are drawn with `ui.painter()` +
  `ui.interact()`/`allocate_exact_size`, not stock widgets. Match this when
  adding UI; reuse `theme::icon_button` / `theme::pill_button` for buttons.
- **Colors** come only from `theme::*` (TEXT, TEXT_DIM, TEXT_MUTED, ACCENT,
  GREEN/YELLOW/RED, SURFACE/INSET/SIDEBAR/RAISED, BORDER, …).
- **Glyphs / fonts**: the bundled font lacks many symbols (✕ U+2715, ⬆ U+2B06
  render as tofu boxes). Use `×` (U+00D7), `↑` (U+2191), `‹`/`›`, etc. that the
  font has. CJK fallback is installed by `settings` on first frame.
- **Background work uses a thread + `mpsc` channel + `ctx.request_repaint()`**;
  the result is polled each frame and stored on `self`. Used by: `git push`
  (`push_rx`), usage fetch (`usage_rx`, 60s), agent session rescan (2s). Never
  block the UI thread on IO/network.
- **Panels pinned to an edge** use `TopBottomPanel::bottom(..).show_inside(ui,..)`
  so the neighbouring area auto-fills (commit box in Diff; usage in Agent).
  Avoid hardcoded reserved heights.

## Subsystem notes

- **Terminal** (`terminal.rs`): one `Terminal` per tab (`app::TermTab`). Features:
  keyboard incl. ctrl/arrows/app-cursor mode, IME (CJK), bracketed paste, mouse
  wheel scrollback, drag/double/triple-click selection, ⌘C copy, ⌘-hover/⌘-click
  to open URLs. New sessions run in a login shell (`$SHELL -l -i`) so the child
  inherits the user's PATH; an optional first command is typed in.
- **Agent sessions** (`agent.rs`): Codex sessions filtered by `cwd` in the
  `session_meta` line; Claude Code located via the encoded project dir
  (`/` and `.` → `-`). The Agent list rescans every 2s so new sessions appear.
- **Usage** (`usage.rs`): degrades gracefully — if a tool isn't installed the
  readout is `None` and that row/panel is hidden. No new HTTP deps; uses `curl`
  and `security` (macOS keychain) via `std::process::Command`.
- **Graceful absence**: the app must work with neither Codex nor Claude Code
  installed (empty session lists, hidden usage, shell shows "command not found").

## Gotchas

- Don't bump egui/eframe casually — it ripples through every painter call.
- `git push` shells out to the system `git` on purpose (reuses the user's
  credentials); don't reimplement auth via libgit2.
- An unborn branch (fresh repo, no commits) is normal: `list_commits` returns
  empty rather than erroring.
