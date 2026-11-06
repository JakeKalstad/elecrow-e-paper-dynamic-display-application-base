use anyhow::{bail, ensure, Context, Result};
use clap::{Parser, Subcommand};
use image::{GrayImage, Luma};
use std::{
    io::{Read, Write},
    net::{TcpStream, ToSocketAddrs},
    path::PathBuf,
    time::{Duration, Instant},
};
#[derive(Parser)]
struct Args {
    #[arg(
        long,
        default_value = "/dev/serial/by-id/usb-1a86_USB_Serial-if00-port0"
    )]
    port: String,
    /// Device Wi-Fi address, e.g. 192.168.1.50:7878 (takes precedence over USB).
    #[arg(long)]
    host: Option<String>,
    #[arg(long)]
    preview: Option<PathBuf>,
    #[arg(long)]
    dry_run: bool,
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    Text {
        text: String,
        #[arg(long, default_value_t = 40.0)]
        size: f32,
        #[arg(
            long,
            default_value = "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf"
        )]
        font: PathBuf,
    },
    Image {
        path: PathBuf,
    },
    Test,
    Clear,
}
fn pack(im: &GrayImage) -> Vec<u8> {
    let mut frame = vec![255; 27200];
    for (x, y, v) in im.enumerate_pixels() {
        let px = x as usize + if x >= 396 { 8 } else { 0 };
        if v[0] < 128 {
            frame[y as usize * 100 + px / 8] &= !(0x80 >> (px % 8));
        }
    }
    frame
}
fn line<T: Read + ?Sized>(port: &mut T) -> Result<String> {
    let start = Instant::now();
    let mut s = Vec::new();
    loop {
        ensure!(
            start.elapsed() < Duration::from_secs(90),
            "board response timeout"
        );
        let mut b = [0];
        match port.read(&mut b) {
            Ok(1) if b[0] == b'\n' => return Ok(String::from_utf8_lossy(&s).trim().into()),
            Ok(1) => {
                if s.len() < 256 {
                    s.push(b[0]);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(e) => return Err(e.into()),
            Ok(0) => bail!("Device closed connection"),
            _ => {}
        }
    }
}
fn expect<T: Read + ?Sized>(port: &mut T, wanted: &str) -> Result<()> {
    for _ in 0..20 {
        let got = line(port)?;
        if got == wanted {
            return Ok(());
        }
        if got.starts_with("ERR") {
            bail!("{got}");
        }
        eprintln!("board: {got}");
    }
    bail!("Expected {wanted}")
}
fn transmit<T: Read + Write + ?Sized>(port: &mut T, frame: &[u8]) -> Result<()> {
    port.write_all(b"EPD1")?;
    port.write_all(&(frame.len() as u32).to_le_bytes())?;
    port.write_all(&crc32fast::hash(frame).to_le_bytes())?;
    expect(port, "READY")?;
    for chunk in frame.chunks(512) {
        port.write_all(chunk)?;
        expect(port, "CHUNK")?;
    }
    println!("Uploaded. Waiting for e-paper refresh…");
    expect(port, "DONE")?;
    println!("Display updated.");
    Ok(())
}
fn main() -> Result<()> {
    let a = Args::parse();
    let mut im = GrayImage::from_pixel(792, 272, Luma([255]));
    match a.command {
        Command::Text { text, size, font } => {
            ensure!(
                size.is_finite() && size >= 8.0 && size <= 240.0,
                "size must be 8..240"
            );
            let f = ab_glyph::FontArc::try_from_vec(
                std::fs::read(&font).with_context(|| format!("Reading font {}", font.display()))?,
            )?;
            for (i, line) in text.lines().enumerate() {
                imageproc::drawing::draw_text_mut(
                    &mut im,
                    Luma([0]),
                    20,
                    20 + (i as f32 * (size + 8.0)) as i32,
                    size,
                    &f,
                    line,
                );
            }
        }
        Command::Image { path } => {
            let resized = image::open(path)?
                .resize(792, 272, image::imageops::FilterType::Lanczos3)
                .to_luma8();
            let x = (792 - resized.width()) / 2;
            let y = (272 - resized.height()) / 2;
            image::imageops::overlay(&mut im, &resized, x as i64, y as i64);
            image::imageops::dither(&mut im, &image::imageops::BiLevel);
        }
        Command::Test => {
            for (x, y, v) in im.enumerate_pixels_mut() {
                if x < 3 || x >= 789 || y < 3 || y >= 269 || ((x / 40 + y / 40) % 2 == 0 && y > 100)
                {
                    *v = Luma([0]);
                }
            }
            // Both edges of the dual-controller seam should be visible.
            for y in 10..90 {
                im.put_pixel(395, y, Luma([0]));
                im.put_pixel(396, y, Luma([0]));
            }
        }
        Command::Clear => {}
    }
    if let Some(path) = a.preview {
        im.save(path)?;
    }
    let frame = pack(&im);
    if a.dry_run {
        println!(
            "Rendered {} bytes; no serial connection opened",
            frame.len()
        );
        return Ok(());
    }
    if let Some(host) = a.host {
        let key = std::env::var("EPAPER_DEVICE_KEY")
            .context("Set EPAPER_DEVICE_KEY for Wi-Fi delivery")?;
        ensure!(
            key.len() >= 16 && key.len() <= 128 && key.bytes().all(|b| b.is_ascii_alphanumeric()),
            "Device key must be 16–128 ASCII letters/digits"
        );
        let addr = host
            .to_socket_addrs()?
            .next()
            .context("No device address")?;
        let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(10))?;
        stream.set_read_timeout(Some(Duration::from_secs(90)))?;
        stream.set_write_timeout(Some(Duration::from_secs(15)))?;
        stream.write_all(format!("AUTH {key}\n").as_bytes())?;
        expect(&mut stream, "AUTH_OK")?;
        transmit(&mut stream, &frame)?;
    } else {
        let mut port = serialport::new(a.port, 115200)
            .timeout(Duration::from_secs(1))
            .open()?;
        port.write_data_terminal_ready(false)?;
        port.write_request_to_send(false)?;
        std::thread::sleep(Duration::from_secs(2));
        port.clear(serialport::ClearBuffer::Input)?;
        transmit(&mut *port, &frame)?;
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn seam_padding() {
        let mut im = GrayImage::from_pixel(792, 272, Luma([255]));
        im.put_pixel(395, 0, Luma([0]));
        im.put_pixel(396, 0, Luma([0]));
        im.put_pixel(791, 271, Luma([0]));
        let f = pack(&im);
        assert_eq!(f.len(), 27200);
        assert_eq!(f[49], 0xef);
        assert_eq!(f[50], 0xf7);
        assert_eq!(f[27199], 0xfe);
    }
}
