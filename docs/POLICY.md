# Managed policy for IT administrators

Snapoleon can be configured centrally. Policy values override the user's own settings and are re-applied each time
Snapoleon starts. Where a setting is managed, the Settings window shows it as managed by your organization.

## Settings

| Name | Type | Effect |
|---|---|---|
| `DisableUploads` | bool | Removes every upload destination and the default destination. Local save and clipboard still work. |
| `DisableRecording` | bool | Turns screen recording off. Screenshots are unaffected. |
| `DisableAI` | bool | Turns off all AI features, so no image or text is sent to an AI endpoint. |
| `DisableHistory` | bool | Turns off the capture library; nothing is recorded in it. |
| `ForceRedactBeforeUpload` | bool | Always strips metadata and forces automatic redaction of detected sensitive data before upload. |
| `RetentionDays` | integer | History entries older than this many days are deleted. |
| `Language` | string | Locale code such as `en` or `ru`. Overrides the user's choice. |
| `DefaultDestination` | string | Id of the default destination (must match an id in `DestinationsToml`). |
| `DestinationsToml` | string | TOML text with one or more `[[destinations]]` tables. Replaces the user's destination list; users cannot add others. |

Booleans are true for `1`, `true`, `True` or `yes` (a registry DWORD of 1, a plist boolean true). Any other value
is false.

A destination in `DestinationsToml` has an `id`, a `name`, a `type` and type-specific fields. Examples:

```toml
[[destinations]]
id = "team"
name = "Company Snapoleon server"
type = "snap_server"
url = "https://snap.example.com"

[[destinations]]
id = "archive"
name = "Archive bucket"
type = "s3"
endpoint = "https://s3.example.com"
region = "us-east-1"
bucket = "screenshots"
access_key_id = "AKIA..."
public_base_url = "https://img.example.com"
secret_ref = "archive"        # secret key stored in the user's OS keyring under this name
```

Secrets are never part of policy. Provision them in the user's keyring (or use a destination that authenticates
with the user's own sign-in, such as the team server with OIDC).

## Sources and precedence

Snapoleon reads, and merges, these sources. Boolean `Disable*` and `ForceRedact*` flags are OR-ed together (any
source turning something off wins). For the other values, a platform store (registry or managed preferences)
takes precedence over `policy.toml`.

| Platform | Source |
|---|---|
| Windows | `HKLM\SOFTWARE\Policies\Snapoleon`, then `HKCU\SOFTWARE\Policies\Snapoleon`, then `%ProgramData%\Snapoleon\policy.toml` |
| macOS | Managed preferences `/Library/Managed Preferences/app.snapoleon.plist` (and the per-user copy), then `/Library/Application Support/Snapoleon/policy.toml` |
| Linux | `/etc/snapoleon/policy.toml` |

On Windows, machine policy (HKLM) wins over user policy (HKCU). `REG_SZ`, `REG_EXPAND_SZ` and `REG_DWORD` values
are read; other registry types are ignored.

## Windows: Group Policy (ADMX)

Templates are in `packaging/policy/`:

```
Snapoleon.admx
en-US\Snapoleon.adml
ru-RU\Snapoleon.adml
```

Install them in the Central Store of your domain, or locally for testing:

```powershell
copy packaging\policy\Snapoleon.admx            C:\Windows\PolicyDefinitions\
copy packaging\policy\en-US\Snapoleon.adml      C:\Windows\PolicyDefinitions\en-US\
copy packaging\policy\ru-RU\Snapoleon.adml      C:\Windows\PolicyDefinitions\ru-RU\
```

For a domain use `\\<domain>\SYSVOL\<domain>\Policies\PolicyDefinitions\` instead. Then open the Group Policy
editor: Computer Configuration (or User Configuration), Administrative Templates, Snapoleon. Every policy is
available in both the computer and user class and writes to `Software\Policies\Snapoleon`.

Notes:

- A boolean policy set to Enabled writes `1`; set to Disabled writes `0` (explicitly allow); Not Configured
  removes the value.
- `RetentionDays` is a `REG_DWORD` between 1 and 3650.
- `DestinationsToml` is a `REG_SZ`. The Group Policy editor limits string values to 1023 characters. For longer
  destination lists deploy `%ProgramData%\Snapoleon\policy.toml` with a file or Group Policy Preferences item.

You can also set values without Group Policy, for example from a deployment script:

```powershell
$k = "HKLM:\SOFTWARE\Policies\Snapoleon"
New-Item -Path $k -Force | Out-Null
New-ItemProperty -Path $k -Name DisableAI -Value 1 -PropertyType DWord -Force
New-ItemProperty -Path $k -Name Language -Value "en" -PropertyType String -Force
```

## macOS: configuration profile

`packaging/macos/Snapoleon.mobileconfig.example` is a configuration profile with a payload of type
`app.snapoleon` that sets each key. Deploy it through your MDM (Jamf, Kandji, Intune, Mosyle, ...). The MDM
writes the values to `/Library/Managed Preferences/app.snapoleon.plist`, which Snapoleon reads. Replace the two
`PayloadUUID` values with fresh ones. Booleans must be boolean (`<true/>`) or integer, and `RetentionDays` an
integer.

Check what is applied on a Mac:

```sh
plutil -p "/Library/Managed Preferences/app.snapoleon.plist"
```

Pre-approving Screen Recording for Snapoleon requires a Privacy Preferences Policy Control payload (PPPC) that
references the app's code signing identity; that is a separate MDM payload and is not part of this profile.

## All platforms: policy.toml

A file with the same names in snake case:

```toml
disable_uploads = false
disable_recording = false
disable_ai = true
disable_history = false
force_redact_before_upload = true
retention_days = 30
language = "en"
default_destination = "team"

[[destinations]]
id = "team"
name = "Company Snapoleon server"
type = "snap_server"
url = "https://snap.example.com"
```

Locations: `%ProgramData%\Snapoleon\policy.toml` (Windows), `/Library/Application Support/Snapoleon/policy.toml`
(macOS), `/etc/snapoleon/policy.toml` (Linux). The sample is `packaging/policy/policy.toml.example`, and the
`.deb` installs it as `/etc/snapoleon/policy.toml.example`. Make the file writable by administrators only, since
a user who can edit it can also remove the restrictions. An invalid file is ignored and a warning is logged.

## Deploying the application

| Goal | Method |
|---|---|
| Per-machine install for all users | `msiexec /i Snapoleon-<ver>-windows-x64.msi /qn` (Intune, SCCM, GPO software install) |
| Per-user install without admin rights | `Snapoleon-<ver>-windows-x64-setup.exe /S` |
| No installation | Portable zip (contains a `portable` marker file; data stays next to the executable) |
| winget in a managed environment | `winget install Snapoleon.Snapoleon --scope machine` (MSI) |

Network access happens only for destinations you or your policy configure, for AI endpoints (unless `DisableAI` is set),
and for on-demand downloads of optional OCR models.
