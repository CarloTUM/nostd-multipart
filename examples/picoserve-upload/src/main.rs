//! OTA-style upload with [picoserve]: a plain HTML `<input type="file">` form, no
//! JavaScript. The handler streams the request body through
//! `MultipartReaderAsync` with a 1 KiB buffer and hands the part named
//! `firmware` to a "flash" sink chunk by chunk, so the image never has to fit in
//! RAM. Here the sink only counts bytes and computes a CRC-32; on a board it would
//! be the flash driver.
//!
//! The handler code is the same on embassy; only `main` (tokio sockets) differs.
//!
//! Run with `cargo run --release` and open <http://localhost:8000>.
//!
//! [picoserve]: https://crates.io/crates/picoserve

use std::time::Duration;

use nostd_multipart::{content_disposition, Event, MultipartReaderAsync, Parser, ReadError};
use picoserve::io::Read;
use picoserve::request::Request;
use picoserve::response::{IntoResponse, ResponseWriter, StatusCode};
use picoserve::routing::{get_service, RequestHandlerService};
use picoserve::ResponseSent;

const FORM: &str = r#"<!doctype html>
<meta charset="utf-8">
<title>picoserve + nostd-multipart</title>
<form method="post" enctype="multipart/form-data">
  <p><input name="version" placeholder="version" value="1.0.0">
  <p><input type="file" name="firmware">
  <p><button>Upload</button>
</form>
"#;

/// Stand-in for a flash driver: counts bytes and keeps a CRC-32 (IEEE).
struct FlashSink {
    len: u32,
    crc: u32,
}

impl FlashSink {
    fn new() -> Self {
        FlashSink { len: 0, crc: !0 }
    }

    fn write(&mut self, data: &[u8]) {
        self.len += data.len() as u32;
        for &b in data {
            self.crc ^= u32::from(b);
            for _ in 0..8 {
                self.crc = (self.crc >> 1) ^ (0xEDB8_8320 & (self.crc & 1).wrapping_neg());
            }
        }
    }
}

/// Why an upload was rejected.
enum UploadError<E> {
    NotMultipart,
    NoFirmware,
    Read(ReadError<E>),
}

impl<E> From<ReadError<E>> for UploadError<E> {
    fn from(e: ReadError<E>) -> Self {
        UploadError::Read(e)
    }
}

/// Streams the body, writing the `firmware` part to `flash`.
async fn receive<R: Read>(
    content_type: &[u8],
    body: R,
    flash: &mut FlashSink,
) -> Result<(), UploadError<R::Error>> {
    let parser: Parser<'_, 256, 8> =
        Parser::from_content_type(content_type).map_err(|_| UploadError::NotMultipart)?;
    let mut reader = MultipartReaderAsync::<_, 1024, 256, 8>::new(body, parser);
    let mut in_firmware = false;
    let mut found = false;
    loop {
        match reader.next_event().await? {
            Event::PartStart => in_firmware = false,
            Event::Header { name, value } if name.eq_ignore_ascii_case(b"content-disposition") => {
                if let Ok(cd) = content_disposition(value) {
                    in_firmware = cd.name().is_some_and(|n| n == "firmware");
                    found |= in_firmware;
                }
            }
            Event::Data(d) if in_firmware => flash.write(d),
            Event::Finished => {
                return if found {
                    Ok(())
                } else {
                    Err(UploadError::NoFirmware)
                }
            }
            _ => {}
        }
    }
}

struct Upload;

impl RequestHandlerService<()> for Upload {
    async fn call_request_handler_service<R: Read, W: ResponseWriter<Error = R::Error>>(
        &self,
        (): &(),
        (): (),
        mut request: Request<'_, R>,
        response_writer: W,
    ) -> Result<ResponseSent, W::Error> {
        // The parser borrows the boundary, so copy the header out of the request.
        let mut content_type = [0u8; 128];
        let ct_len = match request.parts.headers().get("content-type") {
            Some(v) if v.as_raw().len() <= content_type.len() => {
                content_type[..v.as_raw().len()].copy_from_slice(v.as_raw());
                v.as_raw().len()
            }
            _ => 0,
        };

        let mut flash = FlashSink::new();
        let reader = request
            .body_connection
            .body()
            .reader()
            // Large images take longer than the default read timeout. On embassy use
            // `.with_different_timeout(..)` instead.
            .with_different_timeout_signal(Box::pin(tokio::time::sleep(Duration::from_secs(60))));
        let result = receive(&content_type[..ct_len], reader, &mut flash).await;
        let connection = request.body_connection.finalize().await?;

        match result {
            Ok(()) => {
                let crc = !flash.crc;
                println!("firmware: {} bytes, crc32 {crc:08x}", flash.len);
                format_args!("firmware: {} bytes, crc32 {crc:08x}\r\n", flash.len)
                    .write_to(connection, response_writer)
                    .await
            }
            Err(UploadError::Read(ReadError::Io(e))) => Err(e),
            Err(e) => {
                let msg = match e {
                    UploadError::NotMultipart => "expected multipart/form-data\r\n",
                    UploadError::NoFirmware => "no part named \"firmware\"\r\n",
                    _ => "malformed multipart body\r\n",
                };
                (StatusCode::BAD_REQUEST, msg)
                    .write_to(connection, response_writer)
                    .await
            }
        }
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::io::Result<()> {
    let port = 8000;
    let app = std::rc::Rc::new(picoserve::Router::new().route(
        "/",
        get_service(picoserve::response::File::html(FORM)).post_service(Upload),
    ));

    let socket = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).await?;
    println!("http://localhost:{port}/");

    tokio::task::LocalSet::new()
        .run_until(async {
            loop {
                let (stream, remote) = socket.accept().await?;
                let app = app.clone();
                tokio::task::spawn_local(async move {
                    static CONFIG: picoserve::Config = picoserve::Config::const_default();
                    if let Err(e) = picoserve::Server::new_tokio(&app, &CONFIG, &mut [0; 2048])
                        .serve(stream)
                        .await
                    {
                        eprintln!("{remote}: {e}");
                    }
                });
            }
        })
        .await
}
