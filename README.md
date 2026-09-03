# Norns XRF Agent

Reads finished measurements from an XRF analyser and offers them, read-only,
on the local network.

## Install

Copy the executable onto the analyser's PC and open it. Windows asks for
administrator rights; confirm the dialog. The result is shown in a window.

The agent locates the measurement database, registers a scheduled task that
starts with the machine and runs as SYSTEM, adds a firewall rule limited to
the local subnet, and starts serving.

| Command | Effect |
|---|---|
| *(no argument)* | Install and start |
| `--probe [path]` | Print the five most recent measurements and exit |
| `--jetzt [path]` | Run in the foreground |
| `--entfernen` | Remove the task and the firewall rule |

If the database is not found automatically, pass its path:

```
norns-xrf-agent.exe "D:\XRFSeries\XRF-A7\Data\User\samplesummary.db"
```

## Interface

TCP 9614, JSON.

| Address | Answer |
|---|---|
| `GET /` | Version, database path, last measurement, count |
| `GET /messungen?seit=<id>` | All measurements with an id greater than `<id>` |

A measurement carries the application used, the sample name, the time, every
element found with its share in per mille, and the derived gold content and
karat. Elements reported as zero are omitted; trace elements from gemstone
applications are kept.

```json
{
  "id": 2150,
  "anwendung": "AuAgX",
  "probe": "TempSpektr",
  "gemessenAm": 1788457620,
  "elemente": [{ "symbol": "Au", "promille": 999.9 }],
  "goldPromille": 999.9,
  "karat": 24.0
}
```

## Safety

The analyser contains an X-ray tube. The agent is built so that it cannot act
on the device:

* Read-only. No write statement, no address that changes anything, and no
  address that starts a measurement.
* Local subnet only. No outbound connections.
* The measurement is read through a read-only connection; if the file is
  locked, a copy is read instead.

## Build

```
cargo build --release --target x86_64-pc-windows-msvc
```

Rust toolchain only. SQLite is compiled in; nothing has to be installed
beside the executable.

## Licence

MIT.
