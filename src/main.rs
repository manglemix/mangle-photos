#![feature(string_leak)]
#![feature(arc_into_inner)]
#![feature(iterator_try_collect)]
#![feature(path_file_prefix)]

use std::fs::read_dir;
use std::mem::{forget, transmute, MaybeUninit};
use std::net::{Ipv4Addr, SocketAddrV4};
use std::ops::Deref;
use std::sync::mpsc::channel;
use std::time::Instant;

use axum::http::HeaderValue;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Router, Server};
use fern::Dispatch;
use image::codecs::jpeg::JpegDecoder;
use image::DynamicImage;
use log::{error, LevelFilter};
use rayon::spawn;
use std::io::{Cursor, Read, Write};
use tower_http::compression::CompressionLayer;
use zip::write::FileOptions;
use zip::ZipWriter;

#[tokio::main]
async fn main() {
    let start_time = Instant::now();

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
        .level(LevelFilter::Info)
        .chain(Dispatch::new().chain(std::io::stderr()))
        .apply()
        .unwrap();

    let entries = read_dir(".")
        .expect("Reading current directory")
        .filter_map(|res| {
            let entry = res.expect("Error listing entry");
            match entry.path().extension().map(|x| x.to_str()).flatten() {
                Some("jpg" | "jpeg") => Some(entry),
                _ => None,
            }
        })
        .collect::<Vec<_>>();

    let entry_count = entries.len();

    let (image_sender, image_receiver) = channel();

    entries
        .into_iter()
        .zip(vec![image_sender; entry_count])
        .enumerate()
        .for_each(|(i, (file, image_sender))| {
            // Multithreaded image loading and preview generation
            spawn(move || {
                let path = file.path();

                // I read the whole image to memory, even though there is a method in image
                // to do that. For some reason, this is around 10x faster
                let mut file = std::fs::File::open(&path).expect("Loading image");

                let mut full = Vec::new();
                file.read_to_end(&mut full).expect("Reading image");

                let mut decoder =
                    JpegDecoder::new(full.as_slice()).expect("Initializing decoder for image");
                decoder.scale(900, 600).expect("Scaling image");
                let image = DynamicImage::from_decoder(decoder).expect("Decoding image");

                let preview = webp::Encoder::from_image(&image)
                    .expect("Encoding to webp")
                    .encode(35.0);

                // Leak full as it will be used for the duration of the program,
                // but will not be modified, so we don't need the extra data in Vec
                let full: &[u8] = full.leak();

                // Leak the webp
                // Since there isn't a leak method, we manually leak it
                let preview = unsafe {
                    let ptr: &[u8] = transmute(preview.deref());
                    forget(preview);
                    ptr
                };

                let _ = image_sender.send((i, path, preview, full));
            });
        });

    let mut home_page_lines = Vec::with_capacity(entry_count);

    for _ in 0..entry_count {
        home_page_lines.push(MaybeUninit::uninit());
    }

    let mut router = Router::new();
    // In-memory zip of all images
    let mut all_zip = ZipWriter::new(Cursor::new(Vec::new()));

    while let Ok((i, path, preview_image, full_image)) = image_receiver.recv() {
        let fname = path.file_name().unwrap().to_str().unwrap();
        let preview_name = format!(
            "/preview_{}.webp",
            path.file_prefix().unwrap().to_str().unwrap()
        );

        // Register handlers for preview and full images
        router = router
            .route(
                &(format!("/{fname}")),
                get(move || async {
                    let mut resp = full_image.into_response();
                    resp.headers_mut()
                        .insert("Content-Type", HeaderValue::from_static("image/jpeg"));
                    resp
                }),
            )
            .route(
                &preview_name,
                get(move || async {
                    let mut resp = preview_image.into_response();
                    resp.headers_mut()
                        .insert("Content-Type", HeaderValue::from_static("image/webp"));
                    resp
                }),
            );

        // Add image to home page
        home_page_lines
            .get_mut(i)
            .unwrap()
            .write(
                format!("<a href=\"{fname}\"><img src=\"{preview_name}\" style=\"width:900px;height:600px;\"></a><br>")
            );

        // Zip
        all_zip
            .start_file(fname, FileOptions::default())
            .expect("image to zip");

        all_zip.write_all(full_image).expect("image to zip");
    }

    let home_page_body = home_page_lines
        .into_iter()
        .map(|x| unsafe { MaybeUninit::assume_init(x) })
        .collect::<String>();

    // Finalize the zip and leak that data too
    let all_zip: &[u8] = all_zip
        .finish()
        .expect("Zip to succeed")
        .into_inner()
        .leak();

    println!("Image processing completed in {:?}", start_time.elapsed());

    let home_page_doc = format!(
        "<html>
<head>
    <link href =\"https://fonts.googleapis.com\" rel=\"preconnect\">
    <link href=\"https://fonts.googleapis.com/css?family=Open+Sans\" rel=\"stylesheet\">
    <style>
        * {{
            font-family: 'Open Sans';
            color: white;
            text-align: center;
        }}
        body {{
            background-color: #0c0c0c;
        }}
        
        p {{
            line-height: 35px;
            max-width: 800px;
            margin: auto;
        }}
        
        h1 {{
            font-size: 60px;
        }}
        
        h2 {{
            font-size: 40px;
        }}

        a {{
            text-decoration: none;
        }}
    </style>
</head>
<body>
    <a href=\"images.zip\" download><h2>Download All</h2></a>
{home_page_body}
    </body>
</html>"
    );

    let home_page_doc: &str = home_page_doc.leak();

    let router = router
        .route(
            "/",
            // Serve home page
            get(move || async {
                let mut resp = home_page_doc.into_response();
                resp.headers_mut()
                    .insert("Content-Type", HeaderValue::from_static("text/html"));
                resp
            }),
        )
        .route(
            "/images.zip",
            // Serve zip
            get(move || async {
                let mut resp = all_zip.into_response();
                resp.headers_mut()
                    .insert("Content-Type", HeaderValue::from_static("application/zip"));
                resp
            }),
        )
        .layer(CompressionLayer::new());

    // Allow ctrl-c to be gracefully handled
    let fut = async {
        println!("Deployed to all interfaces!");
        if let Err(e) = tokio::signal::ctrl_c().await {
            error!(target: "console_server", "Faced the following error while listening for ctrl_c: {e:?}");
            return;
        }
        println!("Ending...");
    };

    // Get port from command line or default to 80
    let port = std::env::args().last().unwrap().parse().unwrap_or(80);
    println!("Deploying to {port}");

    Server::bind(&std::net::SocketAddr::V4(SocketAddrV4::new(
        Ipv4Addr::new(0, 0, 0, 0),
        port,
    )))
    .serve(router.into_make_service())
    .with_graceful_shutdown(fut)
    .await
    .expect("Running server");
}
