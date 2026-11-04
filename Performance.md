# Performance Rules

Flux exists because file managers are slow. Speed is a feature and a regression is a bug. These rules keep it that way.

## Principles

1. **Measure, never guess.** Every claim about speed comes from the benchmarks below, run on a release build, before and after the change, on the same machine, back to back.
2. **The UI thread only paints and reacts.** Anything that can block (disk, network, shell, COM, icons, thumbnails, hashing, modules) runs on a worker and reports back through `Event`.
3. **Pay for what is visible.** Work scales with what is on screen, not with folder size: icons, thumbnails, previews and metadata are requested only for visible rows, and requests for rows that scrolled away are dropped at the end of the frame (`Media::EndFrame`).
4. **Idle means zero.** When nothing changes, Flux paints nothing, runs no timers and polls nothing. Animation and smooth-scroll timers run only while something is moving. File changes arrive through the watcher, not polling.
5. **No avoidable allocation in hot paths.** Listings store names in one shared UTF-16 buffer with fixed-size entries; sorting and filtering work on `u32` index arrays; per-frame code reuses buffers. Never clone a listing, a path list or a string per row per frame.
6. **Bounded everything.** Queues, caches and histories have explicit limits (media queue, thumbnail cache budget, recent list, undo stack). Unbounded growth is a bug.
7. **Scale to the hardware.** Large sorts run in parallel across cores (`Sorting::SortOrder`); enumeration streams partial results so the first rows appear before the folder finishes loading.
8. **Background work stays in the background.** Recursive folder sizes are computed by at most 4 threads in Windows background priority mode (lower CPU and I/O priority), nearest folders first, and never on the UI thread. Results are cached on disk so a folder is only fully walked once; later visits show the cached value instantly and re-verify quietly.

## Benchmarks

Build first: `cargo build --release -p flux`. Create the large test folder once:

```
target\release\Flux.exe --generate %TEMP%\FluxBench200k --count 200000
```

Headless engine benchmark (enumeration, memory per entry, sort per key, filter, recursive search):

```
target\release\Flux.exe --benchmark C:\Windows\System32 --iterations 5 --query dll --output System32.txt
target\release\Flux.exe --benchmark %TEMP%\FluxBench200k --iterations 5 --query beta --output Large.txt
```

UI benchmark (startup to first frame, listing painted, navigation, 240 scroll frames, idle CPU and idle frames, memory):

```
target\release\Flux.exe --bench-ui C:\Windows\System32 --output UiSystem32.txt
target\release\Flux.exe --bench-ui %TEMP%\FluxBench200k --output UiLarge.txt
```

The UI benchmark never touches the user's saved session.

## Baseline

Release build, 16 logical processors, Windows 10 19045, measured on a machine under normal desktop load. Absolute numbers vary with load; compare before/after runs made back to back.

| Measure | System32 (4,976 entries) | 200,000 entries |
|---|---|---|
| Enumerate, median | 3.1–5.2 ms | 53–98 ms |
| Listing memory | 79–133 bytes/entry | 91 bytes/entry |
| Sort by name, median | 1.0 ms | 11–14 ms |
| Sort by type, median | 1.6–1.9 ms | 25 ms |
| Filter, median | 0.15–0.20 ms | 3.5–3.8 ms |
| Recursive search | 18,520 entries in 280–400 ms | 200,000 entries in 155–290 ms |
| Scroll frame, p50 / p95 | 3.0–5.4 / 7.9–17.9 ms | 2.7–5.1 / 4.4–9.6 ms |
| Idle CPU after scrolling | 0.0–0.8 % of one core | 2–4 % while icon results drain, then 0 |
| First frame after process start | 210–290 ms | 240–370 ms |
| Working set | 53 MB | 92 MB |

Folder sizes: on a Desktop folder of 690 subfolders holding about 850,000 directories, the first full walk ran in the background while the window stayed responsive, the cache file was 53 MB, and private memory with the full directory tree loaded was about 200 MB. After a restart, sizes appeared within 3 seconds from the cache, and a file removed while Flux was closed was corrected by background verification within seconds.

## Budgets

A change may not make any of these worse. Changes that improve them update the baseline table above in the same change.

| Budget | Limit |
|---|---|
| Scroll frame p95 on the 200,000-entry folder | under 16.7 ms (60 fps) |
| Idle CPU, 4 s after the last input, nothing loading | under 0.5 % of one core |
| Frames painted while idle | only the 1 Hz performance overlay tick, if enabled |
| Sort of 200,000 entries by name | under 20 ms |
| Filter of 200,000 entries | under 10 ms |
| Listing memory | under 100 bytes per entry for typical names |
| Opening a folder from another app when Flux is running | handed to the running window, no new process startup |
| Startup to first frame | under 400 ms (target: under 150 ms) |

## Review checklist for performance-sensitive changes

- Does anything new run on the UI thread that can touch the disk, the network, the shell or COM?
- Does any per-frame or per-row code allocate, clone paths, or format strings that could be cached?
- Is every new queue or cache bounded, and are stale requests dropped?
- Does any new timer or animation stop when the motion ends?
- Were both benchmarks run before and after, and are the numbers in the change description?
