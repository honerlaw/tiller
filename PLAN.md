# Tiller: Terminal-First IDE Plan

## Context

Forking Lapce (Apache 2.0) to build Tiller — a terminal-first IDE for supervising agentic CLI workflows (Claude Code, Aider, Codex, etc.). The terminal becomes the primary surface; the editor/file tree/diff viewer become inspection tools. Key additions over vanilla Lapce:

- Terminal as the center/primary panel (not a bottom drawer)
- Multi-tab terminal where each tab is bound to a git worktree
- Always-visible diff auto-refreshed against the active tab's worktree
- `cd` into a git repo registers it as a project root

Repo already cloned at `./tiller` with `upstream` → lapce/lapce and `origin` → honerlaw/tiller.

---

## Directory Map (Relevant Modules)

| Crate | Path | Role |
|---|---|---|
| lapce-rpc | `lapce-rpc/` | IPC message/data types shared between app and proxy. No UI. |
| lapce-core | `lapce-core/` | Pure text/syntax logic (tree-sitter, rope, highlight). No UI. Used by both app and proxy. |
| lapce-proxy | `lapce-proxy/` | Backend service: LSP, git2, search, WASM plugin runtime (wasmtime), terminal (alacritty_terminal), file watcher, file I/O. Has `src/` subdirs for `plugin/`. Can run remotely. |
| lapce-app | `lapce-app/` | Full GUI via Floem. 44+ src files + subdirs: `panel/`, `editor/`, `terminal/`, `proxy/`, `palette/`, `config/`. |

**Key lapce-app modules:**

| File | Role |
|---|---|
| `panel/kind.rs` | `PanelKind` enum (11 variants); `default_position()` per kind |
| `panel/position.rs` | `PanelPosition` (6 slots) and `PanelContainerPosition` (Left/Bottom/Right) |
| `panel/data.rs` | `PanelOrder` type; `default_panel_order()`; `PanelData`, `PanelSize` |
| `panel/view.rs` | Renders Left/Bottom/Right containers; handles resize drag |
| `panel/terminal_view.rs` | Terminal panel tab header UI; focus-on-click |
| `panel/plugin_view.rs` | Plugin marketplace panel UI ← **strip target** |
| `terminal/panel.rs` | `TerminalPanelData`; `new_tab()`, `focus_terminal()` |
| `terminal/tab.rs` | `TerminalTabData`; multi-terminal-per-tab |
| `terminal/data.rs` | `TerminalData`; alacritty raw terminal handle |
| `main_split.rs` | Main editor split/layout |
| `window_tab.rs` | `WindowTabData::new()` (lines 284–630); `Focus` enum; startup focus set at line 341 |
| `workspace.rs` | `LapceWorkspace`, `LapceWorkspaceType` (Local / RemoteSSH / RemoteWSL) |
| `proxy.rs` | `new_proxy()` — branches on workspace type |
| `proxy/remote.rs` | Remote SSH/WSL generic trait ← **strip target** |
| `proxy/ssh.rs` | SSH remote impl ← **strip target** |
| `proxy/wsl.rs` | WSL remote impl ← **strip target** |
| `plugin.rs` | Plugin marketplace data model, volts fetch/install ← **strip target** |
| `source_control.rs` | Git UI data model (105 lines) |
| `panel/source_control_view.rs` | Source control panel UI |
| `editor/diff.rs` | Diff viewer/renderer (450+ lines); `DiffEditorData` |
| `editor/gutter.rs` | Gutter diff indicators |
| `update.rs` | Software updater (already behind `#[cfg(feature = "updater")]`) |
| `palette.rs` | Palette commands; SSH/WSL connect at lines 806, 863, 1209, 1221, 1339 |

**lapce-proxy git backend:** `dispatch.rs` lines 1383–1632 (git2 ops: init, commit, checkout, discard, diff, remote-url).

**lapce-rpc source control types:** `lapce-rpc/src/source_control.rs`.

---

## Strip List

### 1. Software updater (Very Low Risk)
- **What**: `update.rs` + `app.rs` usage at lines 3816, 3961
- **Gating**: Already behind `#[cfg(feature = "updater")]`
- **Action**: Remove `updater` from `[features] default` in `lapce-app/Cargo.toml`
- **Dependencies/Risk**: None. Feature-flag removal only; code stays but dead.

### 2. Plugin Marketplace UI (Low Risk)
- **What**: The install/registry/browse UI — NOT the WASM runtime in lapce-proxy (keep that; other things may depend on it)
- **Files to delete**: `lapce-app/src/plugin.rs`, `lapce-app/src/panel/plugin_view.rs`
- **Cascade touches**:
  - `panel/kind.rs:14` — remove `Plugin` variant from `PanelKind`
  - `panel/data.rs:29` — remove `PanelKind::Plugin` from `default_panel_order()`
  - `panel/kind.rs:56` — remove `Plugin` arm from `default_position()`
  - `panel/kind.rs:30` / `svg_name()` — remove Plugin arm
  - `window_tab.rs` — remove plugin data initialization and any plugin-related signal setup
  - `lapce-rpc/src/proxy.rs` — remove RPC method stubs: `install_volt`, `enable_volt`, `disable_volt`, `reload_volt`, `remove_volt`
  - `lapce-proxy/src/dispatch.rs` — remove corresponding dispatch handlers
  - `settings.rs:443` — remove "Plugin Settings" entry
- **Risk**: Low. Plugin marketplace is a leaf in the dependency graph; the WASM runtime is in lapce-proxy and not touched.

### 3. Remote Dev / SSH / WSL (Low-Medium Risk)
- **What**: Remote connection capability (run lapce-proxy over SSH or WSL)
- **Files to delete**: `lapce-app/src/proxy/remote.rs`, `proxy/ssh.rs`, `proxy/wsl.rs`
- **Cascade touches**:
  - `workspace.rs:66–70` — remove `RemoteSSH(SshHost)` and `RemoteWSL(WslHost)` variants from `LapceWorkspaceType`; remove `is_remote()`, `is_ssh()` helper methods that match on them
  - `proxy.rs` `new_proxy()` — remove the SSH/WSL branches, keep only Local path
  - `palette.rs:806,863,1209,1221,1339` — remove SSH/WSL connect commands
  - Any match arms on `LapceWorkspaceType` throughout codebase (workspace display formatting at lines 92–97)
- **Risk**: Medium — `LapceWorkspaceType` is referenced throughout; need a full `grep` sweep after deleting variants. All remaining code should be Local-only.

---

## Layout Change Surface (Terminal as Primary)

Goal: Terminal occupies the large center panel by default; editor is a secondary surface you invoke explicitly.

**Where the current layout is defined:**
- Terminal default slot: `panel/kind.rs:53` → `PanelPosition::BottomLeft`
- Panel order initialization: `panel/data.rs:23–51` `default_panel_order()` — Terminal is first entry in `BottomLeft` bucket, editor (`main_split`) is the unnamed center
- Panel container layout rendering: `panel/view.rs` — Left/Bottom/Right rendered as resizable flex containers around the center (`main_split`)
- Startup focus: `window_tab.rs:341` — `Focus::Workbench` (editor-focused); change to `Focus::Panel(PanelKind::Terminal)`
- Panel size defaults: `panel/data.rs:69–76` `PanelSize` struct — `bottom` field controls height of the Bottom container

**Minimal approach to make terminal primary (without a full layout rewrite):**
1. In `default_panel_order()`: move Terminal to `LeftTop` or a new `CenterBottom` — OR keep it in `BottomLeft` but maximize height
2. Change startup focus in `window_tab.rs:341` to `Focus::Panel(PanelKind::Terminal)`
3. Increase the default `bottom` panel height in `PanelSize` so the terminal gets most of the vertical space on first launch
4. Optionally: add a `toggle_maximize()` call on the terminal panel at startup to make it full-height by default

**Longer-term (post-strip) layout restructure:** Introduce a new `PanelPosition::Center` that displaces `main_split` as the primary surface. This is higher-risk and should follow after rebrand + strip commits are stable.

---

## Worktree Work Needed

### What exists
- `git2` is already a dependency in `lapce-proxy/Cargo.toml`
- Git operations in `lapce-proxy/src/dispatch.rs:1383–1632`: init, commit, checkout, discard, diff, remote-url
- Diff viewer in `lapce-app/src/editor/diff.rs` with reactive `listen_diff_changes()`
- `TerminalTabData` in `terminal/tab.rs:12–18` — has `active: RwSignal<usize>` and a vector of terminals

### What's missing / needs to be added

1. **Worktree RPC types** (`lapce-rpc/src/source_control.rs`):
   - `WorktreeInfo { path: PathBuf, branch: String, is_main: bool }`
   - RPC notifications: `ListWorktrees`, `CreateWorktree`, `RemoveWorktree`

2. **Worktree git operations** (`lapce-proxy/src/dispatch.rs`):
   - `git_list_worktrees()` — uses `Repository::worktrees()` + `Repository::find_worktree()`
   - `git_create_worktree(name, path, branch)` — uses `Repository::worktree()`
   - `git_remove_worktree(name)` — uses `Worktree::prune()`

3. **Terminal tab worktree binding** (`terminal/tab.rs`, `terminal/panel.rs`):
   - Add `worktree_path: Option<PathBuf>` field to `TerminalTabData`
   - When creating a new terminal tab, optionally accept a worktree path and set the terminal CWD accordingly
   - UI: show worktree branch name in tab header

4. **Auto-refreshing diff** (`source_control.rs`, `editor/diff.rs`):
   - Watch for active terminal tab change (reactive signal in `TerminalPanelData`)
   - When active tab changes, trigger `git_diff_new()` against that tab's worktree path
   - Source control panel auto-switches to show diffs for the active worktree

5. **Worktree management UI** (`panel/source_control_view.rs` or new panel):
   - List worktrees, create, remove, open in new terminal tab

---

## Suggested Commit Order (Lowest Risk First)

| # | Scope | Commit Message | Files Touched | Risk |
|---|---|---|---|---|
| 1 | rebrand | `rebrand: rename lapce → tiller` | Root `Cargo.toml`, `lapce-app/Cargo.toml`, `lapce-proxy/Cargo.toml`, `lapce-core/Cargo.toml`, `lapce-rpc/Cargo.toml`, binary name in `lapce-app/src/bin/lapce.rs`, app title string, config dir name in `directory.rs` | Low |
| 2 | verify | Build check after rebrand — `cargo build` | — | — |
| 3 | strip | `strip: remove updater feature` | `lapce-app/Cargo.toml` (remove `updater` from defaults) | Very Low |
| 4 | strip | `strip: remove plugin marketplace UI` | Delete `plugin.rs`, `panel/plugin_view.rs`; update `panel/kind.rs`, `panel/data.rs`, `window_tab.rs`, `lapce-rpc/src/proxy.rs`, `lapce-proxy/src/dispatch.rs`, `settings.rs` | Low |
| 5 | strip | `strip: remove remote dev (SSH/WSL)` | Delete `proxy/remote.rs`, `proxy/ssh.rs`, `proxy/wsl.rs`; update `workspace.rs`, `proxy.rs`, `palette.rs` | Medium |
| 6 | layout | `layout: make terminal the primary panel` | `panel/kind.rs`, `panel/data.rs`, `window_tab.rs` (startup focus) | Medium |
| 7 | feat | `feat: add worktree RPC types` | `lapce-rpc/src/source_control.rs` | Low |
| 8 | feat | `feat: add worktree git operations to proxy` | `lapce-proxy/src/dispatch.rs` | Medium |
| 9 | feat | `feat: bind terminal tabs to git worktrees` | `terminal/tab.rs`, `terminal/panel.rs`, `panel/terminal_view.rs` | Medium |
| 10 | feat | `feat: auto-refresh diff for active terminal tab` | `source_control.rs`, `editor/diff.rs`, `window_tab.rs` | Medium-High |

---

## Verification

After each strip commit: `cargo build` (no `--release` needed; just verify it compiles).

After layout commit: run the app (`cargo run`) and confirm terminal opens maximized/focused on startup.

After worktree commits: manual test — create two worktrees, open two terminal tabs bound to each, switch tabs and confirm diff panel updates.
