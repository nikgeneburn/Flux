<p align="center"><img src="logo.png" width="112" alt="Flux logo"></p>

<h1 align="center">Flux</h1>

<p align="center">A fast, keyboard-first file manager for Windows, written in Rust on raw Win32 and Direct2D.</p>

---

Flux is built to replace File Explorer as your everyday file manager. Folders open in milliseconds, the window never freezes on a slow drive, and every menu, panel and dialog is drawn by Flux itself. It is a single 2 MB executable with no runtime, no web view and three dependencies, all of them Microsoft's Windows bindings.

## Features

**Browsing**
- Tabs (drag to reorder, reopen closed, duplicate) and an optional dual-pane layout
- Breadcrumb address bar with folder drop-downs, path autocomplete and history; drop files on any segment to move them there
- Details, List, Grid and Thumbnails views with sortable, resizable and configurable columns
- True folder sizes: the Size column shows everything inside each folder, computed in the background, cached on disk, verified with per-folder content hashes and kept current by a live change watcher
- Sidebar with collapsible Favorites, Recent and folder tree, plus drives with a blue-to-red usage bar
- Inspector panel with previews of images, text and binary files, folder sizes and quick actions

**Finding things**
- Instant filter as you type, recursive search of subfolders with streamed results
- Command palette (Ctrl+K) for every command, and fuzzy "go to folder" (Ctrl+P)

**File operations**
- Copy, move, rename, new folder and new file, all undoable (Ctrl+Z / Ctrl+Y)
- Delete to the Recycle Bin by default; conflicts are never resolved by silent overwrite
- Cut, copy and paste with Explorer and other apps, and drag and drop in both directions
- Copy to or move to the other pane (F7 / F8)

**Right-click anywhere**
- Custom context menus for files, folder backgrounds, tabs, columns, breadcrumbs and sidebar items
- Copy the current folder path, or open it in Windows Terminal, PowerShell 7, Windows PowerShell or Command Prompt (whichever are installed)
- App actions from installed shell extensions (7-Zip, Git, Visual Studio, ShareX, Send to and others) are pulled into Flux's own menus with their icons and submenus, most-used first; show them directly or grouped under one entry in Settings
- Windows' full shell menu is one click away under "Show more options"

**Make it yours**
- Settings panel for every option, also stored as plain `Settings.ini`
- Every shortcut can be rebound in `Keybindings.ini`
- Dark, Light and System themes, custom themes as `.ini` files, three densities
- Optional animations and smooth scrolling

**Replaces File Explorer**
- One switch makes Flux open folders, drives and Win+E, including from other apps
- Folders opened from elsewhere become a new tab in the running window instead of a new process
- Shares the File Explorer taskbar button, so it never adds a slot to a full taskbar
- Fully reversible: the original registry values are backed up and restored exactly

**Modules**
- Optional DLL plugins with a versioned C ABI, user approval and SHA-256 integrity checks
- Bundled: Developer Tools (hashes, Git status, hex preview), Duplicate Finder, Folder Compare, Batch Rename and Folder Index

**Reliability**
- Out-of-process crash reporter writes a minidump and a readable report with stack traces, registers, arguments and loaded modules
- Built-in performance overlay (F12) and command-line benchmarks

## Install

Requirements: Windows 10 or 11 (x64) and the [Rust toolchain](https://rustup.rs). The toolchain version is pinned in `rust-toolchain.toml`.

```powershell
git clone <this repository>
cd Flux
.\Package.ps1 -Install
```

This builds a release, installs it to `%LOCALAPPDATA%\Programs\Flux`, and registers Flux as the default file manager. To only build a package in `Distribution\Flux` without installing, run `.\Package.ps1`. Add `-Portable` to keep settings next to the executable.

To give folders back to File Explorer, turn off **Settings → Windows integration → Default file manager**, or run:

```powershell
& "$env:LOCALAPPDATA\Programs\Flux\Flux.exe" --unregister-shell
```

## Keyboard

| Action | Shortcut |
|---|---|
| Command palette | Ctrl+K, Ctrl+Shift+P |
| Go to folder | Ctrl+P, Ctrl+G |
| Filter or search | Ctrl+F, F3 |
| Edit address | Ctrl+L, Alt+D |
| Back, forward, up | Alt+Left, Alt+Right, Alt+Up or Backspace |
| New tab, close tab, reopen tab | Ctrl+T, Ctrl+W, Ctrl+Shift+T |
| Dual pane, switch pane | F9, Tab |
| Copy or move to other pane | F7, F8 |
| Views: Details, List, Grid, Thumbnails | Ctrl+1 to Ctrl+4 |
| Rename, new folder | F2, Ctrl+Shift+N |
| Recycle, delete permanently | Delete, Shift+Delete |
| Undo, redo | Ctrl+Z, Ctrl+Y |
| Copy path | Ctrl+Shift+C |
| Open terminal here | Ctrl+` |
| Toggle hidden items, sidebar, inspector | Ctrl+H, Ctrl+B, Ctrl+I |
| Settings | Ctrl+, |
| Performance overlay | F12 |

All bindings live in `%APPDATA%\Flux\Keybindings.ini`. Reload with Ctrl+Shift+R.

## Command line

```
Flux.exe [folder | file | /select,<path>]     open a folder, or a file's folder with the file selected
Flux.exe --new-window                         start a separate window
Flux.exe --register-shell | --unregister-shell
Flux.exe --benchmark <dir> [--iterations N] [--query Q] [--output F]
Flux.exe --bench-ui <dir> [--output F]
Flux.exe --generate <dir> [--count N]
```

## Performance

Release build, measured on a busy desktop (16 threads). Full methodology and budgets are in [Performance.md](Performance.md).

| | `C:\Windows\System32` (4,976 items) | Synthetic folder (200,000 items) |
|---|---|---|
| Enumerate | 3–5 ms | 53–98 ms |
| Sort by name | 1 ms | 11–14 ms |
| Filter | 0.2 ms | 3.5 ms |
| Scroll frame, median | 3–5 ms | 3–5 ms |
| Idle CPU | ~0 % | ~0 % once icons finish loading |
| Memory (working set) | 53 MB | 92 MB |

## Writing a module

A module is a DLL plus a `Module.ini` manifest in its own folder under `Modules\`. Modules are built with the `FluxSdk` crate in `Sdk/`:

```rust
use FluxSdk::{ExportModule, Host, Invocation, Module, Outcome};

struct Hello;

impl Module for Hello {
    fn Invoke(&self, Host: &Host, Request: &Invocation) -> Outcome {
        Host.Line(&format!("{} item(s) selected", Request.Selection.len()));
        Outcome::Completed
    }
}

static Instance: Hello = Hello;
ExportModule!(Instance);
```

See the five modules in `Modules/` for complete examples, and [Compatibility.md](Compatibility.md) for the manifest format and ABI stability rules. Flux loads a module only after you approve it, and asks again if the DLL changes.

## Project layout

```
Core/      the Flux executable
  Source/Platform      Win32 wrappers: windows, shell, clipboard, drag and drop, imaging, integration
  Source/FileSystem    enumeration, sorting, search, watching, file operations
  Source/Services      settings, workers, crash reporting, modules, performance
  Source/Interface     drawing: chrome, file views, overlays, menus, themes
  Source/Control       input, commands, panes, sessions, undo
Sdk/       the module SDK (FluxSdk crate)
Modules/   bundled modules
```

## Rules

Flux has a short set of non-negotiable rules. Read them before contributing:

- [Rules.md](Rules.md): the core rules. Never break the user, never lose data, never freeze the UI, never get slower.
- [Compatibility.md](Compatibility.md): every contract users and modules depend on.
- [CodeQuality.md](CodeQuality.md): structure, naming and correctness rules.
- [Performance.md](Performance.md): benchmarks, baselines and budgets.

## Configuration and data

| | Location |
|---|---|
| Settings, session, shortcuts, themes, module trust, menu usage | `%APPDATA%\Flux\` |
| Installed modules, crash reports, indexes, folder size cache, backups | `%LOCALAPPDATA%\Flux\` |
| Portable mode | `Data\` next to `Flux.exe` when a `Flux.portable` file exists |
