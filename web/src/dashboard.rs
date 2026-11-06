use super::*;
use anyhow::{ensure, Context, Result};
use image::{GrayImage, Luma};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub location: String,
    pub latitude: f64,
    pub longitude: f64,
    pub weather_enabled: bool,
    pub coinbase_products: Vec<String>,
    pub auto_display: bool,
    pub display_interval_seconds: u64,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            location: "San Juan, PR (edit location)".into(),
            latitude: 18.4663,
            longitude: -66.1057,
            weather_enabled: false,
            coinbase_products: vec![],
            auto_display: false,
            display_interval_seconds: 300,
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Quote {
    pub symbol: String,
    pub price: f64,
    pub source: String,
    pub timestamp: u64,
}
#[derive(Clone, Serialize)]
pub struct Series {
    pub latest: Quote,
    pub samples: Vec<(u64, f64)>,
}
#[derive(Clone, Serialize)]
pub struct Weather {
    pub temperature: f64,
    pub humidity: f64,
    pub wind: f64,
    pub code: u32,
    pub timestamp: u64,
    pub fetched_at: u64,
}
#[derive(Clone, Default, Serialize)]
pub struct Snapshot {
    pub weather: Option<Weather>,
    pub tickers: BTreeMap<String, Series>,
    pub errors: BTreeMap<String, String>,
    pub server_time: u64,
}
pub struct Dashboard {
    config: Mutex<Config>,
    pub snapshot: Mutex<Snapshot>,
    path: PathBuf,
    pub client: reqwest::Client,
    pub reset: std::sync::atomic::AtomicBool,
    pub fetch_lock: tokio::sync::Mutex<()>,
}
impl Dashboard {
    pub fn new() -> Result<Self> {
        let path = std::env::var_os("EPAPER_CONFIG")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("data/dashboard.json"));
        let config = if path.exists() {
            serde_json::from_slice(&std::fs::read(&path)?)?
        } else {
            Config::default()
        };
        validate(&config)?;
        Ok(Self {
            config: Mutex::new(config),
            snapshot: Default::default(),
            path,
            client: reqwest::Client::builder()
                .user_agent("CrowPanelDesk/2.0")
                .timeout(Duration::from_secs(15))
                .build()?,
            reset: std::sync::atomic::AtomicBool::new(false),
            fetch_lock: Default::default(),
        })
    }
    pub fn config(&self) -> Config {
        self.config.lock().unwrap().clone()
    }
    pub fn snapshot(&self) -> Snapshot {
        let mut s = self.snapshot.lock().unwrap().clone();
        s.server_time = now();
        s
    }
    pub async fn save(&self, c: Config) -> Result<()> {
        validate(&c)?;
        // Serialize config writes and feed refreshes, avoiding stale-location writes.
        let _guard = self.fetch_lock.lock().await;
        if let Some(p) = self.path.parent() {
            if !p.as_os_str().is_empty() {
                tokio::fs::create_dir_all(p).await?;
            }
        }
        let tmp = self.path.with_extension("json.tmp");
        tokio::fs::write(&tmp, serde_json::to_vec_pretty(&c)?).await?;
        tokio::fs::rename(tmp, &self.path).await?;
        let old = self.config();
        if c.latitude != old.latitude || c.longitude != old.longitude {
            self.snapshot.lock().unwrap().weather = None;
        }
        *self.config.lock().unwrap() = c;
        self.reset.store(true, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }
    pub fn ingest(&self, q: Quote) -> Result<()> {
        ensure!(
            !q.symbol.is_empty()
                && q.symbol.len() <= 24
                && q.symbol
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-./_".contains(&b)),
            "Invalid symbol"
        );
        ensure!(
            !q.source.is_empty() && q.source.chars().count() <= 48,
            "Source must contain 1–48 characters"
        );
        ensure!(
            q.price.is_finite() && q.price > 0. && q.price < 1e15,
            "Price must be finite and positive"
        );
        ensure!(
            q.timestamp > 0 && q.timestamp <= now() + 60,
            "Timestamp must be a Unix timestamp in seconds, not in the future"
        );
        let mut s = self.snapshot.lock().unwrap();
        let key = format!("{}:{}", q.source, q.symbol);
        ensure!(
            s.tickers.contains_key(&key) || s.tickers.len() < 12,
            "At most 12 ticker/source pairs"
        );
        if let Some(series) = s.tickers.get_mut(&key) {
            ensure!(
                q.timestamp >= series.latest.timestamp,
                "Out-of-order quote rejected"
            );
            if q.timestamp == series.latest.timestamp {
                ensure!(
                    q.price == series.latest.price,
                    "Conflicting quote at same timestamp"
                );
                return Ok(());
            }
            series.samples.push((q.timestamp, q.price));
            if series.samples.len() > 120 {
                series.samples.remove(0);
            }
            series.latest = q;
        } else {
            s.tickers.insert(
                key,
                Series {
                    samples: vec![(q.timestamp, q.price)],
                    latest: q,
                },
            );
        }
        Ok(())
    }
    pub fn frame(&self) -> Result<Vec<u8>> {
        let c = self.config();
        let s = self.snapshot();
        let mut im = GrayImage::from_pixel(792, 272, Luma([255]));
        let f = render::font()?;
        render::text_fit(&mut im, &f, 16, 8, 22., 265, &c.location);
        if let Some(w) = s.weather {
            let stale = now().saturating_sub(w.timestamp) > 1800;
            render::text(&mut im, &f, 16, 42, 52., &format!("{:.0} F", w.temperature));
            render::text(
                &mut im,
                &f,
                16,
                105,
                22.,
                &format!("Humidity {:.0}%", w.humidity),
            );
            render::text(
                &mut im,
                &f,
                16,
                135,
                20.,
                &format!("Wind {:.0} mph", w.wind),
            );
            render::text(&mut im, &f, 16, 165, 18., conditions(w.code));
            render::text(
                &mut im,
                &f,
                16,
                194,
                15.,
                &format!(
                    "{} {} UTC",
                    if stale { "STALE" } else { "As of" },
                    stamp(w.timestamp)
                ),
            );
        } else {
            render::text(&mut im, &f, 16, 70, 22., "Weather unavailable");
            render::text(&mut im, &f, 16, 104, 16., "Enable weather in settings");
        }
        imageproc::drawing::draw_line_segment_mut(&mut im, (294., 10.), (294., 224.), Luma([0]));
        let series = s.tickers.values().next();
        if let Some(series) = series {
            let q = &series.latest;
            let stale = now().saturating_sub(q.timestamp) > 120;
            render::text(
                &mut im,
                &f,
                310,
                10,
                22.,
                &format!("{}  {:.2}", q.symbol, q.price),
            );
            render::text(
                &mut im,
                &f,
                310,
                38,
                13.,
                &format!(
                    "{} | {} UTC {}",
                    q.source.chars().take(20).collect::<String>(),
                    stamp(q.timestamp),
                    if stale { "STALE" } else { "" }
                ),
            );
            let values: Vec<f64> = series.samples.iter().map(|p| p.1).collect();
            if values.len() > 1 {
                render::plot(&mut im, &values, 315., 68., 453., 83.);
            } else {
                render::text(&mut im, &f, 315, 92, 16., "Collecting chart samples…");
            }
            render::text(
                &mut im,
                &f,
                315,
                160,
                12.,
                &format!(
                    "{} samples · {} to {} UTC",
                    values.len(),
                    stamp(series.samples[0].0),
                    stamp(q.timestamp)
                ),
            );
            for (i, t) in s.tickers.values().skip(1).take(2).enumerate() {
                render::text(
                    &mut im,
                    &f,
                    310,
                    185 + (i as i32 * 21),
                    16.,
                    &format!(
                        "{}  {:.2}  {} {}",
                        t.latest.symbol,
                        t.latest.price,
                        t.latest.source,
                        if now().saturating_sub(t.latest.timestamp) > 120 {
                            "STALE"
                        } else {
                            ""
                        }
                    ),
                );
            }
        } else {
            render::text(&mut im, &f, 315, 65, 24., "No ticker data yet");
            render::text(
                &mut im,
                &f,
                315,
                102,
                16.,
                "Enable Coinbase or POST /api/quotes",
            );
        }
        render::text(
            &mut im,
            &f,
            16,
            241,
            13.,
            &format!(
                "Open-Meteo (model) | {} UTC | {}",
                stamp(now()),
                if s.errors.is_empty() {
                    ""
                } else {
                    "FEED ERROR — see web app"
                }
            ),
        );
        render::png(im)
    }
}
fn stamp(ts: u64) -> String {
    chrono::DateTime::from_timestamp(ts as i64, 0)
        .map(|d| d.format("%m/%d %H:%M").to_string())
        .unwrap_or_else(|| "unknown".into())
}
fn conditions(c: u32) -> &'static str {
    match c {
        0 => "Clear",
        1..=3 => "Cloudy",
        45 | 48 => "Fog",
        51..=67 => "Rain / drizzle",
        71..=77 => "Snow",
        80..=86 => "Showers",
        95..=99 => "Thunderstorms",
        _ => "Conditions unknown",
    }
}
fn validate(c: &Config) -> Result<()> {
    ensure!(
        !c.location.trim().is_empty() && c.location.chars().count() <= 28,
        "Location label must have 1–28 characters"
    );
    ensure!(
        c.latitude.is_finite()
            && c.latitude.abs() <= 90.
            && c.longitude.is_finite()
            && c.longitude.abs() <= 180.,
        "Invalid coordinates"
    );
    ensure!(
        (120..=3600).contains(&c.display_interval_seconds),
        "Display refresh must be 120–3600 seconds"
    );
    ensure!(
        c.coinbase_products.len() <= 6
            && c.coinbase_products.iter().all(|p| !p.is_empty()
                && p.len() <= 20
                && p.bytes()
                    .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'-')),
        "Use up to six Coinbase product IDs, e.g. BTC-USD"
    );
    Ok(())
}
async fn json(d: &Dashboard, url: String) -> Result<serde_json::Value> {
    let response = d.client.get(url).send().await?.error_for_status()?;
    ensure!(
        response.content_length().unwrap_or(0) < 1024 * 1024,
        "Feed response too large"
    );
    Ok(response.json().await?)
}
async fn weather(d: &Dashboard, c: &Config) -> Result<Weather> {
    let v=json(d,format!("https://api.open-meteo.com/v1/forecast?latitude={}&longitude={}&current=temperature_2m,relative_humidity_2m,wind_speed_10m,weather_code&temperature_unit=fahrenheit&wind_speed_unit=mph&timeformat=unixtime",c.latitude,c.longitude)).await?;
    let v = &v["current"];
    let w = Weather {
        temperature: v["temperature_2m"]
            .as_f64()
            .context("Weather temperature missing")?,
        humidity: v["relative_humidity_2m"]
            .as_f64()
            .context("Humidity missing")?,
        wind: v["wind_speed_10m"].as_f64().context("Wind missing")?,
        code: v["weather_code"].as_u64().context("Weather code missing")? as u32,
        timestamp: v["time"].as_u64().context("Weather time missing")?,
        fetched_at: now(),
    };
    ensure!(
        w.temperature.is_finite()
            && w.humidity >= 0.
            && w.humidity <= 100.
            && w.wind >= 0.
            && w.timestamp <= now() + 60,
        "Invalid weather data"
    );
    Ok(w)
}
async fn coinbase(d: &Dashboard, product: &str) -> Result<()> {
    let v = json(
        d,
        format!("https://api.exchange.coinbase.com/products/{product}/ticker"),
    )
    .await?;
    let timestamp =
        chrono::DateTime::parse_from_rfc3339(v["time"].as_str().context("Ticker time missing")?)?
            .timestamp();
    ensure!(timestamp > 0, "Invalid ticker timestamp");
    d.ingest(Quote {
        symbol: product.into(),
        price: v["price"].as_str().context("Price missing")?.parse()?,
        source: "Coinbase".into(),
        timestamp: timestamp as u64,
    })
}
fn record(d: &Dashboard, key: &str, result: Result<()>) {
    let mut s = d.snapshot.lock().unwrap();
    match result {
        Ok(()) => {
            s.errors.remove(key);
        }
        Err(e) => {
            s.errors.insert(key.into(), format!("{e:#}"));
        }
    }
}
pub async fn poller(app: App) {
    let mut weather_at = 0;
    let mut quotes_at = 0;
    let mut display_at = 0;
    loop {
        if app
            .dashboard
            .reset
            .swap(false, std::sync::atomic::Ordering::Relaxed)
        {
            weather_at = 0;
            quotes_at = 0;
        }
        let t = now();
        let c;
        {
            let _guard = app.dashboard.fetch_lock.lock().await;
            c = app.dashboard.config();
            if c.weather_enabled && t.saturating_sub(weather_at) >= 600 {
                weather_at = t;
                let result = weather(&app.dashboard, &c)
                    .await
                    .map(|w| app.dashboard.snapshot.lock().unwrap().weather = Some(w));
                record(&app.dashboard, "weather", result);
            }
            if t.saturating_sub(quotes_at) >= 60 {
                quotes_at = t;
                for product in &c.coinbase_products {
                    let result = coinbase(&app.dashboard, product).await;
                    record(&app.dashboard, product, result);
                }
            }
        }
        if c.auto_display && t.saturating_sub(display_at) >= c.display_interval_seconds {
            // Skip while any delivery is pending; avoid piling up scheduled frames during an outage.
            let pending = app
                .jobs
                .lock()
                .unwrap()
                .iter()
                .any(|j| j.status == "queued" || j.status == "sending");
            if !pending {
                display_at = t;
                match app.dashboard.frame() {
                    Ok(png) => {
                        let r = queue_payload(
                            &app,
                            Payload::Image(png),
                            "Scheduled dashboard".into(),
                            0,
                        )
                        .map(|_| ())
                        .map_err(|e| anyhow::anyhow!("{}", e.1 .0));
                        record(&app.dashboard, "dashboard", r);
                    }
                    Err(e) => record(&app.dashboard, "dashboard", Err(e)),
                }
            }
        }
        tokio::time::sleep(Duration::from_secs(10)).await;
    }
}
