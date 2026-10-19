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
| `snapoleon ocr <image>` | Recognize text in an image or in a fresh capture. |
| `snapoleon qr` | Decode a QR or barcode from an image or capture, or generate a QR code. |
| `snapoleon upload <file>` | Upload a file to a configured destination and print the link. |
| `snapoleon pin <image>` | Show an image as an always-on-top pin. |
| `snapoleon edit <image>` | Open an image or `.snap` project in the editor. |
| `snapoleon history` | List and search the capture library. |
| `snapoleon pipeline` | List and run configured pipelines. |
| `snapoleon mcp` | Run the MCP server on stdio. |
| `snapoleon settings` | Open the Settings window. |
| `snapoleon quit` | Quit the running app (does nothing when it is not running). |
| `snapoleon url <snapoleon://...>` | Handle a `snapoleon://` URL (used by the OS URL scheme registration). |

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

`snapoleon mcp` speaks JSON-RPC 2.0 over stdio. Tools planned for the first release: `list_monitors`,
`list_windows`, `capture_screen`, `capture_window`, `capture_region`, `ocr_image`, `decode_codes`,
`search_history`, `get_capture`. Example client configuration:

```json
{
  "mcpServers": {
    "snapoleon": { "command": "snapoleon", "args": ["mcp"] }
  }
}
```

Settings, Advanced shows ready-to-copy CLI and MCP snippets for your installation.
