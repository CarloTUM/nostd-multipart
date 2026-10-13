//! Try the parser from your browser, no board needed.
//!
//! A single-threaded HTTP server that serves a plain HTML upload form and streams
//! every submitted part through a `Parser<'_, 256, 8>` and a 1 KiB buffer, the
//! same budget as on a microcontroller. For each part it reports the size and a
//! CRC-32 of the body; nothing is buffered beyond those 1 KiB, so files of any
//! size work.
//!
//! Run with `cargo run --release --features std --example form_server` and open
//! <http://127.0.0.1:8080>. Pass another address as the first argument.

use std::fmt::Write as _;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};

use nostd_multipart::{content_disposition, Event, ParamValue, Parser};

const FORM: &str = r#"<!doctype html>
<meta charset="utf-8">
<title>nostd-multipart</title>
<form method="post" enctype="multipart/form-data">
  <p><input name="version" placeholder="version" value="1.0.0">
  <p><input type="file" name="firmware">
  <p><button>Upload</button>
</form>
"#;

/// Size and CRC-32 (IEEE) of one part body.
struct Summary {
    name: String,
    filename: Option<String>,
    len: u64,
    crc: u32,
}

/// Bitwise CRC-32, small enough for a bootloader.
fn crc32_update(mut crc: u32, data: &[u8]) -> u32 {
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & (crc & 1).wrapping_neg());
        }
    }
    crc
}

/// A decoded parameter as (lossy) UTF-8.
fn lossy(v: ParamValue<'_>) -> String {
    String::from_utf8_lossy(&v.bytes().collect::<Vec<u8>>()).into_owned()
}

/// The receive loop from the README, with `read` being the request body.
fn receive(content_type: &[u8], mut body: impl Read) -> Result<Vec<Summary>, String> {
    let mut parser: Parser<'_, 256, 8> =
        Parser::from_content_type(content_type).map_err(|e| e.to_string())?;
    let mut buf = [0u8; 1024];
    let (mut start, mut end, mut eof) = (0, 0, false);
    let mut parts: Vec<Summary> = Vec::new();
    loop {
        let (consumed, event) = parser
            .parse(&buf[start..end], eof)
            .map_err(|e| e.to_string())?;
        start += consumed;
        match event {
            Some(Event::PartStart) => parts.push(Summary {
                name: String::new(),
                filename: None,
                len: 0,
                crc: !0,
            }),
            Some(Event::Header { name, value })
                if name.eq_ignore_ascii_case(b"content-disposition") =>
            {
                let cd = content_disposition(value).map_err(|e| e.to_string())?;
                let part = parts.last_mut().ok_or("header outside a part")?;
                part.name = cd.name().map(lossy).unwrap_or_default();
                part.filename = cd.filename().map(lossy);
            }
            Some(Event::Data(bytes)) => {
                // On a device, this is where the bytes go to flash.
                let part = parts.last_mut().ok_or("data outside a part")?;
                part.len += bytes.len() as u64;
                part.crc = crc32_update(part.crc, bytes);
            }
            Some(Event::Finished) => break,
            _ => {}
        }
        if consumed == 0 && event.is_none() {
            buf.copy_within(start..end, 0);
            end -= start;
            start = 0;
            let n = body.read(&mut buf[end..]).map_err(|e| e.to_string())?;
            eof = n == 0;
            end += n;
        }
    }
    for p in &mut parts {
        p.crc = !p.crc;
    }
    Ok(parts)
}

fn respond(stream: &mut TcpStream, status: &str, html: &str) -> io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
        html.len()
    )
}

fn handle(mut stream: TcpStream) -> io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;
    let (mut content_type, mut content_length) = (Vec::new(), 0u64);
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 || line == "\r\n" {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            let value = value.trim();
            if name.eq_ignore_ascii_case("content-type") {
                content_type = value.as_bytes().to_vec();
            } else if name.eq_ignore_ascii_case("content-length") {
                content_length = value.parse().unwrap_or(0);
            }
        }
    }
    if !request_line.starts_with("POST ") {
        return respond(&mut stream, "200 OK", FORM);
    }
    // `BufReader` may already hold the first body bytes; `take` stops at the
    // end of this request instead of waiting for the client to close.
    match receive(&content_type, reader.take(content_length)) {
        Ok(parts) => {
            let mut html = String::from("<!doctype html><meta charset=\"utf-8\"><pre>");
            for p in &parts {
                let file = p
                    .filename
                    .as_deref()
                    .map(|f| format!(" ({f:?})"))
                    .unwrap_or_default();
                let line = format!("{}{file}: {} bytes, CRC-32 {:08x}", p.name, p.len, p.crc);
                println!("{line}");
                let _ = writeln!(html, "{}", line.replace('&', "&amp;").replace('<', "&lt;"));
            }
            html.push_str("</pre><a href=\"/\">again</a>");
            respond(&mut stream, "200 OK", &html)
        }
        Err(e) => {
            println!("rejected: {e}");
            respond(&mut stream, "400 Bad Request", &format!("<pre>{e}</pre>"))
        }
    }
}

fn main() -> io::Result<()> {
    let addr = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:8080".into());
    let listener = TcpListener::bind(&addr)?;
    println!("open http://{addr}");
    for stream in listener.incoming() {
        if let Err(e) = stream.and_then(handle) {
            println!("connection error: {e}");
        }
    }
    Ok(())
}
