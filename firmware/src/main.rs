use anyhow::{ensure, Result};
use esp_idf_hal::{
    delay::FreeRtos,
    gpio::*,
    peripherals::Peripherals,
    prelude::*,
    uart::{self, UartDriver},
};
use std::{
    sync::{mpsc, Arc, Mutex},
    time::{Duration, Instant},
};

mod wireless;

const FRAME: usize = 27200; // 800 padded columns, 272 rows; seam at x=396.

struct Panel<'a> {
    clk: PinDriver<'a, Gpio12, Output>,
    mosi: PinDriver<'a, Gpio11, Output>,
    cs: PinDriver<'a, Gpio45, Output>,
    dc: PinDriver<'a, Gpio46, Output>,
    rst: PinDriver<'a, Gpio47, Output>,
    busy: PinDriver<'a, Gpio48, Input>,
    _power: PinDriver<'a, Gpio7, Output>,
}

impl Panel<'_> {
    // Software SPI for initial hardware bring-up.
    fn byte(&mut self, v: u8) -> Result<()> {
        self.cs.set_low()?;

        for bit in (0..8).rev() {
            self.clk.set_low()?;

            if v & (1 << bit) != 0 {
                self.mosi.set_high()?;
            } else {
                self.mosi.set_low()?;
            }

            self.clk.set_high()?;
        }

        self.clk.set_low()?;
        self.cs.set_high()?;

        Ok(())
    }

    fn cmd(&mut self, c: u8, data: &[u8]) -> Result<()> {
        self.dc.set_low()?;
        self.byte(c)?;
        self.dc.set_high()?;

        for &v in data {
            self.byte(v)?;
        }

        Ok(())
    }

    fn idle(&self) -> Result<()> {
        let start = Instant::now();

        while self.busy.is_high() {
            ensure!(
                start.elapsed() < Duration::from_secs(30),
                "display BUSY timeout"
            );

            FreeRtos::delay_ms(10);
        }

        Ok(())
    }

    fn init(&mut self) -> Result<()> {
        self.cs.set_high()?;
        self.clk.set_low()?;
        self.rst.set_high()?;

        FreeRtos::delay_ms(20);
        self.rst.set_low()?;

        FreeRtos::delay_ms(10);
        self.rst.set_high()?;

        FreeRtos::delay_ms(10);
        self.idle()?;

        self.cmd(0x12, &[])?;

        FreeRtos::delay_ms(10);
        self.idle()?;

        self.cmd(0x18, &[0x80])?;

        self.cmd(0x22, &[0xb1])?;
        self.cmd(0x20, &[])?;
        self.idle()?;

        self.cmd(0x1a, &[0x64, 0])?;

        self.cmd(0x22, &[0x91])?;
        self.cmd(0x20, &[])?;
        self.idle()?;

        self.cmd(0x3c, &[3])?;
        self.idle()
    }

    fn window(&mut self, slave: bool) -> Result<()> {
        if slave {
            self.cmd(0x91, &[4])?;
            self.cmd(0xc4, &[49, 0])?;
            self.cmd(0xc5, &[15, 1, 0, 0])?;
            self.cmd(0xce, &[49])?;
            self.cmd(0xcf, &[15, 1])?;
        } else {
            self.cmd(0x11, &[5])?;
            self.cmd(0x44, &[0, 49])?;
            self.cmd(0x45, &[15, 1, 0, 0])?;
            self.cmd(0x4e, &[0])?;
            self.cmd(0x4f, &[15, 1])?;
        }

        Ok(())
    }

    fn display(&mut self, frame: &[u8]) -> Result<()> {
        ensure!(frame.len() == FRAME, "invalid frame");

        self.init()?;

        for slave in [false, true] {
            self.window(slave)?;
            self.cmd(if slave { 0xa4 } else { 0x24 }, &[])?;

            let first = if slave { 50 } else { 0 };

            // Vendor driver transfers columns of bytes, then rows.
            for col in first..first + 50 {
                for row in 0..272 {
                    self.byte(frame[row * 100 + col])?;
                }

                FreeRtos::delay_ms(1);
            }

            self.window(slave)?;
            self.cmd(if slave { 0xa6 } else { 0x26 }, &[])?;

            for _ in 0..50 {
                for _ in 0..272 {
                    self.byte(0)?;
                }

                FreeRtos::delay_ms(1);
            }
        }

        self.cmd(0x22, &[0xf7])?;
        self.cmd(0x20, &[])?;

        FreeRtos::delay_ms(100);
        self.idle()?;

        self.cmd(0x10, &[1])?;
        FreeRtos::delay_ms(5);

        Ok(())
    }
}

fn send(u: &UartDriver<'_>, data: &[u8]) -> Result<()> {
    let mut n = 0;

    while n < data.len() {
        let k = u.write(&data[n..])?;
        ensure!(k > 0, "UART write stalled");
        n += k;
    }

    Ok(())
}

fn read_some(u: &UartDriver<'_>, out: &mut [u8]) -> Result<usize> {
    match u.read(out, 10) {
        Ok(n) => Ok(n),
        Err(e) if e.code() == esp_idf_hal::sys::ESP_ERR_TIMEOUT => Ok(0),
        Err(e) => Err(e.into()),
    }
}

fn read_exact(u: &UartDriver<'_>, out: &mut [u8]) -> Result<()> {
    let mut n = 0;
    let start = Instant::now();

    while n < out.len() {
        ensure!(start.elapsed() < Duration::from_secs(15), "upload timeout");
        n += read_some(u, &mut out[n..])?;
    }

    Ok(())
}

fn receive(
    u: &UartDriver<'_>,
    panel: &Arc<Mutex<Panel<'static>>>,
    frame: &mut [u8],
    info: &mpsc::Receiver<String>,
) -> Result<()> {
    let mut matched = 0;

    loop {
        let mut b = [0];

        if read_some(u, &mut b)? == 0 {
            if let Ok(message) = info.try_recv() {
                let _ = send(u, message.as_bytes());
            }

            continue;
        }

        if b[0] == b"EPD1"[matched] {
            matched += 1;
        } else {
            matched = usize::from(b[0] == b'E');
        }

        if matched == 4 {
            break;
        }
    }

    let mut header = [0; 8];
    read_exact(u, &mut header)?;

    let len = u32::from_le_bytes(header[..4].try_into()?) as usize;
    let crc = u32::from_le_bytes(header[4..].try_into()?);

    ensure!(len == FRAME, "expected 27200 bytes");

    send(u, b"READY\n")?;

    for chunk in frame.chunks_mut(512) {
        read_exact(u, chunk)?;
        send(u, b"CHUNK\n")?;
    }

    ensure!(crc32fast::hash(frame) == crc, "CRC mismatch");

    panel
        .lock()
        .map_err(|_| anyhow::anyhow!("display lock failed"))?
        .display(frame)?;

    send(u, b"DONE\n")?;

    Ok(())
}

fn main() -> Result<()> {
    esp_idf_hal::sys::link_patches();

    let p = Peripherals::take()?;

    let mut power = PinDriver::output(p.pins.gpio7)?;
    power.set_high()?;

    let panel = Arc::new(Mutex::new(Panel {
        clk: PinDriver::output(p.pins.gpio12)?,
        mosi: PinDriver::output(p.pins.gpio11)?,
        cs: PinDriver::output(p.pins.gpio45)?,
        dc: PinDriver::output(p.pins.gpio46)?,
        rst: PinDriver::output(p.pins.gpio47)?,
        busy: PinDriver::input(p.pins.gpio48)?,
        _power: power,
    }));

    let config = uart::config::Config::new()
        .baudrate(Hertz(115200))
        .rx_fifo_size(2048);

    let u = UartDriver::new(
        p.uart0,
        p.pins.gpio43,
        p.pins.gpio44,
        Option::<Gpio0>::None,
        Option::<Gpio1>::None,
        &config,
    )?;

    let (info_tx, info_rx) = mpsc::channel::<String>();

    wireless::start(p.modem, panel.clone(), info_tx)?;

    let mut frame = vec![255; FRAME];

    send(&u, b"BOOT crowpanel-rust\n")?;

    loop {
        if let Err(e) = receive(&u, &panel, &mut frame, &info_rx) {
            let _ = send(&u, format!("ERR {e}\n").as_bytes());

            // Drain a failed transfer before accepting another frame.
            let mut discard = [0; 512];

            while read_some(&u, &mut discard).unwrap_or(0) > 0 {}
        }
    }
}