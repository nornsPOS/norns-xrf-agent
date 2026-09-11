# Norns XRF Agent

Reads finished analyser measurements from the manufacturer's SQLite database.
It never starts a measurement or writes to that database.

This source is maintained in Norns POS at `apps/pruefgeraet-bote` and mirrored
without changes to the standalone `norns-xrf-agent` repository. Both builds
use the same Cargo files, sources and process tests.

## Installation on Windows

Open `norns-xrf-agent.exe` on the analyser PC and approve the Windows elevation
prompt. The installer locates the source through the analyser process on TCP
9612. If it cannot find the source, run:

```text
norns-xrf-agent.exe --einrichten "D:\XRFSeries\XRF-A7\Data\User\samplesummary.db"
```

A scheduled task runs as SYSTEM at startup, without a login, with restart on
failure and no three-day execution limit. Program and connection code are kept
in `%ProgramData%\Norns\XrfAgent`, restricted to administrators and SYSTEM.
The firewall rule permits TCP 9614 from the local subnet only. A successful
installation requires an authenticated, healthy response from the new process.

The window displays a connection code. Enter it once in Norns device settings.
The code persists across restarts. Older agents without this protocol need to
be updated together with the register; no unauthenticated fallback is used.
The shared code authenticates access over HTTP; transport encryption and
server identity verification are not provided by this protocol. Use only a
trusted shop network. Do not expose the port to the internet.

Commands: `--jetzt [path]` runs in the foreground, `--probe [path]` reads once,
`--entfernen` removes the task and firewall rule. `--einrichten-still` is for
an already elevated deployment/test process; it does not display the code.

## Result contract (protocol 2)

`GET /messungen?seit=-1`, header `X-Norns-Bote: <connection code>`.
`/stand` and `/` use the same authenticated envelope.

```json
{
  "protokoll": 2,
  "quelleOk": true,
  "alterMs": 100,
  "sitzung": "<64 hexadecimal characters, new for each process>",
  "revision": 1,
  "fehler": null,
  "messungen": [{
    "id": 502,
    "anwendung": "AuAgX",
    "probe": "Ring",
    "gemessenAm": 1780000000,
    "elemente": [{"symbol": "Au", "promille": 585.0}],
    "goldPromille": 585.0,
    "karat": 14.0
  }]
}
```

The newest 200 rows are refreshed every 700 ms using a read-only SQLite
connection, including WAL commits. Only the contiguous valid newest batch is
returned: an older unreadable result ends that batch, so an abandoned historical
measurement cannot block newer valid results. The newest unreadable result
still fails closed; the reader never falls back to an older valid measurement. Each refresh replaces the snapshot, so
updates to an existing ID, cleared databases and restarted numbering are seen.
`seit` filters that bounded snapshot; it is not a lossless historical export.
Use `seit=-1` for the current snapshot, including in-place corrections.

`alterMs` is monotonic time since the last successful source read. Source
failure, an incomplete newest measurement or more than five seconds without a
successful read yield HTTP 503, `quelleOk:false` and an empty measurement list.
401 rejects a missing/wrong code. Results are never silently replaced with old
cached values. The register must still validate measurement time and require
explicit acceptance for the current item.

The HTTP server caps concurrent connections at eight, headers at 8 KiB and
socket read/write waiting at two seconds. Only GET routes are supported.

## Verification

```text
cargo test --locked
cargo clippy --all-targets -- -D warnings
```

Process tests create synthetic SQLite/WAL data, start the actual executable,
query HTTP, correct the same ID, break/recover the source and reset numbering.
They terminate their processes and remove their fixtures.

`scripts/windows-probe.ps1 -Programm <exe>` additionally installs and restarts
the actual task on a disposable Windows runner. Never run it against a merchant
installation. CI captures this separately from physical analyser acceptance.

For isolated testing, `NORNS_XRF_HOME` and `NORNS_XRF_LISTEN` override the data
folder and listening address. They are foreground/test overrides; the installed
Windows task uses the durable system defaults.
