# Command line interface

Status: stub. This page lists the planned subcommands from [SPEC.md](SPEC.md). The engine owns the final
interface; run `snapoleon --help` and `snapoleon <command> --help` for the authoritative option list once the CLI
is released.

`snapoleon` with no arguments starts the tray app. The app is a single instance: if one is already running, a
second invocation forwards its arguments to it over local IPC and exits, so `snapoleon capture region` from a
script or a compositor key binding triggers the running app. When no instance is running, the command runs
headless where that makes sense.

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
