# Code Quality Rules

These rules keep Flux readable and reviewable as it grows. They are enforced on every change; a pull request that breaks one is not merged until it is fixed.

## Structure

- **At most 1,000 lines per source file.** When a file approaches the limit, split it by responsibility, not by line count. Never compress code onto long lines to stay under the limit.
- **At most 2 source files per directory and at most 5 subdirectories per directory.** A directory is a unit of responsibility; if it needs more, it needs a sub-unit.
- **Layering is one-way:**
  - `Platform` wraps Win32 and knows nothing about Flux concepts above it.
  - `FileSystem` is pure file logic: enumeration, sorting, search, watching, operations. No UI.
  - `Services` are long-lived subsystems: config, workers, diagnostics, modules, performance.
  - `Interface` draws and hit-tests. It reads state; it does not perform file operations.
  - `Control` turns input and commands into state changes.
  - `Application` owns the state and the message loop.
  Lower layers never call upward. Cross-layer communication goes through `Event`, `Command` and `Deferred`.
- One way to do each thing. Before adding a helper, search for an existing one (`WideString`, `ApiPath`, `ShellPath`, `UniqueName`, `Spawn`, `Pool`, `WriteAtomically`, ...) and use it.

## Naming and style

- **Every identifier is UpperCamelCase**: functions, methods, variables, fields, constants, statics, modules, closure parameters. The only exceptions are names fixed by Rust traits (`drop`, `fmt`, `main`) or external APIs. Crate roots carry `#![allow(non_snake_case, non_upper_case_globals)]`.
- UpperCamelCase bindings can collide with enum variants, unit structs and constants inside `match`/`if let` patterns, silently turning a comparison into a catch-all binding. Pick names that cannot collide, and import every constant you match on.
- **No comments.** Code explains itself through names and structure. If a block needs a comment, extract it into a well-named function instead.
- Names say what something is, not how it is stored: `SelectedCount`, not `Count2`; `Directory`, not `Path1`.
- Never mix formatting-only changes with behavior changes.

## Correctness

- **Zero warnings** in debug and release builds, for every crate in the workspace.
- No `unwrap()`/`expect()` on fallible operations that touch the outside world. Errors carry a message a user can act on and are shown through `Fail`/notices.
- Every Win32 resource is owned by an RAII wrapper or closed on every path (handles, find handles, PIDLs, menus, COM apartments, clipboard sessions).
- Verify Win32 signatures against the `windows` crate source before using an API; in 0.62 most `BOOL` functions return `Result<()>`.
- `unsafe` blocks are as small as possible and contain only the FFI call or the pointer access that needs them.
- Threads are created with `Services::Workers::Spawn` or a `Pool` so crash handling and naming apply. Shared state uses channels (`EventSink`) first, `Mutex`/atomics second.
- Cancellation is checked in every loop that can run longer than a frame.
- Re-entrancy: window messages can arrive while `App` is borrowed (modal loops, `DestroyWindow`, `SendMessage`). Anything that must happen regardless of the borrow is handled outside `WithApp`.

## Dependencies

- Allowed: `windows`, `windows-core`, `windows-numerics`, and the workspace `FluxSdk`. Nothing else without a written justification in the change description.
- Enable only the `windows` features that are used.

## No dead weight

- No placeholders, stubs, mock data, `TODO`, or features behind fake UI.
- Code made unused by a change is deleted in that change, including settings, commands and assets.
- No internal compatibility shims or re-exports for old call sites; migrate every caller. (User-facing compatibility is a different thing: see [Compatibility.md](Compatibility.md).)

## Testing and verification

- Every change is run, not just compiled: build, launch, exercise the changed path, observe the result.
- Bugs are reproduced before they are fixed and confirmed after.
- Performance-sensitive changes include before/after numbers from [Performance.md](Performance.md).
- UI changes are checked at 100% and at a higher DPI scale, in dark and light themes, with animations on and off.
- Throwaway scripts and test files are removed when the work is done.
