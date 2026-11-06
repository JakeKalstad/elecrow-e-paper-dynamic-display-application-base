use super::*;
use esp_idf_svc::{
    eventloop::EspSystemEventLoop,
    hal::modem::Modem,
    nvs::EspDefaultNvsPartition,
    wifi::{AuthMethod, BlockingWifi, ClientConfiguration, Configuration, EspWifi},
};
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
};

pub fn start(
    modem: Modem,
    panel: Arc<Mutex<Panel<'static>>>,
    info: std::sync::mpsc::Sender<String>,
) -> Result<()> {
    let ssid = option_env!("EPAPER_WIFI_SSID").unwrap_or("").to_owned();
    if ssid.is_empty() {
        let _ = info.send("WIFI_DISABLED: no SSID compiled into firmware\n".into());
        return Ok(());
    }
    
    let _ = info.send("WIFI_STARTING\n".into());
    let password = option_env!("EPAPER_WIFI_PASSWORD").unwrap_or("").to_owned();
    let key = option_env!("EPAPER_DEVICE_KEY").unwrap_or("").to_owned();
    ensure!(
        key.len() >= 16 && key.len() <= 128 && key.bytes().all(|b| b.is_ascii_alphanumeric()),
        "Wi-Fi needs a 16–128 character alphanumeric EPAPER_DEVICE_KEY"
    );
    // Wi-Fi setup/retries run independently, so USB remains usable if the AP is unavailable.
    std::thread::Builder::new()
        .name("epaper-wifi".into())
        .stack_size(12288)
        .spawn(move || {
            if let Err(e) = serve(modem, panel, ssid, password, key, info.clone()) {
                let _ = info.send(format!("WIFI_ERROR {e}\n"));
            }
        })?;
    Ok(())
}
fn serve(
    modem: Modem,
    panel: Arc<Mutex<Panel<'static>>>,
    ssid: String,
    password: String,
    key: String,
    info: std::sync::mpsc::Sender<String>,
) -> Result<()> {
    let events = EspSystemEventLoop::take()?;
    let nvs = EspDefaultNvsPartition::take()?;
    let mut wifi = BlockingWifi::wrap(EspWifi::new(modem, events.clone(), Some(nvs))?, events)?;
    wifi.set_configuration(&Configuration::Client(ClientConfiguration {
        ssid: ssid
            .as_str()
            .try_into()
            .map_err(|_| anyhow::anyhow!("SSID too long"))?,
        password: password
            .as_str()
            .try_into()
            .map_err(|_| anyhow::anyhow!("Wi-Fi password too long"))?,
        auth_method: if password.is_empty() {
            AuthMethod::None
        } else {
            AuthMethod::WPA2Personal
        },
        ..Default::default()
    }))?;
    wifi.start()?;
    let _ = info.send("WIFI_STARTED: connecting to access point\n".into());
    
    loop {
        match wifi.connect() {
            Ok(()) => {
                let _ = info.send("WIFI_CONNECT_REQUESTED: waiting for IP\n".into());
    
                match wifi.wait_netif_up() {
                    Ok(()) => break,
                    Err(e) => {
                        let _ = info.send(format!("WIFI_IP_WAIT_ERROR: {e}\n"));
                    }
                }
            }
            Err(e) => {
                let _ = info.send(format!("WIFI_CONNECT_ERROR: {e}\n"));
            }
        }
    
        let _ = wifi.disconnect();
        std::thread::sleep(Duration::from_secs(10));
    }
    let ip = wifi.wifi().sta_netif().get_ip_info()?.ip;
    let mut frame = vec![255; FRAME];
    // Network address is also available through the router's connected-device list.
    let listener = TcpListener::bind("0.0.0.0:7878")?;
    listener.set_nonblocking(true)?;
    let _ = info.send(format!("WIFI {ip}:7878\n"));
    loop {
        if !wifi.is_connected().unwrap_or(false) {
            let _ = wifi.connect();
            let _ = wifi.wait_netif_up();
            std::thread::sleep(Duration::from_secs(5));
            continue;
        }
        match listener.accept() {
            Ok((mut stream, _)) => {
                let result = receive_tcp(&mut stream, &key, &panel, &mut frame);
                if let Err(e) = result {
                    let _ = stream.write_all(format!("ERR {e}\n").as_bytes());
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(100))
            }
            Err(e) => return Err(e.into()),
        }
    }
}
fn receive_tcp(
    s: &mut TcpStream,
    key: &str,
    panel: &Arc<Mutex<Panel<'static>>>,
    frame: &mut [u8],
) -> Result<()> {
    s.set_read_timeout(Some(Duration::from_secs(15)))?;
    s.set_write_timeout(Some(Duration::from_secs(15)))?;
    let start = Instant::now();
    let mut auth = Vec::new();
    loop {
        ensure!(
            start.elapsed() < Duration::from_secs(15),
            "authentication timeout"
        );
        let mut b = [0];
        s.read_exact(&mut b)?;
        if b[0] == b'\n' {
            break;
        }
        ensure!(auth.len() < 140, "authentication too long");
        auth.push(b[0]);
    }
    ensure!(
        auth == format!("AUTH {key}").as_bytes(),
        "authentication failed"
    );
    s.write_all(b"AUTH_OK\n")?;
    let mut header = [0; 12];
    s.read_exact(&mut header)?;
    ensure!(&header[..4] == b"EPD1", "bad frame magic");
    ensure!(
        u32::from_le_bytes(header[4..8].try_into()?) as usize == FRAME,
        "bad frame length"
    );
    let crc = u32::from_le_bytes(header[8..].try_into()?);
    s.write_all(b"READY\n")?;
    let start = Instant::now();
    for chunk in frame.chunks_mut(512) {
        ensure!(start.elapsed() < Duration::from_secs(30), "upload timeout");
        s.read_exact(chunk)?;
        s.write_all(b"CHUNK\n")?;
    }
    ensure!(crc32fast::hash(frame) == crc, "CRC mismatch");
    panel
        .lock()
        .map_err(|_| anyhow::anyhow!("display lock failed"))?
        .display(frame)?;
    s.write_all(b"DONE\n")?;
    Ok(())
}
