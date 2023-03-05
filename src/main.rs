#![feature(option_result_contains)]
#![feature(string_leak)]

use std::collections::HashMap;
use std::fs::read_dir;
use std::mem::forget;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, self};
use fern::Dispatch;
use image::{DynamicImage};
use image::codecs::jpeg::JpegDecoder;
use log::error;
use parking_lot::{RwLock};
use std::sync::mpsc::{channel};
use std::io::Read;


#[tokio::main]
async fn main() -> anyhow::Result<()> {
    Dispatch::new()
        .format(|out, message, record| {
            out.finish(format_args!(
                "[{}][{}:{}] {}",
                record.level(),
                record.target(),
                record
                    .line()
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or("?".into()),
                message
            ))
        })
        .chain(
            Dispatch::new()
                .chain(std::io::stderr())
        )
        .apply()?;

    let cache: Arc<RwLock<HashMap<Box<str>, Box<[u8]>>>> = Default::default();
    let (ready_sender, ready_receiver) = channel::<()>();
    let start_time = Instant::now();

    for res in read_dir(".").context("Reading current directory")? {
        let file = res.context("Listing entry")?;
        let path = file.path();

        match path.extension().map(|x| x.to_str()).flatten() {
            Some("jpg" | "jpeg") => {}
            _ => continue
        }

        let cache = cache.clone();
        let ready_sender = ready_sender.clone();

        rayon::spawn(move || {
            let mut file = match std::fs::File::open(&path) {
                Ok(x) => x,
                Err(e) => {
                    error!("Couldn't read {path:?}: {e:?}");
                    return
                }
            };

            let mut data = Vec::new();
            match file.read_to_end(&mut data) {
                Ok(_) => {}
                Err(e) => {
                    error!("Couldn't read {path:?}: {e:?}");
                    return
                }
            }

            let _ready_sender = ready_sender;
            let res = match JpegDecoder::new(data.as_slice()) {
                Ok(mut x) => {
                    x.scale(900, 600).expect(&format!("scaling of {path:?} to work"));
                    DynamicImage::from_decoder(x)
                }
                Err(e) => {
                    error!("Couldn't read {path:?}: {e:?}");
                    return
                }
            };

            let image = match res {
                Ok(x) => x,
                Err(e) => {
                    error!("Couldn't read {path:?}: {e:?}");
                    return
                }
            };

            drop(data);
            let image = match webp::Encoder::from_image(&image) {
                Ok(x) => unsafe {
                    let mut slice = x.encode(0.0);
                    let boxed = Box::from_raw(slice.as_mut());
                    forget(slice);
                    boxed
                },
                Err(e) => {
                    error!("Couldn't encode to webp for {path:?}: {e}");
                    return
                }
            };

            let key = unsafe {
                let fname = path
                    .file_name()
                    .unwrap();

                Box::from_raw(
                    fname
                        .to_str()
                        .expect(&format!("filename to be valid: {fname:?}"))
                        .to_owned()
                        .leak()
                    )
            };

            cache.write().insert(key, image);
        });
    }

    drop(ready_sender);
    let _ = ready_receiver.recv();
    println!("{:?}", start_time.elapsed());

    Ok(())
}
