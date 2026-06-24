# AGENTS.md — Animawave Agent Protocol

## What Is Animawave?
Animawave is an open-source GTK4/libadwaita internet radio player for 50,000+ stations from radio-browser.info.

**Agent Readiness:** This file contains two kinds of guidance:
- **Universal rules** — project structure, branching, architecture, and security patterns that apply regardless of what tooling you have.
- **Workflow recommendations** — tool-specific tips that are helpful when the relevant tools are available, but not required to complete tasks. Use whatever tools you have access to.

## Repository Structure
```
animawave/
├── build-aux/          # Build scripts, Flatpak manifests
├── data/               # App resources (icons, UI, DB schema)
├── debian/             # Packaging metadata
├── po/                 # i18n files
├── src/
│   ├── api/            # Radio-browser API client
│   ├── audio/          # GStreamer backend, recording
│   ├── database/       # SQLite storage
│   ├── device/         # Device casting (Chromecast)
│   ├── settings/       # Configuration
│   └── ui/             # GTK widgets and dialogs
├── Cargo.toml          # Rust dependencies
├── meson.build         # Build system
└── README.md           # Docs
```

## Build, Test, and Run
```bash
# Flatpak (recommended)
flatpak install gnome-nightly de.haeckerfelix.Animawave.Devel

# Manual build
meson --prefix=/usr build
ninja -C build

# Quality checks
./build-aux/checks.sh cargo_fmt   # Format
./build-aux/checks.sh cargo_clippy # Lint
./build-aux/checks.sh cargo_deny   # Licensing
./build-aux/checks.sh potfiles     # i18n
```

## Tooling Tips (Optional)

When available, structured analysis tools are more efficient than raw file reads:

- **Exploration**: Search for symbols, get file outlines, inspect call hierarchies
- **Impact analysis**: Check blast radius before making changes
- **Code discovery**: Use regex or text search for string literals
- **Build/test**: Use a terminal or shell at the project root

**Note**: Repeated raw file reads can be wasteful. Prefer targeted lookups when your toolset supports them.

## ❗ Agent SOP — The Delegate-Verify Loop

**Follow every time. Never skip verification.**

### Step 1: Analyze & Plan
1. Map blast radius before changing — search symbols, check references
2. Identify integration points (`api/`, `audio/`, `ui/` interactions)
3. Break tasks into minimal, verifiable increments

### Step 2: Delegate One Step
- Pass complete context — the delegate or sub-agent is stateless and needs full context about repo structure, target symbols, and expected outcome
- Never bundle multiple changes into a single delegation

### Step 3: ❗ VERIFY THE RESULT
Agents frequently claim success while silently failing to edit files.

**After every delegated task:**
1. **Read the actual modified file** — confirm the expected code is present (e.g. via `grep` or reading file contents)
2. Check git status — `git --no-pager diff` before committing
3. Validate — verify downstream impact matches expectations (e.g. check references or call sites)
4. **Register edits** — update any indexes or caches your tooling requires
5. Test — run expected functionality plus adjacent components

> ❗ Lesson learned — CoverLoader: A subagent edited `api/cover_loader.rs` to fix cache eviction logic, then reported success. No edits were actually applied — the agent claimed it had updated the code when it had only read it. Only `read_file` confirmed the file was unchanged. Always verify the real file — never trust a delegate's self-report.

---

## Git Rules

### Branch Model
- **Base branch**: `main` — all PRs target this
- **Naming**: `feat/short-desc`, `fix/short-desc`, `docs/short-desc`
- **Sync**: `git pull origin main` before any commit

### Commit Format
```
<type>(<scope>): <message>

<optional body>

<optional footer>
```

| Type    | Scope examples          | Use case                          |
|---------|--------------------------|-----------------------------------|
| `feat`  | `audio`, `ui`, `api`      | New features, behavior changes    |
| `fix`   | `gstreamer`, `cast`       | Bug fixes, resilience improvements|
| `docs`  | `README`, `metainfo`      | Documentation updates             |
| `ref`   | `cleanup`, `sqlite`       | Refactoring without behavior change|

## Testing Rules

### How Tests Work
Animawave relies on CI gates enforced via `build-aux/checks.sh`:
- Format: `cargo fmt --all -- --check`
- Lint: `cargo clippy --all -- -D warnings`
- Licensing: `cargo deny --log-level error check`

**Run locally**: Always invoke via `checks.sh` wrapper.

### Known Failure Modes
| Failure                | Detection               | Recovery strategy          |
|------------------------|--------------------------|----------------------------|
| Non-empty potfiles diff| `./build-aux/checks.sh potfiles` | Add missing `src/*.rs` entries |
| CI formatting mismatch | Flatpak runtime fmt      | Rebase on `origin/main`, rerun |
| IANA private range     | `cargo deny`             | Patch resolver or exclude   |

## Architecture Landmines

| Component               | Hotspot                                          | Why it's risky                  |
|------------------------|-------------------------------------------------|----------------------------------|
| GStreamer backend      | `src/audio/gstreamer_backend.rs`                | Thread-sensitive callback timing  |
| Flatpak permissions    | `build-aux/*.json`                              | Missing `--share=network`         |
| Device casting         | `src/device/cast_sender.rs`                     | GUPnP context thread lifetime     |
| Recording mode         | `src/audio/recording_mode.rs` + GstClockTime    | File handle leaks, storage limits |
| CoverLoader cache      | `api/cover_loader.rs` + glycin crate            | `lcms2` format coercion crashes    |

> ❗ Hotspot example: GStreamer callbacks in `gstreamer_backend.rs` must never block or access GTK directly — use `clone!(@weak obj => async move { obj.do_gui_updates(); })` for thread transitions. Cross-thread violations cause runtime segfaults.

## Credential Rules
This project has no deployment credentials.

**Safe import**: `.env.example` is checked in; `.env` is gitignored. For dev tokens (if ever needed), use:
```
cp .env.example .env
# Edit .env (containing placeholder values only)
```

## UI/Component Gotchas

### GTK/libadwaita Traps

1. **Widget ownership** — GTK widgets exist in a singleton hierarchy. Mutating widgets created in a different context requires:
   ```rust
   clone!(@weak widget => move |_| {
       widget.set_property("visible", true);
   });
   ```

2. **GObject subclasses** — Must implement `Default`, have `#[glib::object_subclass]` macro, and must NEVER contain `
` in method signatures.

3. **Builder patterns** — Use `build_ui` helpers for *.ui template files:
   ```rust
   fn build_ui(&self) {
       let builder = gtk::Builder::from_resource("/path/to/ui/template.ui");
       let widget: CustomWidget = builder.object("name").unwrap();
   }
   ```

### Memory Traps
- `GString` from `String`: `.to_string()` -> `.into()`
- Boxed GDK types: `gdk::Rectangle::from(...)`
- Do not store `gtk::glib::WeakRef` across rust async blocks