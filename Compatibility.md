# Compatibility Contracts

Everything on this page is something a user, a script or a module may depend on. These are the "user space" of Flux. The rules for all of them:

1. **Additive only.** New keys, flags, commands and fields may be added. Existing ones are never renamed, removed, or given a different meaning.
2. **Old input keeps working.** Files written by any earlier release load without errors and without losing information.
3. **Unknown input is preserved, not discarded.** Flux rewrites only the values it owns.
4. **Defaults never change behavior silently.** A new default applies to new installs; existing users keep what they had unless they opt in.
5. **Deprecation is a migration, not a deletion.** If something truly must change, the new version reads the old form, writes the new form, and the change is listed in the release notes.

## Data locations

| Data | Location | Portable mode |
|---|---|---|
| Settings, session, keybindings, module trust, themes, menu usage (`MenuUsage.ini`) | `%APPDATA%\Flux\` | `Data\` next to `Flux.exe` when `Flux.portable` exists |
| Installed modules, crash reports, search index, folder size cache (`Cache\FolderSizes.bin`), backups | `%LOCALAPPDATA%\Flux\` (bundled modules: `Modules\` next to `Flux.exe`) | `Data\` next to `Flux.exe` |
| Installed copy (from `Package.ps1 -Install`) | `%LOCALAPPDATA%\Programs\Flux\` | n/a |

These paths never move. A new location requires reading from the old one.

## Settings.ini, Session.ini, Keybindings.ini, ModuleTrust.ini, themes

- INI format, UTF-8, `Key=Value`, section and key names case-insensitive.
- Saving Settings.ini loads the existing file first and updates only the keys Flux owns, so keys from newer versions or written by hand survive.
- Values that fail to parse fall back to the default for that key only; the rest of the file still applies.
- Boolean values are `true`/`false`. Numbers are plain decimal. Paths are absolute Windows paths.
- Command identifiers used in Keybindings.ini (`View.ToggleAnimations`, `App.OpenSettings`, ...) are permanent. A command that is replaced keeps its identifier as an alias that runs the replacement.
- Theme files keep every color and size key that has ever been read. Missing keys inherit from the built-in theme.
- Settings keys added since the first release: `ReuseWindow`, `FolderSizes`, `ShellActionsInline` (all booleans, default `true`).
- `MenuUsage.ini` section `[Usage]` maps a lower-case app action label to `count,last-used-unix-seconds`. It only ranks menu entries; deleting it resets the ranking and nothing else.

## Folder size cache

- `Cache\FolderSizes.bin` is a pure cache: deleting it costs one rescan and never loses user data.
- The file starts with an 8-byte magic (`FLUXSZ01`). A file with a different magic is ignored and rebuilt, never misread. A new format gets a new magic.
- Every directory record stores its own direct file bytes, file count, last-write time and a content hash of its direct entries (name, size, modified time). Cached values are shown immediately; every folder a user views is re-verified in the background and only corrected where the hash differs. While Flux runs, a recursive change watcher on each fixed drive keeps cached folders current.

## Command line

| Flag | Meaning |
|---|---|
| `Flux.exe <folder>` | open the folder (in the running window when "Open folders in existing window" is on) |
| `Flux.exe <file>` | open the containing folder with the file selected |
| `Flux.exe /select,<path>` | Explorer-compatible form of the above |
| `--new-window` | always start a new process and window |
| `--register-shell` / `--unregister-shell` | become / stop being the default file manager (backed up and restored exactly) |
| `--benchmark <dir> [--iterations N] [--query Q] [--output F]` | headless enumeration, sort, filter and search benchmark |
| `--bench-ui <dir> [--output F]` | UI benchmark: startup, listing, scroll frames, idle CPU, memory |
| `--generate <dir> [--count N]` | create a synthetic benchmark directory |

Flags are never removed and their output formats only gain lines; existing lines keep their wording so scripts that parse them keep working. Exit codes: `0` success, `1` failure, `2` invalid arguments.

## Shell integration

`--register-shell` writes only these per-user keys under `HKCU\Software\Classes`:

- `Directory\shell` default verb, `Directory\shell\open\command`
- `Drive\shell` default verb, `Drive\shell\open\command`
- `CLSID\{52205fd8-5dfb-447d-801a-d0b52f2e83e1}\shell\opennewwindow\command` (Win+E and the taskbar File Explorer entry)
- `Directory\Background\shell\Flux` ("Open in Flux")

Every value that existed before is saved under `HKCU\Software\Flux\ShellBackup` before it is changed, and `--unregister-shell` restores exactly that state. Flux never writes to `HKLM`, never modifies system files, and never needs administrator rights for integration. Any new integration point must follow the same backup-and-restore rule.

The running-instance handoff uses `WM_COPYDATA` with `dwData = 0x464C5558` and a UTF-16 payload (folder path, `/select,<path>`, a `::{CLSID}` location, or empty for the home folder). The payload format is append-only.

## Module ABI

Defined in `Sdk/Source/lib.rs`.

- `AbiVersion` changes only when an existing structure changes layout. The host refuses modules with a different version; it never loads them in a degraded mode.
- Within one `AbiVersion`, structures are append-only: new fields go at the end and every structure carries its own `Size`, so older modules and newer hosts (and the reverse) keep working.
- Exported symbol `FluxModuleInitialize`, status codes, and `Module.ini` keys (`Id`, `Name`, `Version`, `AbiVersion`, `Library`, `Description`, `Author`, `[Command.*]` with `Title`/`Prompt`/`Requires`, `[Preview]` `Extensions`) are permanent.
- A module may never be required for core functionality, and the host must keep working when a module DLL is missing, corrupt, or returns an error.
- Approval is bound to the SHA-256 of the DLL. Changing that rule requires re-approval of every module, never silent trust.

## Crash reports

Report and dump file names (`Flux-<date>-<time>-<pid>.txt/.dmp`) and the section headings inside reports stay stable so tooling can parse them.
