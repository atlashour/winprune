# winprune

Removes the parts of Windows 10 and 11 that nobody asked for: preinstalled apps,
telemetry, advertising, Copilot, Recall, Xbox, OneDrive. One executable, a terminal
UI to pick what goes, and a plain CLI for unattended runs.

No PowerShell, no DISM, no scripts downloaded at run time. Packages are removed
through the same Windows API the Settings app uses, services through the service
control manager, policies through the registry.

## Levels

Every item in the catalog has a level. Pick one and uncheck whatever you want to keep.

| level  | meant for | adds |
|--------|-----------|------|
| medium | any machine | consumer apps, sponsored apps, telemetry, advertising id, suggestions, typing and speech data, Copilot and Recall policies, a few idle services |
| high   | machines without Microsoft cloud ties | OneDrive uninstall, Xbox app, Copilot app, new Outlook, Phone Link, Get Help, Sticky Notes, location, compatibility appraiser, fresh telemetry device id |
| max    | kiosks, signage, appliances | Windows Update, search indexer, Widgets, Xbox identity provider, the OneDrive folder itself |

Items that can cost you something carry a warning, and the plan fills it with what it
finds on the machine (how many files sit in the OneDrive folder, for instance).
High-risk items make you type `yes`.

Where Microsoft honours a policy only on Enterprise and Education, the item says so.
It is still written on Pro and Home; it just does nothing there, and the per-user
switches that do work on those editions sit next to it.

## Use

```
winprune                      terminal UI, starts at level medium
winprune --level high         terminal UI, starts at level high
winprune plan --level max     print what would happen, change nothing
winprune apply --level high   apply after a confirmation and a UAC prompt
winprune apply --level medium --dry-run --yes
                              walk the real code path without touching anything
winprune catalog list         every item with its level and risk
```

`--skip a.b,c.d` leaves items out, `--add x.y` pulls items in on top of the level,
`--only x.y,z.w` ignores the level entirely. `plan --json` prints the plan for
tooling. Each apply writes a JSON report and a log under `%ProgramData%\winprune`
(or `%LocalAppData%\winprune` when not elevated).

Plan and dry-run never need administrator rights. Applying does: winprune writes the
run into a folder under `%ProgramData%\winprune\runs`, asks through UAC, continues in
a new window, and prints the outcome back in the window you started from.

Double-clicking `winprune.exe` asks through UAC first and opens the terminal UI
already elevated, so a whole run happens in one window. Refuse the prompt and the UI
opens without elevation instead. A window that winprune opened for itself stays until
you press Enter.

## Per-user settings

Many switches live in the user's own registry hive. winprune writes them for the
account that started it (even when the UAC prompt was answered with another
administrator account), for every other profile that is loaded at the time, and for
the Default profile, so accounts created later start clean. Profiles that are not
loaded are left alone.

## What it does not do

There is no undo. Removed packages come back only through the Store or a reinstall;
policies and services can be reverted by hand from the report. Read the plan before
applying it.

It does not touch Edge, Defender, the Store, drivers or anything performance related.

## Extending the catalog

The catalog is a set of TOML files compiled into the binary (`catalog/`). Nothing
Windows-specific lives in Rust, so a renamed package or a new Windows release is a
data change. To try changes without rebuilding, pass an overlay:

```
winprune plan --catalog my.toml
```

An overlay can add items or patch existing ones by id:

```toml
[[item]]
id = "appx.media-player"
enabled = false            # keep Media Player

[[item]]
id = "appx.company-tool"
name = "Old company tool"
category = "appx"
level = "medium"
risk = "low"
summary = "Removes the previous vendor's agent."
[[item.step]]
kind = "appx"
patterns = ["*OldVendor*"]
```

Step kinds: `appx`, `service`, `registry`, `task`, `kill`, `run`, `delete`. Registry
and delete steps under a user profile take `scope = "user" | "all-users" |
"default-profile"`. See the files under `catalog/` for the shape of each.
`winprune catalog validate --catalog my.toml` checks an overlay.

## Build

```
cargo build --release
```

Rust 1.88 or newer, MSVC toolchain. The C runtime is linked statically
(`.cargo/config.toml`), so the executable runs on a fresh Windows install without the
Visual C++ redistributable. Tests run with `cargo test` and do not need administrator
rights.
