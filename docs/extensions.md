# Your own extensions

Programs of your own can show what they do in Trayward: a backup's progress, when a job runs next,
whether a build passed. Each writes a JSON file; Trayward reads it and shows its rows in the widget's
tooltip and in the flyout.

Trayward only **reads** these files. Nothing in them is run, opened, clicked or sent anywhere: a file
can't start a program, open a link, show a notification or reach your network. It can only say
which rows are shown, within the limits below. Your program runs on its own (from Task Scheduler,
as a service, by hand); Trayward never starts it.

## Switching it on

1. Settings → **Extensions** → **Your custom extensions**. This adds the **Custom extensions** page.
2. Your program writes its file into `%LOCALAPPDATA%\Trayward\extensions\` (**Open** on that page
   shows the folder).
3. The file appears on that page with its label, id and description, switched off. Switch it on:
   its rows show in the tooltip and the flyout.

A new file stays off until you switch it on, so nothing shows that you haven't looked at. A file
Trayward can't read is listed with the reason in red.

To try it without a program of your own: `npm run extension-example` (in a clone of this repository)
writes a made-up extension and keeps it up to date every 2 seconds; `-- --remove` removes it again.

## The format (version 1)

One file per extension, UTF-8, with the extension `.json`:

```json
{
  "version": 1,
  "id": "nas-backup",
  "label": "Backup",
  "description": "Nightly backup of the NAS",
  "tooltip": [
    { "label": "Backup", "type": "progress", "value": 42 },
    { "label": "Next run", "type": "time", "value": "2026-11-18T23:00:00+01:00" }
  ],
  "flyout": [
    { "label": "Photos", "type": "progress", "value": 412, "max": 640, "text": "412 of 640 GB" },
    { "label": "Documents", "type": "text", "value": "Done", "state": "ok" },
    { "label": "Videos", "type": "text", "value": "Share offline", "state": "bad" },
    { "label": "Started", "type": "time", "value": "2026-11-18T13:12:00+01:00" }
  ],
  "stale_after": 600
}
```

| Field         | Required | What it is                                                                                                                                             |
| ------------- | -------- | ------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `version`     | yes      | `1`. A file of another version is listed as unreadable.                                                                                                |
| `id`          | yes      | 1 to 40 lower-case letters, digits or hyphens. Your switch is kept under it, so keep it the same. Two files with one id: the first one (by name) wins. |
| `label`       | yes      | The extension's name: its card in the settings, its heading in the tooltip and the flyout. Up to 40 characters.                                        |
| `description` | no       | What it is, on its card in the settings. Up to 100 characters.                                                                                         |
| `tooltip`     | no       | Rows for the widget's tooltip, at most 8.                                                                                                              |
| `flyout`      | no       | Rows for the flyout, under the extension's label, at most 8.                                                                                           |
| `stale_after` | no       | Seconds. When the file hasn't changed for longer, it's shown as out of date (see below).                                                               |

### Rows

Each row has a `label` (up to 40 characters), a `type`, a `value`, and optionally a `state`.

| `type`     | `value`                                                        | Shown                                                                                                                         |
| ---------- | -------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------- |
| `progress` | A number, out of `max` (default 100)                           | A bar, and the percentage beside it, or `text` instead when given ("412 of 640 GB"). Below 0 or above `max` stays in the bar. |
| `time`     | A time with its time zone: `2026-11-18T23:00:00+01:00` or `…Z` | How long until it (`in 2h 05m`) or since it (`5m ago`). Trayward counts it every second: no need to write the file for that.  |
| `text`     | Text, up to 100 characters                                     | As written.                                                                                                                   |

`state` colours the value (and the bar): `ok` green, `warn` yellow, `bad` red. Without it, the value
has the usual colour and the bar the accent colour.

A row of a type this Trayward doesn't know is left out, so a file written for a newer version still
shows what it can. Other fields are ignored (a `link` is never opened).

Text is shown as written but for control characters and invisible direction marks, which are taken
out; longer text is cut with "…".

### Out of date

With `stale_after`, a file that hasn't changed for longer is shown as out of date: in the tooltip,
its heading with a yellow note instead of its rows ("Not updated for 15m"); in the flyout, that note beside the
label and the rows greyed out. That way a program that stopped doesn't leave old values looking
current. Write the file again even when nothing changed if you want it to count as up to date.

### Limits

- At most 5 files are read (by name); the settings say when there are more.
- A file over 64 KB isn't read.
- 8 rows each for the tooltip and the flyout; texts as above. A file with more rows in one of them,
  or with a `state` other than the three above, isn't shown: the settings say what's wrong with it.

## Writing the file

Write the whole file to a temporary name first, then rename it over the old one. Trayward reads the
folder when a file in it changes; renaming makes the new file appear at once and whole. (A file
caught half-written is listed as not valid JSON until the next change.) Trayward reads only files
that changed, so writing often costs it nearly nothing; it also looks at the folder once a minute.

PowerShell, run from Task Scheduler for example:

```powershell
$dir = Join-Path $env:LOCALAPPDATA "Trayward\extensions"
New-Item -ItemType Directory -Force $dir | Out-Null

$done = 42  # what your job knows
$file = @{
    version     = 1
    id          = "nas-backup"
    label       = "Backup"
    description = "Nightly backup of the NAS"
    tooltip     = @(
        @{ label = "Backup"; type = "progress"; value = $done }
        @{ label = "Next run"; type = "time"; value = (Get-Date).Date.AddDays(1).AddHours(2).ToString("o") }
    )
    stale_after = 600
}
$temp = Join-Path $dir "nas-backup.json.tmp"
$file | ConvertTo-Json -Depth 5 | Set-Content -Encoding utf8 $temp
Move-Item -Force $temp (Join-Path $dir "nas-backup.json")
```

`ToString("o")` gives the time with its time zone, as the format wants it. Windows PowerShell 5's
`-Encoding utf8` starts the file with a byte order mark; Trayward reads that too.

## Removing an extension

Delete its file. Its switch is kept in case it comes back under the same id.
