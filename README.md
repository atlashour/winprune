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
| medium | any machine | consumer apps, sponsored apps, telemetry, advertising id, suggestions, typing and speech data, Copilot, Recall and the AI in Settings, Notepad and Paint, a few idle services |
| high   | machines without Microsoft cloud ties | OneDrive uninstall, Xbox app, Copilot app, new Outlook, Phone Link, Media Player, Get Help, Sticky Notes, Spotlight, location, error reporting, app access to the Windows AI models, compatibility appraiser, fresh telemetry device id |
| max    | kiosks, signage, appliances | Windows Update, search indexer, Widgets, the Recommended section in Start, Xbox identity and Gaming Services, the OneDrive folder itself |

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
winprune diff --level high    what of level high is still left to do here
winprune apply --level high   apply after a confirmation and a UAC prompt
winprune apply --level medium --dry-run --yes
                              walk the real code path without touching anything
winprune catalog list         every item with its level and risk
```

`--skip a.b,c.d` leaves items out, `--add x.y` pulls items in on top of the level,
`--only x.y,z.w` ignores the level entirely. `plan --json` prints the plan for
tooling.

`diff` reads the machine the same way `plan` does and sorts every item of the level
into pending (applying would still change something), applied, absent (nothing it
acts on is here, removed earlier or never shipped), blocked (nothing left to apply,
but parts Windows keeps or winprune does not remove remain) and not for this build.

Without elevation the provisioned packages and the Default profile cannot be read.
Items that depend on them, or on anything else that could not be read, show as
`applied?` when nothing readable is pending, or `unknown` otherwise. `diff --all`
lists what could not be read, and `diff --exit-code` exits with 5 when something is
pending; items that could not be read do not count. `diff --json` gives the same for
tooling, with `status` one of `pending`, `unconfirmed` (shown as `applied?`),
`unverified` (shown as `unknown`), `applied`, `blocked`, `absent` and
`not_applicable`.

The terminal UI shows the same status next to each item and the counts under the
list; `p` lists only the selected items that are pending.

Exit codes: 0 done, 1 bad arguments or setup error, 2 some operations failed, 3
elevation needed, 4 aborted at a prompt, 5 `diff --exit-code` found pending items.

Each apply writes a JSON report and a log under `%LocalAppData%\winprune` of the
account that runs it.

Plan and dry-run never need administrator rights. Applying does: winprune writes the
run into a folder under `%LocalAppData%\winprune\runs`, asks through UAC, continues in
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

There is no undo. Removed packages come back only through the Store or a reinstall.
Policies and services can be put back by hand, but the report only says what was
changed, not what was there before. `winprune plan` saves a `plan-*.json` next to the
log where each registry and service operation records what it found (a registry value
with nothing recorded was not set), so run it right before applying if you may want
to go back.

It does not touch the Edge browser, Defender, the Store app itself, drivers or
anything performance related.

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

An item that others list in `requires` cannot be disabled on its own: the overlay
fails to load until those are disabled too. To check an overlay before using it:

```
winprune catalog validate --catalog my.toml
```

Step kinds: `appx`, `service`, `registry`, `task`, `kill`, `run`, `delete`,
`unsupported`. See the files under `catalog/` for the shape of each.

Registry and delete steps under a user profile take `scope = "user" | "all-users" |
"default-profile"`; the Default profile is left out for keys under
`Software\Classes`, whose hive winprune does not mount. A registry value is a `dword`
from 0 to 4294967295 or a quoted `string`; anything else fails validation.

A `run` step can list in `when_registered` the registry values the software it
removes keeps while installed, read for the account the command runs as. Once none is
present, the step counts as done and is not run again. An `unsupported` step names
something winprune leaves in place and reports it while its values are present.

## Build

```
cargo build --release
```

Rust 1.88 or newer, MSVC toolchain. The C runtime is linked statically
(`.cargo/config.toml`), so the executable runs on a fresh Windows install without the
Visual C++ redistributable. Tests run with `cargo test` and do not need administrator
rights.
