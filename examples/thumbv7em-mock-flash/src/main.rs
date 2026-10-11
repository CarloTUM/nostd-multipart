//! `#![no_std]` example for `thumbv7em-none-eabihf`: receives a multipart upload
//! through the `embedded-io` adapter with a 256-byte buffer and writes the part
//! named `firmware` into a mock flash, page by page. No `alloc`, no heap.
//!
//! Build with `cargo build --release` (the target is set in `.cargo/config.toml`).
//! The binary is not meant to run on hardware (no vector table / HAL); it proves
//! that the crate links for a Cortex-M4F target without `alloc`.
#![no_std]
#![no_main]

use core::sync::atomic::{AtomicU32, Ordering};

use embedded_io::{ErrorType, Read};
use nostd_multipart::{content_disposition, Event, MultipartReader, Parser, ReadError};

const PAGE: usize = 256;
const PAGES: usize = 16;

/// A tiny mock of a page-programmed flash: data is staged in a page buffer and
/// "programmed" when the page is full or the part ends.
struct MockFlash {
    page: [u8; PAGE],
    fill: usize,
    next_page: usize,
    programmed: [[u8; PAGE]; PAGES],
}

#[derive(Debug)]
enum FlashError {
    Full,
}

impl MockFlash {
    const fn new() -> Self {
        MockFlash {
            page: [0xff; PAGE],
            fill: 0,
            next_page: 0,
            programmed: [[0xff; PAGE]; PAGES],
        }
    }

    fn write(&mut self, mut data: &[u8]) -> Result<(), FlashError> {
        while !data.is_empty() {
            let room = PAGE - self.fill;
            let n = room.min(data.len());
            let (now, later) = data.split_at(n);
            self.page[self.fill..self.fill + n].copy_from_slice(now);
            self.fill += n;
            data = later;
            if self.fill == PAGE {
                self.flush()?;
            }
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<(), FlashError> {
        if self.fill == 0 {
            return Ok(());
        }
        let slot = self
            .programmed
            .get_mut(self.next_page)
            .ok_or(FlashError::Full)?;
        slot.copy_from_slice(&self.page);
        self.next_page += 1;
        self.page = [0xff; PAGE];
        self.fill = 0;
        Ok(())
    }
}

/// Stand-in for a TCP socket: hands out the request body 37 bytes at a time.
struct Socket {
    body: &'static [u8],
}

impl ErrorType for Socket {
    type Error = core::convert::Infallible;
}

impl Read for Socket {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        let n = buf.len().min(self.body.len()).min(37);
        buf[..n].copy_from_slice(&self.body[..n]);
        self.body = &self.body[n..];
        Ok(n)
    }
}

/// Outcome of the upload, kept in a global so the optimizer keeps the code.
static RESULT: AtomicU32 = AtomicU32::new(0);

const CONTENT_TYPE: &[u8] = b"multipart/form-data; boundary=----ota-7f3e";
const BODY: &[u8] = b"------ota-7f3e\r\nContent-Disposition: form-data; name=\"version\"\r\n\r\n1.2.3\r\n------ota-7f3e\r\nContent-Disposition: form-data; name=\"firmware\"; filename=\"app.bin\"\r\nContent-Type: application/octet-stream\r\n\r\n\x7fELF\x01\x01\x01\x00firmware image bytes ... \xde\xad\xbe\xef\r\n------ota-7f3e--\r\n";

fn receive(flash: &mut MockFlash) -> Result<u32, ReadError<core::convert::Infallible>> {
    let parser: Parser<'_, 128, 8> = Parser::from_content_type(CONTENT_TYPE)?;
    let mut reader = MultipartReader::<_, 256, 128, 8>::new(Socket { body: BODY }, parser);
    let mut in_firmware = false;
    let mut written: u32 = 0;
    loop {
        match reader.next_event()? {
            Event::PartStart => in_firmware = false,
            Event::Header { name, value } if name.eq_ignore_ascii_case(b"content-disposition") => {
                if let Ok(cd) = content_disposition(value) {
                    in_firmware = cd.name().is_some_and(|n| n == "firmware");
                }
            }
            Event::Data(d) if in_firmware => {
                flash.write(d).map_err(|_| ReadError::BufferTooSmall)?;
                written += d.len() as u32;
            }
            Event::PartEnd if in_firmware => {
                flash.flush().map_err(|_| ReadError::BufferTooSmall)?
            }
            Event::Finished => return Ok(written),
            _ => {}
        }
    }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let mut flash = MockFlash::new();
    let code = match receive(&mut flash) {
        Ok(n) if flash.programmed[0].starts_with(b"\x7fELF") => n,
        Ok(_) => 0xffff_fffe,
        Err(_) => 0xffff_ffff,
    };
    RESULT.store(code, Ordering::SeqCst);
    loop {
        core::hint::spin_loop();
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
