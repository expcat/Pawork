//! Deliberately small RFB 3.8 / raw client for the bundled Xvnc desktop.
//! There is no OS input API or arbitrary network endpoint in this backend.
use super::*;
use image::{codecs::jpeg::JpegEncoder, imageops::FilterType, RgbImage};
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};

const DESKTOP_NAME: &[u8] = b"Pawork-Isolated";
const IO_BUDGET: Duration = Duration::from_secs(8);
static CONNECTION_SEQUENCE: AtomicU64 = AtomicU64::new(1);

pub(super) struct Rfb(Mutex<Option<Connection>>);

impl Rfb {
    pub(super) fn new() -> Self {
        Self(Mutex::new(None))
    }

    fn with<T>(&self, f: impl FnOnce(&mut Connection) -> Result<T, Error>) -> Result<T, Error> {
        let mut slot = self
            .0
            .lock()
            .map_err(|_| failure("connection lock poisoned"))?;
        if slot.is_none() {
            let address = SocketAddr::from((Ipv4Addr::LOCALHOST, 5905));
            let stream =
                TcpStream::connect_timeout(&address, Duration::from_secs(1)).map_err(|_| {
                    failure(
                        "isolated desktop unavailable; start desktop/compose.yaml (127.0.0.1:5905)",
                    )
                })?;
            *slot = Some(Connection::open(stream)?);
        }
        let connection = slot.as_mut().unwrap();
        connection.deadline = Instant::now() + IO_BUDGET;
        let result = f(connection);
        // Never retry an input. A later connection gets a new identity, forcing
        // a fresh observation before the next input.
        if result.is_err() {
            *slot = None;
        }
        result
    }
}

impl Backend for Rfb {
    fn permissions(&self) -> Result<Permissions, Error> {
        self.with(|_| {
            Ok(Permissions {
                capture: true,
                input: true,
            })
        })
    }
    fn desktop(&self) -> Result<Desktop, Error> {
        self.with(|c| Ok(c.desktop.clone()))
    }
    fn capture(&self) -> Result<Capture, Error> {
        self.with(Connection::capture)
    }
    fn input(&self, input: Input, cancelled: &dyn Fn() -> bool) -> Result<(), Error> {
        self.with(|c| c.input(input, cancelled))
    }
}

struct Connection {
    stream: TcpStream,
    deadline: Instant,
    desktop: Desktop,
    pointer: Point,
}

fn failure(message: &str) -> Error {
    Error::Backend(message.into())
}

impl Connection {
    fn open(stream: TcpStream) -> Result<Self, Error> {
        stream
            .set_nodelay(true)
            .map_err(|_| failure("socket setup failed"))?;
        let mut c = Self {
            stream,
            deadline: Instant::now() + IO_BUDGET,
            desktop: Desktop {
                session_id: CONNECTION_SEQUENCE.fetch_add(1, Ordering::Relaxed),
                width: 0.0,
                height: 0.0,
            },
            pointer: Point { x: 0.0, y: 0.0 },
        };
        if &c.read::<12>()? != b"RFB 003.008\n" {
            return Err(failure("isolated desktop requires RFB 3.8"));
        }
        c.write(b"RFB 003.008\n")?;
        let count = c.read::<1>()?[0] as usize;
        let mut types = vec![0; count];
        c.read_into(&mut types)?;
        if !types.contains(&1) {
            return Err(failure("isolated desktop handshake rejected"));
        }
        c.write(&[1])?; // None; this transport is restricted to the local container port.
        if c.read::<4>()? != [0; 4] {
            return Err(failure("isolated desktop connection refused"));
        }
        c.write(&[1])?; // Do not evict another viewer; Xvnc rejects additional clients.
        let init = c.read::<24>()?;
        let width = u16::from_be_bytes([init[0], init[1]]) as u32;
        let height = u16::from_be_bytes([init[2], init[3]]) as u32;
        if width == 0 || height == 0 || width > 4096 || height > 4096 || width * height > 8_388_608
        {
            return Err(failure("virtual framebuffer dimensions exceed budget"));
        }
        let name_len = u32::from_be_bytes(init[20..24].try_into().unwrap()) as usize;
        if name_len != DESKTOP_NAME.len() {
            return Err(failure(
                "endpoint is not the configured Pawork isolated desktop",
            ));
        }
        let mut name = vec![0; name_len];
        c.read_into(&mut name)?;
        if name != DESKTOP_NAME {
            return Err(failure(
                "endpoint is not the configured Pawork isolated desktop",
            ));
        }
        c.desktop.width = width as f64;
        c.desktop.height = height as f64;
        // 32-bit little-endian true color, bytes R,G,B,padding. No clipboard,
        // cursor, resize, compression or vendor extensions are negotiated.
        c.write(&[
            0, 0, 0, 0, 32, 24, 0, 1, 0, 255, 0, 255, 0, 255, 0, 8, 16, 0, 0, 0,
        ])?;
        c.write(&[2, 0, 0, 1, 0, 0, 0, 0])?;
        Ok(c)
    }

    fn remaining(&self) -> Result<Duration, Error> {
        self.deadline
            .checked_duration_since(Instant::now())
            .filter(|d| !d.is_zero())
            .ok_or_else(|| failure("isolated desktop timed out"))
    }
    fn read_into(&mut self, mut bytes: &mut [u8]) -> Result<(), Error> {
        while !bytes.is_empty() {
            self.stream
                .set_read_timeout(Some(self.remaining()?))
                .map_err(|_| failure("socket setup failed"))?;
            let n = self
                .stream
                .read(bytes)
                .map_err(|_| failure("isolated desktop disconnected or timed out"))?;
            if n == 0 {
                return Err(failure("isolated desktop disconnected"));
            }
            bytes = &mut bytes[n..];
        }
        Ok(())
    }
    fn read<const N: usize>(&mut self) -> Result<[u8; N], Error> {
        let mut bytes = [0; N];
        self.read_into(&mut bytes)?;
        Ok(bytes)
    }
    fn write(&mut self, mut bytes: &[u8]) -> Result<(), Error> {
        while !bytes.is_empty() {
            self.stream
                .set_write_timeout(Some(self.remaining()?))
                .map_err(|_| failure("socket setup failed"))?;
            let n = self
                .stream
                .write(bytes)
                .map_err(|_| failure("isolated desktop disconnected or timed out"))?;
            if n == 0 {
                return Err(failure("isolated desktop disconnected"));
            }
            bytes = &bytes[n..];
        }
        Ok(())
    }

    fn capture(&mut self) -> Result<Capture, Error> {
        let (width, height) = (self.desktop.width as u16, self.desktop.height as u16);
        let mut request = vec![3, 0, 0, 0, 0, 0]; // full, non-incremental framebuffer
        request.extend(width.to_be_bytes());
        request.extend(height.to_be_bytes());
        self.write(&request)?;
        let mut rgb = vec![0; width as usize * height as usize * 3];
        let mut covered = vec![false; width as usize * height as usize];
        let mut received = 0usize;
        // Unsolicited messages are bounded and never forwarded to host facilities.
        for _ in 0..64 {
            match self.read::<1>()?[0] {
                0 => {
                    let header = self.read::<3>()?;
                    let count = u16::from_be_bytes([header[1], header[2]]);
                    if count > 4096 {
                        return Err(failure("too many framebuffer rectangles"));
                    }
                    for _ in 0..count {
                        let r = self.read::<12>()?;
                        let u16_at = |i| u16::from_be_bytes([r[i], r[i + 1]]) as usize;
                        let (x, y, w, h) = (u16_at(0), u16_at(2), u16_at(4), u16_at(6));
                        if r[8..12] != [0; 4]
                            || w == 0
                            || h == 0
                            || x + w > width as usize
                            || y + h > height as usize
                        {
                            return Err(failure("invalid raw framebuffer rectangle"));
                        }
                        received += w * h * 4;
                        if received > width as usize * height as usize * 8 {
                            return Err(failure("framebuffer update exceeds byte budget"));
                        }
                        let mut row = vec![0; w * 4];
                        for line in y..y + h {
                            self.read_into(&mut row)?;
                            for (column, pixel) in row.chunks_exact(4).enumerate() {
                                let offset = line * width as usize + x + column;
                                rgb[offset * 3..offset * 3 + 3].copy_from_slice(&pixel[..3]);
                                covered[offset] = true;
                            }
                        }
                    }
                    if covered.iter().all(|p| *p) {
                        let raw = RgbImage::from_raw(width as u32, height as u32, rgb).unwrap();
                        let scale = (MAX_IMAGE_EDGE as f64 / width.max(height) as f64).min(1.0);
                        let scaled = image::imageops::resize(
                            &raw,
                            (width as f64 * scale).round() as u32,
                            (height as f64 * scale).round() as u32,
                            FilterType::Triangle,
                        );
                        for quality in [75, 50, 30] {
                            let mut jpeg = Vec::new();
                            JpegEncoder::new_with_quality(&mut jpeg, quality)
                                .encode_image(&scaled)
                                .map_err(|_| failure("JPEG encoding failed"))?;
                            if jpeg.len() <= MAX_IMAGE_BYTES {
                                return Ok(Capture {
                                    desktop: self.desktop.clone(),
                                    width: scaled.width(),
                                    height: scaled.height(),
                                    jpeg,
                                });
                            }
                        }
                        return Err(failure("screenshot exceeds JPEG byte budget"));
                    }
                }
                2 => {} // Bell: no host sound.
                3 => {
                    let h = self.read::<7>()?;
                    let size = u32::from_be_bytes(h[3..7].try_into().unwrap()) as usize;
                    if size > 65_536 {
                        return Err(failure("clipboard message exceeds discard budget"));
                    }
                    self.read_into(&mut vec![0; size])?; // never access the host clipboard
                }
                _ => return Err(failure("unsupported framebuffer message")),
            }
        }
        Err(failure("incomplete framebuffer update"))
    }

    fn pointer(&mut self, at: Point, mask: u8) -> Result<(), Error> {
        let mut event = vec![5, mask];
        event.extend((at.x.floor() as u16).to_be_bytes());
        event.extend((at.y.floor() as u16).to_be_bytes());
        self.write(&event)?;
        self.pointer = at;
        Ok(())
    }
    fn key(&mut self, keysym: u32, down: bool) -> Result<(), Error> {
        let mut event = vec![4, down as u8, 0, 0];
        event.extend(keysym.to_be_bytes());
        self.write(&event)
    }
    fn input(&mut self, input: Input, cancelled: &dyn Fn() -> bool) -> Result<(), Error> {
        let check = || {
            if cancelled() {
                Err(Error::Cancelled)
            } else {
                Ok(())
            }
        };
        let mut held = Vec::new();
        let mut pointer_held = false;
        let result = (|| {
            check()?;
            match input {
                Input::Move(at) => self.pointer(at, 0)?,
                Input::Click { at, button, clicks } => {
                    let mask = match button {
                        Button::Left => 1,
                        Button::Middle => 2,
                        Button::Right => 4,
                    };
                    for _ in 0..clicks {
                        check()?;
                        pointer_held = true;
                        self.pointer(at, mask)?;
                        self.pointer(at, 0)?;
                        pointer_held = false;
                    }
                }
                Input::Drag { from, to } => {
                    pointer_held = true;
                    self.pointer(from, 1)?;
                    for step in 1..=12 {
                        check()?;
                        let f = step as f64 / 12.0;
                        self.pointer(
                            Point {
                                x: from.x + (to.x - from.x) * f,
                                y: from.y + (to.y - from.y) * f,
                            },
                            1,
                        )?;
                        std::thread::sleep(Duration::from_millis(12));
                    }
                }
                Input::Scroll {
                    at,
                    delta_x,
                    delta_y,
                } => {
                    for (delta, negative, positive) in [(delta_y, 8, 16), (delta_x, 32, 64)] {
                        for _ in 0..delta.unsigned_abs().div_ceil(40) {
                            check()?;
                            pointer_held = true;
                            self.pointer(at, if delta < 0 { negative } else { positive })?;
                            self.pointer(at, 0)?;
                            pointer_held = false;
                        }
                    }
                }
                Input::TypeText(text) => {
                    for ch in text.chars() {
                        check()?;
                        let code = ch as u32;
                        let code = if code <= 0xff {
                            code
                        } else {
                            0x0100_0000 | code
                        };
                        held.push(code);
                        self.key(code, true)?;
                        self.key(code, false)?;
                        held.pop();
                    }
                }
                Input::Key { key, modifiers } => {
                    for modifier in modifiers {
                        let code = match modifier.as_str() {
                            "shift" => 0xffe1,
                            "control" => 0xffe3,
                            "option" => 0xffe9,
                            "command" => 0xffeb,
                            _ => return Err(Error::Invalid("unsupported modifier")),
                        };
                        held.push(code);
                        self.key(code, true)?;
                    }
                    let code = keysym(&key).ok_or(Error::Invalid("unsupported key"))?;
                    held.push(code);
                    self.key(code, true)?;
                }
            }
            Ok(())
        })();
        // Releases are attempted even on cancellation/error. On transport failure
        // `with` also drops the connection; Xvnc releases its client input state.
        let mut release = Ok(());
        for code in held.into_iter().rev() {
            if let Err(error) = self.key(code, false) {
                release = Err(error);
            }
        }
        if pointer_held {
            if let Err(error) = self.pointer(self.pointer, 0) {
                release = Err(error);
            }
        }
        result.and(release)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    fn read<const N: usize>(stream: &mut TcpStream) -> [u8; N] {
        let mut data = [0; N];
        stream.read_exact(&mut data).unwrap();
        data
    }
    fn server(stream: &mut TcpStream, name: &[u8], width: u16) {
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        stream.write_all(b"RFB 003.008\n").unwrap();
        assert_eq!(&read::<12>(stream), b"RFB 003.008\n");
        stream.write_all(&[1, 1]).unwrap();
        assert_eq!(read::<1>(stream), [1]);
        stream.write_all(&[0; 4]).unwrap();
        assert_eq!(read::<1>(stream), [1]);
        let mut init = vec![];
        init.extend(width.to_be_bytes());
        init.extend(1u16.to_be_bytes());
        init.extend([32, 24, 0, 1, 0, 255, 0, 255, 0, 255, 0, 8, 16, 0, 0, 0]);
        init.extend((name.len() as u32).to_be_bytes());
        init.extend(name);
        stream.write_all(&init).unwrap();
    }
    fn pair(
        f: impl FnOnce(TcpStream) + Send + 'static,
    ) -> (TcpStream, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let thread = std::thread::spawn(move || f(listener.accept().unwrap().0));
        (TcpStream::connect(address).unwrap(), thread)
    }

    #[test]
    fn raw_screenshot_and_virtual_unicode_keys_click_and_cancelled_drag() {
        let (stream, thread) = pair(|mut s| {
            server(&mut s, DESKTOP_NAME, 2);
            assert_eq!(
                read::<20>(&mut s)[4..],
                [32, 24, 0, 1, 0, 255, 0, 255, 0, 255, 0, 8, 16, 0, 0, 0]
            );
            assert_eq!(read::<8>(&mut s), [2, 0, 0, 1, 0, 0, 0, 0]);
            assert_eq!(read::<10>(&mut s), [3, 0, 0, 0, 0, 0, 0, 2, 0, 1]);
            // A discarded clipboard update and bell cannot affect host state.
            s.write_all(&[3, 0, 0, 0, 0, 0, 0, 1, b'x', 2]).unwrap();
            s.write_all(&[
                0, 0, 0, 1, 0, 0, 0, 0, 0, 2, 0, 1, 0, 0, 0, 0, 255, 0, 0, 0, 0, 255, 0, 0,
            ])
            .unwrap();
            assert_eq!(read::<12>(&mut s), [5, 1, 0, 1, 0, 0, 5, 0, 0, 1, 0, 0]);
            for (code, down) in [(0x0100_4e2d_u32, true), (0x0100_4e2d, false)] {
                let event = read::<8>(&mut s);
                assert_eq!(event[..4], [4, down as u8, 0, 0]);
                assert_eq!(event[4..], code.to_be_bytes());
            }
            for (code, down) in [(0xffe3_u32, true), (97, true), (97, false), (0xffe3, false)] {
                let event = read::<8>(&mut s);
                assert_eq!(event[..4], [4, down as u8, 0, 0]);
                assert_eq!(event[4..], code.to_be_bytes());
            }
            assert_eq!(read::<6>(&mut s)[1], 1);
            assert_eq!(read::<6>(&mut s)[1], 1);
            assert_eq!(read::<6>(&mut s)[1], 0); // cancellation still releases
        });
        let mut c = Connection::open(stream).unwrap();
        let capture = c.capture().unwrap();
        let image = image::load_from_memory(&capture.jpeg).unwrap();
        assert_eq!((image.width(), image.height()), (2, 1));
        assert!(image.to_rgb8().get_pixel(0, 0)[0] > 150);
        c.input(
            Input::Click {
                at: Point { x: 1.0, y: 0.0 },
                button: Button::Left,
                clicks: 1,
            },
            &|| false,
        )
        .unwrap();
        c.input(Input::TypeText("中".into()), &|| false).unwrap();
        c.input(
            Input::Key {
                key: "a".into(),
                modifiers: vec!["control".into()],
            },
            &|| false,
        )
        .unwrap();
        let calls = AtomicU64::new(0);
        assert!(matches!(
            c.input(
                Input::Drag {
                    from: Point { x: 0.0, y: 0.0 },
                    to: Point { x: 1.0, y: 0.0 }
                },
                &|| calls.fetch_add(1, Ordering::Relaxed) >= 2
            ),
            Err(Error::Cancelled)
        ));
        thread.join().unwrap();
    }

    #[test]
    fn rejects_wrong_desktop_oversized_frame_and_out_of_bounds_rectangle() {
        for mode in 0..3 {
            let (stream, thread) = pair(move |mut s| {
                server(
                    &mut s,
                    if mode == 0 {
                        b"Physical screen"
                    } else {
                        DESKTOP_NAME
                    },
                    if mode == 1 { u16::MAX } else { 2 },
                );
                if mode == 2 {
                    read::<20>(&mut s);
                    read::<8>(&mut s);
                    read::<10>(&mut s);
                    s.write_all(&[0, 0, 0, 1, 0, 1, 0, 0, 0, 2, 0, 1, 0, 0, 0, 0])
                        .unwrap();
                }
                // No input is sent to a rejected endpoint/frame.
                match s.read(&mut [0; 1]) {
                    Ok(0) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => {}
                    other => panic!("rejected endpoint received input: {other:?}"),
                }
            });
            let connection = Connection::open(stream);
            if mode == 2 {
                let mut c = connection.unwrap();
                assert!(c.capture().is_err());
                drop(c);
            } else {
                assert!(connection.is_err());
            }
            thread.join().unwrap();
        }
    }
}
