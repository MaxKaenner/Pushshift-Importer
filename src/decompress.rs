use std::fs::File;
use std::io::prelude::*;
use std::{collections::VecDeque, path::Path};
use std::{fs, sync::mpsc::Receiver};
use std::{io::BufReader, sync::mpsc::SyncSender};

use anyhow::{anyhow, Context, Result};
use bzip2::read::BzDecoder;
use flate2::read::GzDecoder;
use log::error;
use xz2::read::XzDecoder;

pub fn iter_lines(filename: &Path) -> Result<Box<dyn Iterator<Item = String>>> {
    let extension = filename
        .extension()
        .and_then(|extension| extension.to_str())
        .ok_or_else(|| anyhow!("cannot the file extension for {}", filename.display()))?;
    let filename_owned = filename.to_path_buf();
    if extension == "gz" {
        let file = File::open(filename)?;
        let gzip_file = BufReader::new(GzDecoder::new(file));
        let iter = gzip_file.lines().map(move |line| {
            line.with_context(|| format!("failed to read file {}", filename_owned.display()))
                .unwrap()
        });
        return Ok(Box::new(iter));
    } else if extension == "bz2" {
        let reader = fs::File::open(filename)?;
        let decoder = BufReader::new(BzDecoder::new(reader));
        let iter = decoder.lines().map(move |line| {
            line.with_context(|| format!("failed to read file {}", filename_owned.display()))
                .unwrap()
        });
        return Ok(Box::new(iter));
    } else if extension == "xz" {
        let reader = fs::File::open(filename)?;
        let decoder = BufReader::new(XzDecoder::new_multi_decoder(reader));
        let iter = decoder.lines().map(move |line| {
            line.with_context(|| format!("failed to read file {}", filename_owned.display()))
                .unwrap()
        });
        return Ok(Box::new(iter));
    } else if extension == "zst" {
        let (tx, rx) = std::sync::mpsc::sync_channel(2);
        let mut reader = fs::File::open(filename)?;
        let mut writer = ChannelWriter {
            current: VecDeque::new(),
            tx,
        };
        std::thread::spawn(move || std::io::copy(&mut reader, &mut writer));
        let reader = ChannelReader {
            rx,
            current: VecDeque::new(),
        };
        let (tx2, rx2) = std::sync::mpsc::sync_channel(2);
        let writer = ChannelWriter {
            tx: tx2,
            current: VecDeque::new(),
        };
        std::thread::spawn(move || {
            if let Err(e) = zstd::stream::copy_decode(reader, writer) {
                error!("error decoding file: {e:?}");
            }
        });
        let reader = ChannelReader {
            rx: rx2,
            current: VecDeque::new(),
        };
        let decoder = BufReader::new(reader);
        let iter = decoder.lines().map(move |line| {
            line.with_context(|| format!("failed to read file {}", filename_owned.display()))
                .unwrap()
        });
        return Ok(Box::new(iter));
    }
    Err(anyhow!(
        "unknown file extension for file {}",
        filename.display()
    ))
}

struct ChannelReader {
    rx: Receiver<VecDeque<u8>>,
    current: VecDeque<u8>,
}

impl Read for ChannelReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if self.current.is_empty() {
            if let Ok(vec) = self.rx.recv() {
                self.current = vec;
            }
        }
        self.current.read(buf)
    }
}

struct ChannelWriter {
    current: VecDeque<u8>,
    tx: SyncSender<VecDeque<u8>>,
}

impl Write for ChannelWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let len = self.current.write(buf)?;
        if self.current.len() >= 1024 * 1024 {
            self.flush()?;
        }
        Ok(len)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.tx.send(std::mem::take(&mut self.current)).ok();
        Ok(())
    }
}
