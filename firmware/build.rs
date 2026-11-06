fn main() {
    for name in [
        "EPAPER_WIFI_SSID",
        "EPAPER_WIFI_PASSWORD",
        "EPAPER_DEVICE_KEY",
    ] {
        println!("cargo:rerun-if-env-changed={name}");
    }
    if let Ok(ssid) = std::env::var("EPAPER_WIFI_SSID") {
        if !ssid.is_empty() {
            assert!(
                ssid.len() <= 32,
                "EPAPER_WIFI_SSID must be at most 32 bytes"
            );
            let key = std::env::var("EPAPER_DEVICE_KEY").unwrap_or_default();
            assert!((16..=128).contains(&key.len()) && key.bytes().all(|b|b.is_ascii_alphanumeric()),
                "Set EPAPER_DEVICE_KEY in .env to a generated 16–128 character alphanumeric key before flashing");
        }
    }
    embuild::espidf::sysenv::output();
}
