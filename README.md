# CrowPanel Desk · Rust dashboard

Messages, images, charts, outdoor weather and tickers for the ELECROW CrowPanel 5.79-inch ESP32-S3 e-paper display. Includes the Rust web server, laptop sender and firmware. The web interface stays local; the ESP32 can receive frames over USB or authenticated TCP over your 2.4 GHz Wi-Fi.

## Upgrade an existing working installation

Copy the contents of this release into your existing `crowpanel-rust` project. Keep your `.env` if you already have one. The archive includes no credentials or build output.

First, run in USB mode with the firmware you already have:

```bash
cd ~/dev/crowpanel-rust
bash scripts/web.sh
```

Open http://127.0.0.1:8787 . Choose Message, Image, Line chart, or Weather & tickers. Preview renders the same 792 × 272 monochrome image as the display. Images are fitted, composited over white and dithered. Charts accept 2–1000 numeric values in sample order. Text is multiline with manual line breaks; oversized text is clipped.

No firmware update is needed for the new web features over USB.

## Enable Wi-Fi delivery

### 1. Configure the board

```bash
cp .env.example .env
python3 -c 'import secrets; print(secrets.token_hex(24))'
```

Edit `.env` with your 2.4 GHz SSID/password and the generated `EPAPER_DEVICE_KEY`. Leave `EPAPER_DEVICE_HOST` commented out for now. Keys must contain 16–128 ASCII letters or digits. Wi-Fi credentials and the device key are compiled into the firmware; changing them requires reflashing. They are never sent to the frontend.

Flash once using the same Python 3.11 isolation that worked for the initial firmware:

```bash
pyenv install -s 3.11
(
  export PYENV_VERSION="$(pyenv latest 3.11)"
  export PYTHONNOUSERSITE=1
  unset PYTHONPATH PYTHONHOME VIRTUAL_ENV
  unset PIP_TARGET PIP_PREFIX PIP_USER
  source "$HOME/.cargo/env"
  bash scripts/flash.sh
)
```

If the board does not enter download mode: hold BOOT, tap RESET, release BOOT, retry. The flash script installs a 6 MiB factory app partition on the board’s 8 MiB flash to leave room for networking. No OTA partition is configured. Existing USB image transfers remain supported. The Wi-Fi thread reconnects after loss of connection; USB runs independently if the access point is unavailable.

### 2. Find the board address

The firmware prints `WIFI 192.168.x.x:7878` over its USB serial connection after getting an address. To see it:

```bash
espflash monitor --port /dev/serial/by-id/usb-1a86_USB_Serial-if00-port0
```

Tap RESET after opening the monitor. Close the monitor before using the sender. The router's connected-device list is another way to find the ESP32 address. A DHCP reservation in your router keeps it stable.

Uncomment and edit this line in `.env`:

```bash
EPAPER_DEVICE_HOST='192.168.1.50:7878'
```

Restart `bash scripts/web.sh`. It now uses Wi-Fi. Test an update while USB is still attached, then move the display to a USB wall adapter or suitable USB power bank. It no longer needs a laptop data cable. Battery-connector operation/charging and low-power sleep are not part of this release; Wi-Fi remains awake.

The TCP frame protocol requires the shared key before accepting an image. It is intended for a trusted LAN; it is not an encrypted internet-facing service.

### 3. Where the server runs

The server gathers feeds and renders images, then pushes them to the display. It must remain running for new updates. If it stops, e-paper retains the last image and its printed timestamps.

| Arrangement | What runs |
| --- | --- |
| USB development | Laptop server + USB connection |
| Wi-Fi display | Laptop server + display on a USB power adapter |
| Updates while laptop is off | Server on an always-on Linux machine + Wi-Fi display |

For an always-on Linux machine with Docker, copy the project and `.env`, set the Wi-Fi address/key, and run:

```bash
mkdir -p data
docker compose up -d --build
```

The container uses Linux host networking so the local web UI remains on loopback at `127.0.0.1:8787`. Open a browser on that machine, or tunnel it from your laptop:

```bash
ssh -L 8787:127.0.0.1:8787 user@your-server
```

Then browse http://127.0.0.1:8787 on your laptop. Config persists in `./data/dashboard.json`. No PostgreSQL, Redis or external frontend tools are needed for this small service. Container operation has not been tested in the preparation environment.

## Weather and ticker feeds

Expand **Dashboard feeds & automatic updates** in the web UI:

- Set your location label and coordinates. The initial coordinates are San Juan, Puerto Rico; edit them for your actual location.
- Enable Open-Meteo for temperature (F), relative humidity (%), wind (mph), and weather condition. These are modelled outdoor conditions, not a sensor inside your room. Fetched every 10 minutes.
- Add Coinbase product IDs such as `BTC-USD, ETH-USD`. Last-trade snapshots are polled every 60 seconds; this is not a tick-by-tick stream.
- Preview/send the dashboard manually, or enable scheduled display updates (default 300 seconds, minimum 120 seconds).

The full-refresh driver flashes during updates. Feed polling and physical display cadence are intentionally separate. Scheduled frames do not pile up while another delivery is active. Scheduled mode replaces manual messages when the next dashboard update is due; disable it to leave a manual image on screen.

Ticker charts accumulate up to 120 timestamped samples per source/symbol in memory. Source timestamps are preserved; quotes older than 120 seconds and weather older than 30 minutes are labelled STALE. Failed fetches preserve old values and show a FEED ERROR. The screen's printed timestamps remain fixed if the server goes offline. Snapshot and job histories reset on server restart; settings persist. The first sorted source/symbol gets the chart, followed by two ticker rows. The JSON API retains up to 12 source/symbol pairs.

Sources: https://open-meteo.com/en/docs and https://docs.cdp.coinbase.com/api-reference/exchange-api/rest-api/products/get-product-ticker . Observe each provider's usage terms. No API keys are required by these built-in endpoints. Both built-in feeds were fetched successfully by the server during preparation, with source timestamps preserved.

## HTTP API

The web API binds only to `127.0.0.1`. Browser requests from another origin are rejected. Existing text endpoints remain compatible.

### Text

```bash
curl http://127.0.0.1:8787/api/messages \
  -H 'Content-Type: application/json' \
  -d '{"text":"Hello, Jake","size":40}'
```

### Image

```bash
curl http://127.0.0.1:8787/api/images -F 'file=@picture.png'
curl 'http://127.0.0.1:8787/api/images?preview=true' -F 'file=@picture.png' -o preview.png
```

Images: PNG/JPEG/WebP/GIF, at most 4 MiB and 4096 × 4096; GIF uses the decoded first image. Every image is resized and converted to the display's monochrome frame.

### Chart

```bash
curl http://127.0.0.1:8787/api/charts \
  -H 'Content-Type: application/json' \
  -d '{"title":"Temperature","unit":"F","values":[80,82,81,84,83]}'
```

Add `?preview=true` to receive a PNG instead of queueing a display update.

### Integrate your stock/futures/crypto/sensor source

Your existing feed adapter or ChartOtter process can POST prices without giving the display provider credentials:

```bash
curl http://127.0.0.1:8787/api/quotes \
  -H 'Content-Type: application/json' \
  -d "{\"symbol\":\"SPY\",\"price\":500.25,\"source\":\"my-feed\",\"timestamp\":$(date +%s)}"
```

That price is an illustrative payload, not a market quote. Supply the actual source's Unix timestamp in seconds; do not stamp delayed data with the time of receipt. Prices must be finite and positive. Out-of-order samples, future timestamps beyond 60 seconds, and conflicting values at an identical timestamp are rejected. Same-time/same-price repeats are deduplicated.

The quote-ingestion endpoint caches data; it does not send a frame immediately. Enable scheduled dashboards or call:

```bash
curl -X POST http://127.0.0.1:8787/api/dashboard/frame
curl 'http://127.0.0.1:8787/api/dashboard/frame?preview=true' -o dashboard.png
```

| Endpoint | Purpose |
| --- | --- |
| GET /api/status | Transport configuration, serial path presence, queue availability |
| POST /api/messages | Queue text (size 12–96) |
| POST /api/preview | Text preview PNG |
| POST /api/images | Queue multipart image; `?preview=true` for PNG |
| POST /api/charts | Queue JSON line chart; `?preview=true` for PNG |
| POST /api/quotes | Ingest source/symbol/price/timestamp |
| GET /api/dashboard | Cached weather, ticker samples, source errors |
| GET or POST /api/dashboard/frame | Queue dashboard; `?preview=true` for PNG |
| GET or PUT /api/config | Get/save dashboard feed settings |
| GET /api/messages | Latest 100 delivery jobs |
| GET /api/messages/{id} | Specific job status/error |

Queue responses are HTTP 202 with job IDs. Completion states are `displayed`, `failed`, or `simulated`; `displayed` requires the firmware's DONE acknowledgement. The web status shows the configured transport, not a Wi-Fi health probe. A configured IP can be unreachable; job errors report that. Eight waiting jobs plus one active delivery are allowed. Errors are not automatically retried because a physical refresh may have occurred before a timeout. No guarantees of persistence are made for jobs across restart.

## Runtime options

Settings are shell assignments in `.env`, loaded by `scripts/web.sh` and `scripts/flash.sh`:

- `EPAPER_DEVICE_HOST`: `IP:7878`; absent means USB.
- `EPAPER_DEVICE_KEY`: shared Wi-Fi key; compiled into firmware and supplied to the sender at runtime.
- `EPAPER_PORT`: alternative serial device path.
- `EPAPER_HTTP_PORT`: local web port, default 8787.
- `EPAPER_CONFIG`: settings file, default `data/dashboard.json`.
- `EPAPER_DRY_RUN=1`: render and simulate delivery without contacting USB or Wi-Fi.
- `EPAPER_BIN`: custom sender binary when running the web executable directly.
- `EPAPER_FONT`: font for server-rendered charts/dashboard (defaults to DejaVu Sans).

## Checks

The Rust sender and server compile and the mock-device integration checks pass. Both external feed adapters were exercised successfully, and the generated dashboard PNG was visually inspected. The ESP32 toolchain was installed and a cross-build attempted, but the initial ESP-IDF dependency checkout did not finish; the Wi-Fi firmware therefore still requires a complete cross-build and physical board test on your laptop. The existing display driver sequence is retained. Browser automation was unavailable in the preparation environment; frontend JavaScript syntax was checked, but desktop/mobile visual verification remains local.

```bash
cargo test --manifest-path cli/Cargo.toml
cargo test --manifest-path web/Cargo.toml
cargo build --manifest-path cli/Cargo.toml
cargo build --manifest-path web/Cargo.toml
python3 web/tests/integration.py
```

The Python integration check requires Pillow. It starts its own mock TCP display and server on temporary local ports, then checks key authentication, frame size/CRC, chunk acknowledgements, text/chart/dashboard queue completion, image preview, invalid quotes and config writes. It never contacts physical hardware. `web/tests/smoke.py` additionally checks a separately running default-port simulation server.

Hardware: ESP32-S3-WROOM-1-N8R8, dual SSD1683. GPIO power7, SCK12, MOSI11, CS45, DC46, RESET47, BUSY48. The 792 × 272 visible image maps to an 800 × 272 buffer with eight padding columns at x=396. Original display command sequence follows ELECROW's reference repository: https://github.com/Elecrow-RD/CrowPanel-ESP32-5.79-E-paper-HMI-Display-with-272-792 .
