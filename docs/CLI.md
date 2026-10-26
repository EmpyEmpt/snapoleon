# Command line interface

Run `snapoleon --help` and `snapoleon <command> --help` for the authoritative option list (the text follows the
app language).

`snapoleon` with no arguments starts the tray app. The app is a single instance: if one is already running, a
second invocation forwards its arguments to it over local IPC and exits, so `snapoleon capture` from a script or a
compositor key binding opens the overlay of the running app. When no instance is running, the command runs
headless where that makes sense.

A bare first argument is understood without a subcommand, which is how desktop entries (`snapoleon %U`), file
associations and drag-and-drop call the app: `snapoleon snapoleon://capture_region` is `snapoleon url ...`, and
`snapoleon shot.png` (any existing path) is `snapoleon edit shot.png`.

On Windows the binary uses the GUI subsystem and attaches to the parent console when started from a terminal, so
output appears in `cmd` and PowerShell.

| Command | Purpose |
|---|---|
| `snapoleon capture` | Capture a region, window, monitor or full screen; optionally run a pipeline and print or copy the result. |
| `snapoleon record` | Start or stop a screen recording. |
| `snapoleon ocr <image>` | Recognize text in an image. |
| `snapoleon qr decode <image>` / `qr encode <text>` | Decode QR codes and barcodes, or draw a QR code. |
| `snapoleon upload <file>` | Upload a file to a configured destination and print the link. |
| `snapoleon pin <image>` | Show an image as an always-on-top pin. |
| `snapoleon edit <image>` | Open an image or `.snap` project in the editor. |
| `snapoleon history search [words]` | Search the capture library. |
| `snapoleon pipeline list` / `pipeline run <id> <image>` | List and run configured pipelines. |
| `snapoleon list monitors` / `list windows` | Show the monitor indexes, window ids and rectangles the capture options take. |
| `snapoleon doctor` (alias `permissions`) | Check the installation: version, paths, signature, the running app, privacy permissions. |
| `snapoleon install-cli [--dir DIR]` | Put a `snapoleon` command on PATH that runs this installation (macOS and Linux). |
| `snapoleon mcp` | Run the MCP server on stdio. |
| `snapoleon settings` | Open the Settings window. |
| `snapoleon quit` | Quit the running app (does nothing when it is not running). |
| `snapoleon url <snapoleon://...>` | Handle a `snapoleon://` URL (used by the OS URL scheme registration). |

## Coordinates

Every rectangle and position is in **physical pixels** of the virtual desktop, with the origin at the top-left of the
primary monitor; monitors to its left or above have negative coordinates. `snapoleon list monitors` prints each
monitor's rectangle and scale factor. On macOS, where other tools use points, divide by the monitor's scale factor
(2 on a Retina display) to convert.

```sh
snapoleon list monitors                    # 0  Built-in Retina Display  0,0,3024,1964  2x  primary
snapoleon list windows                     # 4182  Safari  Snapoleon - GitHub  0,74,3024,1890
snapoleon capture --region 0,0,1200,800 --out part.png
snapoleon capture --window Safari --out page.png    # title, then app name, case-insensitive
```

## Output and exit codes

Results go to standard output, progress and diagnostics to standard error, so a script can capture one without the
other. Errors are one line starting with `Error:` on standard error.

| Exit code | Meaning |
|---|---|
| 0 | Success. |
| 1 | The command failed (nothing to capture, no permission, upload error, no code found, ...). |
| 2 | Bad arguments (unknown option, malformed region, no target given). |

Without `--json` a command prints the most useful single thing: the link if there is one, else the saved file, else
the text. These take `--json` for the full result:

| Command | JSON |
|---|---|
| `capture ... --json` | `id`, `file`, `url`, `short_url`, `delete_url`, `text`, `codes`, `width`, `height`, `screen_rect` |
| `ocr <image> --json` | `text`, `engine`, `lines` with words and boxes in image pixels |
| `qr decode <image> --json` | `format`, `text` and the box of each code |
| `history search --json` | the library entries |
| `pipeline list --json` / `pipeline run ... --json` | the pipelines / the outcome, as for `capture` |
| `list monitors --json` / `list windows --json` | the same objects as the MCP `list_monitors` / `list_windows` |
| `doctor --json` | `version`, `os`, `exe`, `bundle`, `signature`, `host_app`, `on_path`, `paths`, `permissions`, `app` |

`--out -` writes the image to standard output (`capture`, `qr encode`), and `ocr -` / `qr decode -` read the image
from standard input.

## macOS

**Putting `snapoleon` on PATH.** Run `/Applications/Snapoleon.app/Contents/MacOS/snapoleon install-cli`, or use
Settings > Advanced > Command line tool. It writes `/usr/local/bin/snapoleon` (asking for an administrator password
when that folder needs one; `--dir ~/.local/bin` avoids that), a two-line script that `exec`s the binary inside the
app. Do not symlink the executable instead: macOS identifies an app by the path it was started from, and a symlinked
binary runs without its bundle (no bundle id, no Info.plist, privacy grants that never apply). The Homebrew cask
installs the same kind of script.

**Privacy permissions.** macOS checks Screen Recording, Accessibility and the rest against the app that *started* a
process. A headless command run from Terminal, iTerm, an editor or an agent host (Claude, Codex) is therefore checked
as that app, not as Snapoleon, and Snapoleon's own grant does not apply to it. Snapoleon handles this for you:

- A headless capture (`capture --region|--window|--monitor|--all|--element`, and the MCP capture tools) that may not
  read the screen asks the running app to take the screenshot instead and reads it back from a temporary file. When
  the app is not running and the command runs from inside `Snapoleon.app` (directly or through the PATH script), it
  starts the app in the background first. Only the app then needs Screen Recording.
- When neither is possible (a bare `cargo build` binary with no app running), the error names the app that needs the
  permission, for example `Terminal`.
- `record` with a target cannot be handed over, so it needs Screen Recording for the app that started it.
- A GUI start from a terminal (`snapoleon`, `snapoleon capture` without a target, `snapoleon capture --pipeline ...`)
  relaunches the app through LaunchServices and forwards the request, so the app runs under its own identity and its
  answer still prints in the terminal.
- `list windows` without Screen Recording shows empty titles (macOS hides them); it says so on standard error.
  `capture --window <title>` still works, because the app matches the title itself.

`snapoleon doctor` shows the permissions of both: this process and the running app.

## URL scheme

A `snapoleon://` link can be fired by any web page, so it is untrusted and deliberately weak. The link names an action
by its key (`snapoleon://capture_region`, `capture_window`, `record_region`, `ocr_region`, ...), and the app starts
that action the way its hotkey would, with the default pipeline: the overlay or picker opens and the user chooses the
target. Everything after `?` is ignored, so a link cannot carry a region, window, monitor, pipeline or file path.

- `capture_monitor`, `capture_all_monitors` and `capture_last_region` open the region overlay, and `record_screen`
  opens the region recorder, because a link must not capture or record the screen on its own.
- `step_capture_toggle` is refused.
- `snapoleon://library` and `snapoleon://settings` open those windows.
- There are no `edit` or `pin` links. To open or pin a file use `snapoleon edit <path>` / `snapoleon pin <path>`
  (file associations do this), and for a non-interactive capture with a region, window, monitor or pipeline use
  `snapoleon capture`; both come from your own command line, not from a link.

## MCP

`snapoleon mcp` speaks the Model Context Protocol (JSON-RPC 2.0, one message per line) on stdio, protocol versions
2025-06-18, 2025-03-26 and 2024-11-05. The tools only read: `save_to` writes a new file and never replaces one.

| Tool | Arguments | Returns |
|---|---|---|
| `list_monitors` | none | `index`, `name`, `x`, `y`, `width`, `height`, `scale_factor`, `primary`, `hdr`, `refresh_hz` |
| `list_windows` | none | front to back: `id`, `title`, `app`, `pid`, `x`, `y`, `width`, `height`, `z`, `minimized`; plus `screen_recording` and, when titles are hidden, a `hint` |
| `capture_screen` | `monitor` (default: primary), `save_to` | the image and `capture_id`, `width`, `height`, `screen_rect`, `scale`, `app`, `window_title`, `file` |
| `capture_window` | `title` (part of the title or app name) or `id`, `save_to` | as `capture_screen` |
| `capture_region` | `x`, `y`, `w`, `h`, `save_to` | as `capture_screen` |
| `ocr_image` | `path` or `capture_id`, `format`: `text`, `markdown_table`, `tsv`, `lines` | the text, or lines with boxes |
| `decode_codes` | `path` or `capture_id` | `codes` with `format`, `text`, `rect` |
| `search_history` | `query`, `limit` (1 to 50, default 10) | library entries with a text snippet, file and links |
| `get_capture` | `id` (a `capture_id` or a history id), `save_to` | the image and its details |
| `permissions` | none | the privacy permissions of the server and of the running app, the server's host app and a `hint` |

Coordinates are physical pixels, as on the command line. The image a tool returns is scaled down to at most 1568 px
on the long side (`scale` is the factor); pass `save_to` with an absolute path to also get the full-resolution image
as a PNG, JPEG or WebP file (by its extension), whose path comes back as `file`. The last four captures of a session
can be referred to by `capture_id`.

On macOS the server process belongs to the app that started it, so its own permissions are that app's. Captures go
through the Snapoleon app when the server may not read the screen (see [macOS](#macos)), which is why the app's
Screen Recording is the one to grant. Call `permissions` to see both.

Claude Code:

```sh
claude mcp add snapoleon -- /Applications/Snapoleon.app/Contents/MacOS/snapoleon mcp
```

Claude Desktop and other clients (`claude_desktop_config.json`):

```json
{
  "mcpServers": {
    "snapoleon": { "command": "/Applications/Snapoleon.app/Contents/MacOS/snapoleon", "args": ["mcp"] }
  }
}
```

On Windows and Linux use `snapoleon` (it is on PATH after installation) or the full path of the executable. Settings,
Advanced shows ready-to-copy CLI and MCP snippets for your installation.
