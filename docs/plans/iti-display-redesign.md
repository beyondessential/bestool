# iti display redesign — single-process, no ZeroMQ

## Background

The Iti device's LCD is currently driven by a producer/consumer fleet wired
together over ZeroMQ:

- **`iti-lcd-server.service`** (`bestool iti lcd serve`) owns the SPI/GPIO
  link to the ST7789V2 panel and listens on a ZMQ REP socket
  (`tcp://[::1]:2009`) for JSON `Screen` messages.
- **Producers** REQ-connect to that socket and send positioned `{x, y,
  fill, stroke, text, ...}` items:
  - `iti-battery.service` → `bestool iti battery --watch 10s --update-screen 220`
  - `iti-temperature.service` → `bestool iti temperature --watch 10s --update-screen 195`
  - `iti-sparks.service` → `bestool iti sparks` (CPU+mem spark lines)
  - `services/iti-addresses` (bash + jq + `bestool iti lcd send`) — hostname + IPs
  - `services/iti-lcd-wifi` (bash + nmcli + `bestool iti lcd send`) — current SSID
  - `services/iti-localtime` (bash + date + `bestool iti lcd send`) — date+time
- **`bestool iti lcd send/clear/on/off`** ad-hoc CLI for one-shot updates and
  for the bash producers above.

### Why this is being redesigned

- **ZMQ is heavyweight for this.** It pulls libzmq and `zeromq-src`, slows the
  build noticeably, and we use loopback IPv6 only — every "feature" of ZMQ
  (network transparency, multi-language clients, pub/sub, etc.) is unused.
- **The flexibility was theoretical.** The point of the producer/consumer
  split was to "rapidly add stuff to the display." In practice the layout has
  been stable for a long time and we have not added anything in months.
- **Each shell-script producer spins up a full ZMQ context per send**
  (loaded once a minute, but still: heavy for what is logically `printf`).
- **Coordination is by hand.** Every producer hard-codes its Y position
  in its service unit; nothing prevents two producers from claiming the same
  pixels.
- **JSON-over-ZMQ wire format isn't versioned**, has no acks beyond
  "received", and silently drops reconnect logic.

## Goals

- Single long-running process (`bestool iti display`) owns the LCD and renders
  the whole screen on a tick.
- All current display content preserved: clock, hostname + IPs, Wi-Fi name,
  battery, temperature, CPU/mem spark lines.
- Drop the ZMQ dep entirely; remove the `__iti` feature flag.
- Drop the bash service scripts (`iti-addresses`, `iti-lcd-wifi`,
  `iti-localtime`).
- One systemd unit replaces six.

## Non-goals

- Hot-pluggable widgets, scripted user content, theme files. If the display
  needs a new widget, write Rust. The "rapidly add stuff" use case is
  withdrawn.
- Changing the LCD driver crate (`rpi-st7789v2-driver`) — untouched.
- Touching unrelated `iti` subcommands (`iti improv-wifi` from the previous
  branch). The `iti battery` and `iti temperature` *one-shot* CLIs are kept;
  only their `--update-screen` flag goes away.

## Architecture

### Crate layout

The display lives entirely in `bestool` (no new crate). Reorganising:

```
crates/bestool/src/actions/iti/
├── battery.rs              # ad-hoc CLI; sampler factored out into samplers/
├── temperature.rs          # ad-hoc CLI; sampler factored out into samplers/
├── improv_wifi.rs          # unchanged (from previous branch)
├── display.rs              # NEW: long-running service entry point
├── display/
│   ├── canvas.rs           # owns the Driver, holds a frame buffer, batches updates
│   ├── layout.rs           # auto-arranges enabled widgets in a vertical stack
│   ├── widget.rs           # `Widget` trait + ticking machinery
│   └── widgets/
│       ├── addresses.rs    # hostname + first 3 non-bridge IPv4s
│       ├── battery.rs
│       ├── clock.rs
│       ├── sparks.rs
│       ├── temperature.rs
│       └── wifi.rs
└── samplers/
    ├── battery.rs          # MAX17048 + powered-pin read
    └── temperature.rs      # vcgencmd or /sys/class/thermal/thermal_zone0/temp
```

The `lcd/` directory and the `iti lcd` / `iti sparks` subcommands go away.
No direct-driver debug subcommand: if the LCD itself needs debugging, edit
the service code or stop the unit and run a one-shot binary.

### Widget contract

```rust
pub trait Widget: Send + 'static {
    /// Stable name; used for --enable / --disable and logging.
    fn name(&self) -> &'static str;

    /// How often this widget should refresh.
    fn interval(&self) -> Duration;

    /// Pixel height the layout reserves for this widget.
    fn height(&self) -> u32;

    /// Compute the next frame for this widget. The layout supplies the
    /// rectangle this widget owns. Must be cheap to call; long sampling work
    /// happens behind the scenes (each widget can hold its own state).
    async fn render(&mut self, area: Rectangle) -> Vec<DrawCommand>;
}
```

`DrawCommand` is the existing `Item` type renamed and moved out of the
`json::` module (it stays a plain struct with rectangle + optional fill +
optional text + stroke colour). No serde derives needed any more.

### Tick loop

```rust
loop {
    for widget in &mut widgets {
        if widget.due() {
            let cmds = widget.render(layout.area_for(widget)).await;
            canvas.apply(cmds).await?;
        }
    }
    tokio::time::sleep_until(next_due).await;
}
```

A single `Mutex<Driver>` lives inside `Canvas`; widgets never see the SPI
directly. Widgets that need OS resources (D-Bus to NM, I2C to the X1201
gauge) hold them themselves between ticks rather than re-opening every time.

### Layout

Hard-coded vertical stack, top-to-bottom, with the same visual order as
today's screen:

1. clock (top, narrow, fixed height)
2. addresses
3. wifi
4. temperature
5. battery (single row alongside temperature: see below)
6. sparks (bottom)

Two columns where it fits today (e.g. temperature on the left, battery on the
right) are kept. The simplest expression: each widget gets a `Rectangle`
configured at compile time in a static `LAYOUT` table; we don't auto-pack.
"Auto-arrange" is over-engineering for a screen we'll stare at for years.

The current Y coordinates in the deployed services (`195` for temp, `220`
for battery, etc.) are the source of truth for the static layout.

### Configuration

CLI args only. Default layout matches current deployment.

```
bestool iti display run \
    [--spi 0] [--backlight 18] [--reset 27] [--dc 25] [--ce 0] [--frequency 20000000] \
    [--disable WIDGET[,WIDGET...]] \
    [--interval-clock 10s] [--interval-fast 10s] [--interval-slow 60s]
```

`--disable` lets a deployment turn off a widget without recompiling. No TOML
config file, no `--enable`-with-positions: trying to expose the layout via
config re-creates the problem we're solving.

### Sampler reuse

`iti battery` and `iti temperature` keep their existing CLI surface (sans
`--update-screen`). Their *sampling* code moves into
`actions/iti/samplers/{battery,temperature}.rs`. Both the CLI and the widget
share the same sampler. This is a refactor inside `bestool` — no new crate.

### Wi-Fi widget without nmcli

The existing `iti-lcd-wifi` script shells out to `nmcli`. The improv-wifi
branch already has zbus + NM proxy code. The Wi-Fi widget reads the active
connection via D-Bus (`org.freedesktop.NetworkManager.Connection.Active` →
`Id` property after walking from `PrimaryConnection`). If NM isn't reachable,
display `Wifi: not connected`.

(This adds a zbus dep to `iti-display`, but we already pull it in for
`iti-improv-wifi` and the binary co-installs both.)

### Service unit

One unit replaces six:

```ini
# services/iti-display.service
[Unit]
Description=Iti LCD display
After=network.target

[Service]
ExecStart=/usr/local/bin/bestool --log-timeless iti display run
ExecStop=/usr/local/bin/bestool --log-timeless iti display stop
Restart=always
RestartSec=5s

[Install]
WantedBy=multi-user.target
```

`iti display stop` is implemented by sending SIGTERM to the running process
via the systemd `MainPID`; `bestool iti display run` shuts down cleanly on
SIGTERM (turn LCD off, sleep panel, exit).

(Or we lean on systemd: `KillSignal=SIGTERM`, no `ExecStop` shell-out, single
ExecStart, panic if the LCD complains. Likely the cleaner path; finalise
during step 13.)

## Migration

### Removed

- `bestool iti lcd` subcommand and the `crates/bestool/src/actions/iti/lcd*`
  files.
- `bestool iti sparks` subcommand and `actions/iti/sparks.rs`.
- `--update-screen` and `--zmq-socket` flags on `iti battery` and `iti
  temperature`.
- `__iti` feature flag and the `dep:zmq` line.
- Service files: `iti-lcd-server.service`, `iti-battery.service`,
  `iti-temperature.service`, `iti-sparks.service`.
- Shell scripts and units: `iti-addresses`, `iti-addresses.service`,
  `iti-lcd-wifi`, `iti-lcd-wifi.service`, `iti-localtime`,
  `iti-localtime.service`.

### Added

- `bestool iti display run` (the service).
- `services/iti-display.service`.
- `iti-display` cargo feature replacing `iti-lcd` / `iti-sparks` / `__iti`.

### Kept

- `bestool iti battery` (without `--update-screen`).
- `bestool iti temperature` (without `--update-screen`).
- `bestool iti improv-wifi` (from previous branch, unaffected).
- `rpi-st7789v2-driver` crate, unchanged.

## Implementation order

Each step a single commit. Stop on user feedback at any point.

1. `plan: iti-display-redesign` — this file.
2. `refactor(iti): extract battery sampler` — pull the MAX17048 + GPIO
   reading out of `actions/iti/battery.rs` into
   `actions/iti/samplers/battery.rs`. CLI continues to work; no behaviour
   change.
3. `refactor(iti): extract temperature sampler` — same shape.
4. `feat(iti): scaffold display subcommand` — new `iti-display` feature, new
   `actions/iti/display.rs` with a `run` that opens the LCD, clears, sleeps
   on ctrl-c. Add the systemd unit. No widgets yet.
5. `feat(iti-display): widget trait and canvas` — `Widget` trait, `Canvas`
   wrapping the driver, drop the JSON `Screen`/`Item` types into
   `display::draw` (renamed; serde dropped). Lift `Item::draw` into a plain
   draw method.
6. `feat(iti-display): layout table` — static rectangles for each widget,
   `--disable` flag wiring.
7. `feat(iti-display): clock widget` — replaces `iti-localtime`.
8. `feat(iti-display): addresses widget` — replaces `iti-addresses`. Use
   `if-addrs` or read `/proc/net/route` + `getifaddrs` via `nix`. Filter
   bridges/`podman*`/`tailscale*` like the bash version.
9. `feat(iti-display): wifi widget` — D-Bus query of NM's `PrimaryConnection`.
10. `feat(iti-display): temperature widget` — uses sampler from step 3.
11. `feat(iti-display): battery widget` — uses sampler from step 2,
    including the rate / time-remaining estimation logic that currently
    lives in `battery.rs::once`.
12. `feat(iti-display): sparks widget` — port `sparks.rs` rendering into
    a widget. Drop the `iti sparks` subcommand at the end of this step.
13. `feat(iti-display): graceful shutdown` — SIGTERM clears + powers down
    the LCD before exit.
14. `chore(iti): drop --update-screen from battery/temperature CLIs` —
    delete the flags and their dependencies (no more `--zmq-socket`,
    `lcd::send`, etc.). At this point nothing links zmq.
15. `chore(iti): remove iti lcd subcommand and zmq dep` — delete
    `actions/iti/lcd.rs` and `actions/iti/lcd/`, drop `__iti` feature, drop
    `zmq` from Cargo.toml.
16. `chore(iti): drop legacy services` — remove the six replaced unit files
    and the three bash scripts.
17. `unplan: iti-display-redesign (all phases implemented)`.

## Out of scope

- Per-deployment custom layouts. If a future deployment needs a different
  layout, ship a different binary or add a code-level variant.
- Brightness / backlight scheduling.
- Replacing `vcgencmd` with sysfs reads (a separate cleanup).
- Touch input (the panel doesn't have any).
- A direct-driver `iti display test {on,off,clear,pattern}` debug
  subcommand (no consumers).
- Compatibility shims for `bestool iti lcd send` (no external consumers).
