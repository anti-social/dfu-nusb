use anyhow::{Context, Result};
use dfu_nusb::*;
use std::path::PathBuf;

#[derive(clap::Parser)]
pub struct Cli {
    /// Path to the output file to write the firmware dump.
    path: PathBuf,

    /// Wait for the device to appear.
    #[clap(short, long)]
    wait: bool,

    /// Specify Vendor/Product ID(s) of DFU device.
    #[clap(
        long,
        short,
        parse(try_from_str = Self::parse_vid_pid), name = "vendor>:<product",
    )]
    device: (u16, u16),

    /// Specify the DFU Interface number.
    #[clap(long, short, default_value = "0")]
    intf: u8,

    /// Specify the Altsetting of the DFU Interface by number.
    #[clap(long, short, default_value = "0")]
    alt: u8,

    /// Enable verbose logs.
    #[clap(long, short)]
    verbose: bool,

    /// Override start address (e.g. 0x0800C000).
    #[clap(long, short, value_parser=Self::parse_address, name="address")]
    override_address: Option<u32>,

    /// Number of bytes to read. Reads the full memory region by default.
    #[clap(long, short)]
    length: Option<u32>,
}

impl Cli {
    pub fn run(self) -> Result<()> {
        let Cli {
            path,
            wait,
            device,
            intf,
            alt,
            verbose,
            override_address,
            length,
        } = self;
        let log_level = if verbose {
            simplelog::LevelFilter::Trace
        } else {
            simplelog::LevelFilter::Info
        };
        simplelog::SimpleLogger::init(log_level, Default::default())?;
        let (vid, pid) = device;

        let mut file = std::fs::File::create(&path)
            .with_context(|| format!("could not create output file: {}", path.display()))?;

        let mut device: Dfu = match DfuNusb::open(vid, pid, intf, alt) {
            Err(Error::CouldNotOpenDevice) if wait => {
                let bar = indicatif::ProgressBar::new_spinner();
                bar.set_message("Waiting for device");

                loop {
                    std::thread::sleep(std::time::Duration::from_millis(250));
                    match DfuNusb::open(vid, pid, intf, alt) {
                        Err(Error::CouldNotOpenDevice) => bar.tick(),
                        r => {
                            bar.finish();
                            break r;
                        }
                    }
                }
            }
            r => r,
        }
        .context("could not open device")?;

        if let Some(address) = override_address {
            device.override_address(address);
        }

        let bar = indicatif::ProgressBar::new_spinner();
        bar.set_style(
            indicatif::ProgressStyle::default_spinner().template(
                "{spinner:.green} [{elapsed_precise}] {bytes} ({bytes_per_sec}) {msg:10}",
            ),
        );

        device.with_progress({
            let bar = bar.clone();
            move |count| {
                bar.inc(count as u64);
            }
        });

        match length {
            Some(len) => device.upload(&mut file, len),
            None => device.upload_all(&mut file),
        }
        .context("could not read firmware from the device")?;

        bar.finish();
        println!("Firmware saved to {}", path.display());

        Ok(())
    }

    pub fn parse_vid_pid(s: &str) -> Result<(u16, u16)> {
        let (vid, pid) = s
            .split_once(':')
            .context("could not parse VID/PID (missing `:')")?;
        let vid = u16::from_str_radix(vid, 16).context("could not parse VID")?;
        let pid = u16::from_str_radix(pid, 16).context("could not parse PID")?;
        Ok((vid, pid))
    }

    pub fn parse_address(s: &str) -> Result<u32> {
        if s.to_ascii_lowercase().starts_with("0x") {
            u32::from_str_radix(&s[2..], 16).context("could not parse override address")
        } else {
            s.parse().context("could not parse override address")
        }
    }
}

fn main() -> Result<()> {
    <Cli as clap::Parser>::from_args().run()
}
