# Flux Core Rules

These rules outrank every feature request, refactor, and deadline. A change that violates one of them is a bug, no matter how good the rest of it is. Detailed rules live in [Compatibility.md](Compatibility.md), [CodeQuality.md](CodeQuality.md) and [Performance.md](Performance.md).

## 1. Never break the user

The Flux equivalent of "never break user space". If something worked for a user yesterday, it works for them today.

- **Every published contract stays valid.** Settings keys, session files, keybinding names, theme files, command identifiers, command-line flags, the module ABI and the shell registration are contracts. See [Compatibility.md](Compatibility.md) for the full list. Add to them; never rename, repurpose or remove.
- **A regression is a bug, even if the old behavior was "wrong".** If users depend on it, it stays until a migration exists that carries them across without them noticing.
- **Upgrades are invisible.** A new version reads every file an older version wrote. Unknown keys are preserved, missing keys get defaults, corrupt files fall back to defaults without crashing and without overwriting the user's original.
- **Everything Flux changes outside itself is reversible.** Registry keys, file associations and shell hooks are backed up before they are touched and restored exactly on uninstall (`--unregister-shell` or the Settings toggle).

## 2. Never lose user data

- File operations are recoverable: delete goes to the Recycle Bin unless the user explicitly asks for permanent deletion, and every rename, move, copy and delete is recorded for undo.
- A conflict is never resolved by silently overwriting. Ask, or pick a unique name.
- Cancelled or failed operations leave the file system in a state the user can understand: completed items stay completed, the item in flight is rolled back, nothing is half written under the final name.
- Flux never writes into user folders except as the direct result of a user action.
- Crash reports never contain file contents unless the user enabled full memory dumps.

## 3. The UI thread never waits

- No disk, network, shell, COM, hashing, enumeration or module call runs on the UI thread. Work goes to a worker and comes back as an `Event`.
- Modal OS dialogs run through `Deferred`, outside the `App` borrow.
- Every long operation is cancellable and reports progress.
- A slow network share, a dead drive or a misbehaving module can make *its own* view slow. It must never freeze the window.

## 4. Fast is a feature, and it does not regress

- Every change that touches enumeration, sorting, filtering, search, painting or startup is benchmarked before and after with the commands in [Performance.md](Performance.md).
- A measurable slowdown, memory increase or idle CPU increase is a regression and blocks the change, unless it buys something the user asked for and the cost is written down.
- Idle means idle: no timers, no repaints and no polling when nothing changes.

## 5. Modules cannot hurt the core

- Modules are opt-in, approved by the user, and verified by SHA-256 before every load. A changed DLL needs re-approval.
- The core never depends on a module. Removing every module leaves a fully working file manager.
- Module calls run off the UI thread, are cancellable, and a failing module produces an error message, not a crash.
- The module ABI is append-only. See [Compatibility.md](Compatibility.md).

## 6. Crashes are reported, not hidden

- The out-of-process crash reporter stays on by default. Every crash produces a minidump and a readable report with stack, registers and loaded modules.
- Errors are shown to the user with the real reason and, where possible, the fix. Swallowing an error is a bug.
- No `unwrap()`/`expect()` on anything that depends on the outside world (files, registry, COM, user input, module output).

## 7. Real features only

No placeholders, mock data, fake progress, disabled buttons that pretend to work, or "coming soon" entries. If it is in the UI, it works end to end. If it cannot work on this system, it is hidden or explains why.

## 8. Small core, few dependencies

- The only external crates are `windows`, `windows-core` and `windows-numerics`. Adding a dependency requires a written reason why the Win32 API cannot do the job.
- Features that are not core file management belong in a module.
- Delete dead code in the same change that makes it dead. No compatibility shims inside the codebase; compatibility is for users, not for old internal call sites.

## 9. Verify before claiming

A change is done when it has been run: the build has zero warnings, the changed path was exercised in the real application, and the relevant benchmark was re-run. "It compiles" is not verification.
