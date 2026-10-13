//! Firefox guest for the isolated Linux desktop.
//!
//! Listens on 0.0.0.0:5906 inside the container so Docker can publish that
//! port to the host loopback. The host client still dials only 127.0.0.1:5906.
//! macOS and every other host target exit immediately; AT-SPI and X11 exist
//! only in the Linux binary. `--wayland` instead serves the CU-26 dedicated
//! Sway seat on 0.0.0.0:5907 (see `wayland::guest`).

fn main() {
    let code = match run() {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("computer_guest: {error}");
            1
        }
    };
    std::process::exit(code);
}

fn run() -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        if std::env::args().nth(1).as_deref() == Some("--wayland") {
            return pawork_computer_use::wayland::guest::run();
        }
        linux::run()
    }
    #[cfg(not(target_os = "linux"))]
    {
        Err("computer_guest only runs inside the isolated Linux desktop".into())
    }
}

struct WindowGen {
    generation: u64,
    live: bool,
    pid: u32,
    start_token: u64,
}

fn issue_generation(
    entry: &mut WindowGen,
    pid: u32,
    start_token: u64,
    nonce: u64,
    epoch: u64,
) -> u64 {
    if !entry.live || entry.pid != pid || entry.start_token != start_token {
        entry.generation = entry.generation.saturating_add(1);
        entry.live = true;
        entry.pid = pid;
        entry.start_token = start_token;
    }
    let mixed = nonce.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(
        epoch
            .wrapping_mul(0x6C62_272E_07BB_0142)
            .wrapping_add(entry.generation),
    );
    if mixed == 0 {
        1
    } else {
        mixed
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::{issue_generation, WindowGen};
    use pawork_computer_use::guest::{
        serve_connection, tree_revision, GuestApp, GuestAppInfo, GuestError, GuestMetadata,
        GuestNode, GuestTree, GuestTruncation, GuestWindow, PressQuery, Pressed, RejectedWindow,
        TreeQuery, FIREFOX_BUNDLE_ID, GUEST_PORT,
    };
    use pawork_computer_use::target::AX_TREE_BOUNDS;
    use std::collections::HashMap;
    use std::future::Future;
    use std::io::{Read, Write};
    use std::net::{Ipv4Addr, TcpListener};
    use std::os::unix::net::UnixStream;
    use std::task::Poll;
    use std::time::{Duration, Instant};
    use zbus::zvariant::{OwnedObjectPath, OwnedValue};

    const DISPLAY_SOCKET: &str = "/tmp/.X11-unix/X0";
    const NOT_FIREFOX: &str =
        "only the isolated firefox guest is admitted; this window is not firefox and is not an appkit application";

    pub fn run() -> Result<(), String> {
        let token =
            pawork_computer_use::guest::token_from_env().map_err(|error| error.to_string())?;
        let listener = TcpListener::bind((Ipv4Addr::UNSPECIFIED, GUEST_PORT))
            .map_err(|error| format!("bind 0.0.0.0:{GUEST_PORT}: {error}"))?;
        eprintln!("computer_guest listening on 0.0.0.0:{GUEST_PORT}");
        let mut desktop = LinuxDesktop {
            generations: HashMap::new(),
            epoch: 0,
            nonce: process_nonce()?,
        };
        loop {
            let (stream, _) = listener
                .accept()
                .map_err(|error| format!("accept: {error}"))?;
            if let Err(error) = serve_connection(stream, &token, &mut desktop) {
                eprintln!("computer_guest connection closed: {error}");
            }
        }
    }

    struct LinuxDesktop {
        generations: HashMap<u32, WindowGen>,
        epoch: u64,
        nonce: u64,
    }

    impl GuestApp for LinuxDesktop {
        fn begin_session(&mut self) {
            self.epoch = self.epoch.saturating_add(1);
        }

        fn metadata(&mut self) -> Result<GuestMetadata, GuestError> {
            let found = x11_windows().map_err(|error| GuestError::Unavailable(error))?;
            let mut live = Vec::new();
            let mut apps: HashMap<u32, GuestAppInfo> = HashMap::new();
            let mut rejected = Vec::new();
            for window in found {
                live.push(window.window_id);
                if window.firefox {
                    let generation = self.generation(&window);
                    let app = apps.entry(window.pid).or_insert_with(|| GuestAppInfo {
                        bundle_id: FIREFOX_BUNDLE_ID.into(),
                        name: "Firefox".into(),
                        pid: window.pid,
                        start_token: window.start_token,
                        windows: Vec::new(),
                    });
                    app.windows.push(GuestWindow {
                        window_id: u64::from(window.window_id),
                        generation,
                        x: window.x,
                        y: window.y,
                        width: window.width,
                        height: window.height,
                        class: window.class,
                        instance: window.instance,
                        stack: stack_order(window.stack),
                        parent: u64::from(window.parent),
                    });
                } else if window.occluder {
                    rejected.push(RejectedWindow {
                        window_id: u64::from(window.window_id),
                        x: window.x,
                        y: window.y,
                        width: window.width,
                        height: window.height,
                        class: window.class,
                        instance: window.instance,
                        stack: stack_order(window.stack),
                        parent: u64::from(window.parent),
                        geometry_known: window.geometry_known,
                        reason: NOT_FIREFOX.into(),
                    });
                }
            }
            self.retire(&live);
            let mut apps: Vec<_> = apps.into_values().collect();
            apps.sort_by_key(|app| app.pid);
            for app in &mut apps {
                app.windows.sort_by_key(|window| window.window_id);
            }
            Ok(GuestMetadata {
                apps,
                rejected,
                epoch: self.epoch,
            })
        }

        fn tree(&mut self, query: &TreeQuery) -> Result<GuestTree, GuestError> {
            let window = self
                .require_window(query.window_id, query.generation)
                .map_err(GuestError::Unavailable)?;
            read_window_tree(&window).map_err(GuestError::Unavailable)
        }

        fn press(
            &mut self,
            query: &PressQuery,
            stop: &dyn Fn() -> bool,
        ) -> Result<Pressed, GuestError> {
            if query.path.len() > 64 {
                return Err(GuestError::Protocol("press path is too deep"));
            }
            if stop() {
                return Err(GuestError::Unavailable(
                    "cancelled before the guest action".into(),
                ));
            }
            press_path(self, query, stop).map_err(GuestError::Unavailable)
        }
    }

    impl LinuxDesktop {
        fn generation(&mut self, window: &RawWindow) -> u64 {
            let entry = self
                .generations
                .entry(window.window_id)
                .or_insert(WindowGen {
                    generation: 0,
                    live: false,
                    pid: window.pid,
                    start_token: window.start_token,
                });
            issue_generation(
                entry,
                window.pid,
                window.start_token,
                self.nonce,
                self.epoch,
            )
        }

        fn require_window(&mut self, window_id: u64, generation: u64) -> Result<RawWindow, String> {
            let found = x11_windows()?;
            let live: Vec<u32> = found.iter().map(|window| window.window_id).collect();
            let window = found
                .into_iter()
                .find(|window| window.firefox && u64::from(window.window_id) == window_id);
            self.retire(&live);
            let Some(window) = window else {
                return Err(format!(
                    "window {window_id} is not the issued firefox window"
                ));
            };
            let current = self.generation(&window);
            if current != generation {
                return Err(format!(
                    "window generation {generation} does not match the live window"
                ));
            }
            Ok(window)
        }

        fn retire(&mut self, live: &[u32]) {
            for (id, entry) in &mut self.generations {
                entry.live = live.contains(id);
            }
        }
    }

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum StackRank {
        Known(u32),
        Unknown,
    }

    struct RawWindow {
        window_id: u32,
        parent: u32,
        pid: u32,
        start_token: u64,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        class: String,
        instance: String,
        firefox: bool,
        stack: StackRank,
        /// A visible window that can hide target pixels. Target chrome is excluded later.
        occluder: bool,
        geometry_known: bool,
    }

    fn x11_windows() -> Result<Vec<RawWindow>, String> {
        let mut x11 = X11::open()?;
        let stacking = client_stack(&mut x11)?;
        let root = x11.root;
        let mut found = Vec::new();
        let mut queue = vec![root];
        let mut seen = 0usize;
        while let Some(window) = queue.pop() {
            seen += 1;
            if seen > 4096 || found.len() > 4096 {
                return Err("x11 window enumeration is incomplete".into());
            }
            let attrs = match x11.attributes(window) {
                Ok(attrs) => attrs,
                Err(error) => return Err(format!("x11 window attributes are incomplete: {error}")),
            };
            if attrs.map_state != 2 {
                continue;
            }
            if window != root && attrs.class != 2 {
                if let Some(mut raw) = describe_window(&mut x11, window)? {
                    raw.stack = match stacking.get(&window).copied() {
                        Some(order) => StackRank::Known(order),
                        None => StackRank::Unknown,
                    };
                    found.push(raw);
                }
            }
            let children = x11
                .query_tree(window)
                .map_err(|error| format!("x11 window tree is incomplete: {error}"))?;
            queue.extend(children);
        }
        Ok(found)
    }

    fn client_stack(x11: &mut X11) -> Result<HashMap<u32, u32>, String> {
        let bytes = match x11.property_bytes(x11.root, "_NET_CLIENT_LIST_STACKING", 1024)? {
            Some(bytes) => bytes,
            None => Vec::new(),
        };
        let mut order = HashMap::new();
        for (index, chunk) in bytes.chunks_exact(4).enumerate() {
            let id = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
            order.insert(id, index as u32);
        }
        Ok(order)
    }

    /// Known stacking order stays a small integer. An unlisted window cannot be
    /// ranked below the target, so it uses the sentinel that capture refuses.
    fn stack_order(rank: StackRank) -> u32 {
        match rank {
            StackRank::Known(order) => order,
            StackRank::Unknown => u32::MAX,
        }
    }

    fn describe_window(x11: &mut X11, window: u32) -> Result<Option<RawWindow>, String> {
        let parent = x11
            .parent(window)
            .map_err(|error| format!("x11 window parent is incomplete: {error}"))?;
        let geometry = x11.geometry(window).ok();
        let origin = x11.origin(window).ok();
        let geometry_known = geometry.is_some() && origin.is_some();
        let (width, height) = geometry.unwrap_or((1, 1));
        let (x, y) = origin.unwrap_or((0, 0));
        if geometry_known && (width == 0 || height == 0) {
            return Ok(None);
        }
        let (instance, class) = x11
            .wm_class(window)
            .unwrap_or_else(|| (String::new(), String::new()));
        let firefox_sized = width >= 80 && height >= 80;
        let pid = x11.wm_pid(window).unwrap_or(0);
        let process = process_text(pid);
        let named = !(instance.is_empty() && class.is_empty());
        let firefox = geometry_known
            && named
            && firefox_sized
            && is_firefox(&class, &instance, process.as_deref());
        let start_token = if firefox {
            start_token(pid).unwrap_or(0)
        } else {
            0
        };
        if firefox && start_token == 0 {
            return Ok(Some(RawWindow {
                window_id: window,
                parent,
                pid,
                start_token: 0,
                x: 0,
                y: 0,
                width,
                height,
                class,
                instance,
                firefox: false,
                stack: StackRank::Unknown,
                occluder: true,
                geometry_known: false,
            }));
        }
        Ok(Some(RawWindow {
            window_id: window,
            parent,
            pid,
            start_token,
            x,
            y,
            width,
            height,
            class,
            instance,
            firefox,
            stack: StackRank::Unknown,
            occluder: !firefox,
            geometry_known,
        }))
    }

    fn is_firefox(class: &str, instance: &str, process: Option<&str>) -> bool {
        let class = class.to_ascii_lowercase();
        let instance = instance.to_ascii_lowercase();
        class.contains("firefox")
            || instance.contains("firefox")
            || process.is_some_and(|text| text.contains("firefox"))
    }

    fn process_text(pid: u32) -> Option<String> {
        if pid == 0 {
            return None;
        }
        let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
        let cmd = String::from_utf8_lossy(
            &std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default(),
        )
        .replace('\0', " ");
        Some(format!("{comm} {cmd}").to_ascii_lowercase())
    }

    fn start_token(pid: u32) -> Option<u64> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let rest = stat.rsplit_once(')')?.1.trim_start();
        // Field 22 (starttime) is the 20th field after comm.
        rest.split_whitespace().nth(19)?.parse().ok()
    }

    fn process_nonce() -> Result<u64, String> {
        let pid = u64::from(std::process::id());
        let stat = std::fs::read_to_string("/proc/self/stat")
            .map_err(|error| format!("guest starttime: {error}"))?;
        let rest = stat
            .rsplit_once(')')
            .ok_or_else(|| "guest starttime is missing".to_string())?
            .1
            .trim_start();
        let start: u64 = rest
            .split_whitespace()
            .nth(19)
            .ok_or_else(|| "guest starttime field is missing".to_string())?
            .parse()
            .map_err(|_| "guest starttime is not a number".to_string())?;
        if start == 0 {
            return Err("guest starttime is zero".into());
        }
        let mixed = pid
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
            .wrapping_add(start.wrapping_mul(0xBF58_476D_1CE4_E5B9));
        Ok(if mixed == 0 { 1 } else { mixed })
    }

    struct X11 {
        stream: UnixStream,
        root: u32,
        atoms: HashMap<&'static str, u32>,
    }

    struct Attributes {
        class: u8,
        map_state: u8,
    }

    impl X11 {
        fn open() -> Result<Self, String> {
            let mut stream = UnixStream::connect(DISPLAY_SOCKET)
                .map_err(|error| format!("x11 connect: {error}"))?;
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .map_err(|error| format!("x11 timeout: {error}"))?;
            stream
                .set_write_timeout(Some(Duration::from_secs(2)))
                .map_err(|error| format!("x11 timeout: {error}"))?;
            let mut setup = [0u8; 12];
            setup[0] = b'l';
            setup[2] = 11;
            stream
                .write_all(&setup)
                .map_err(|error| format!("x11 setup: {error}"))?;
            let mut head = [0u8; 8];
            stream
                .read_exact(&mut head)
                .map_err(|error| format!("x11 setup reply: {error}"))?;
            if head[0] != 1 {
                return Err(format!("x11 setup rejected status {}", head[0]));
            }
            let extra = u16::from_le_bytes([head[6], head[7]]) as usize;
            let mut body = vec![0u8; extra * 4];
            stream
                .read_exact(&mut body)
                .map_err(|error| format!("x11 setup body: {error}"))?;
            if body.len() < 32 {
                return Err("x11 setup is too short".into());
            }
            let vendor_len = u16::from_le_bytes([body[16], body[17]]) as usize;
            let roots = body[20] as usize;
            let formats = body[21] as usize;
            let offset = 32 + vendor_len.div_ceil(4) * 4 + formats * 8;
            if roots == 0 || body.len() < offset + 4 {
                return Err("x11 setup has no root window".into());
            }
            let root = u32::from_le_bytes(body[offset..offset + 4].try_into().unwrap());
            Ok(Self {
                stream,
                root,
                atoms: HashMap::new(),
            })
        }

        fn atom(&mut self, name: &'static str) -> Result<u32, String> {
            if let Some(atom) = self.atoms.get(name).copied() {
                return Ok(atom);
            }
            let mut body = Vec::new();
            body.extend_from_slice(&(name.len() as u16).to_le_bytes());
            body.extend_from_slice(&[0, 0]);
            body.extend_from_slice(name.as_bytes());
            pad4(&mut body);
            let reply = self.round_trip(16, 0, &body)?;
            let atom = u32::from_le_bytes(reply[8..12].try_into().unwrap());
            if atom == 0 {
                return Err(format!("x11 atom {name} is missing"));
            }
            self.atoms.insert(name, atom);
            Ok(atom)
        }

        fn attributes(&mut self, window: u32) -> Result<Attributes, String> {
            let reply = self.round_trip(3, 0, &window.to_le_bytes())?;
            if reply.len() < 27 {
                return Err("x11 attributes reply is short".into());
            }
            Ok(Attributes {
                class: u16::from_le_bytes([reply[12], reply[13]]) as u8,
                map_state: reply[26],
            })
        }

        fn query_tree(&mut self, window: u32) -> Result<Vec<u32>, String> {
            let reply = self.round_trip(15, 0, &window.to_le_bytes())?;
            tree_children(&reply)
        }

        fn parent(&mut self, window: u32) -> Result<u32, String> {
            let reply = self.round_trip(15, 0, &window.to_le_bytes())?;
            tree_parent(&reply)
        }

        fn geometry(&mut self, window: u32) -> Result<(u32, u32), String> {
            let reply = self.round_trip(14, 0, &window.to_le_bytes())?;
            if reply.len() < 20 {
                return Err("x11 geometry reply is short".into());
            }
            let width = u16::from_le_bytes([reply[16], reply[17]]) as u32;
            let height = u16::from_le_bytes([reply[18], reply[19]]) as u32;
            Ok((width, height))
        }

        fn origin(&mut self, window: u32) -> Result<(i32, i32), String> {
            let mut body = Vec::new();
            body.extend_from_slice(&window.to_le_bytes());
            body.extend_from_slice(&self.root.to_le_bytes());
            body.extend_from_slice(&0i16.to_le_bytes());
            body.extend_from_slice(&0i16.to_le_bytes());
            let reply = self.round_trip(40, 0, &body)?;
            if reply.len() < 16 {
                return Err("x11 translate reply is short".into());
            }
            let x = i16::from_le_bytes([reply[12], reply[13]]) as i32;
            let y = i16::from_le_bytes([reply[14], reply[15]]) as i32;
            Ok((x, y))
        }

        fn wm_class(&mut self, window: u32) -> Option<(String, String)> {
            let bytes = self.property_bytes(window, "WM_CLASS", 256).ok()??;
            let mut parts = bytes
                .split(|byte| *byte == 0)
                .filter(|part| !part.is_empty());
            let instance = String::from_utf8_lossy(parts.next()?).into_owned();
            let class = String::from_utf8_lossy(parts.next().unwrap_or_default()).into_owned();
            Some((instance, class))
        }

        fn wm_pid(&mut self, window: u32) -> Option<u32> {
            let bytes = self.property_bytes(window, "_NET_WM_PID", 4).ok()??;
            if bytes.len() < 4 {
                return None;
            }
            Some(u32::from_le_bytes(bytes[..4].try_into().ok()?))
        }

        fn property_bytes(
            &mut self,
            window: u32,
            name: &'static str,
            long_length: u32,
        ) -> Result<Option<Vec<u8>>, String> {
            let atom = self.atom(name)?;
            let mut body = Vec::new();
            body.extend_from_slice(&window.to_le_bytes());
            body.extend_from_slice(&atom.to_le_bytes());
            body.extend_from_slice(&0u32.to_le_bytes());
            body.extend_from_slice(&0u32.to_le_bytes());
            body.extend_from_slice(&long_length.to_le_bytes());
            let reply = self.round_trip(20, 0, &body)?;
            if reply.len() < 32 {
                return Err("x11 property reply is short".into());
            }
            let format = reply[1];
            if format == 0 {
                return Ok(None);
            }
            let unit = match format {
                8 => 1,
                16 => 2,
                32 => 4,
                _ => return Err(format!("x11 property format {format}")),
            };
            let count = u32::from_le_bytes(reply[16..20].try_into().unwrap()) as usize;
            let size = count * unit;
            let end = 32 + size;
            if end > reply.len() {
                return Err("x11 property value is short".into());
            }
            Ok(Some(reply[32..end].to_vec()))
        }

        fn round_trip(&mut self, opcode: u8, data: u8, body: &[u8]) -> Result<Vec<u8>, String> {
            let mut request = Vec::with_capacity(4 + body.len());
            request.push(opcode);
            request.push(data);
            let units = ((4 + body.len()) / 4) as u16;
            request.extend_from_slice(&units.to_le_bytes());
            request.extend_from_slice(body);
            if request.len() % 4 != 0 {
                return Err("x11 request is not padded".into());
            }
            self.stream
                .write_all(&request)
                .map_err(|error| format!("x11 write: {error}"))?;
            let mut header = [0u8; 32];
            self.stream
                .read_exact(&mut header)
                .map_err(|error| format!("x11 read: {error}"))?;
            if header[0] == 0 {
                return Err(format!("x11 error code {}", header[1]));
            }
            if header[0] != 1 {
                return Err(format!("x11 unexpected packet {}", header[0]));
            }
            let extra = u32::from_le_bytes(header[4..8].try_into().unwrap()) as usize;
            let mut reply = header.to_vec();
            let mut rest = vec![0u8; extra * 4];
            if !rest.is_empty() {
                self.stream
                    .read_exact(&mut rest)
                    .map_err(|error| format!("x11 read body: {error}"))?;
                reply.extend(rest);
            }
            Ok(reply)
        }
    }

    fn pad4(body: &mut Vec<u8>) {
        while body.len() % 4 != 0 {
            body.push(0);
        }
    }

    /// QueryTree stores root at bytes 8..12 and the parent window at 12..16.
    fn tree_parent(reply: &[u8]) -> Result<u32, String> {
        if reply.len() < 16 {
            return Err("x11 query tree reply is short".into());
        }
        Ok(u32::from_le_bytes(reply[12..16].try_into().unwrap()))
    }

    /// A child list that does not fit the reply is an incomplete enumeration.
    fn tree_children(reply: &[u8]) -> Result<Vec<u32>, String> {
        if reply.len() < 18 {
            return Err("x11 query tree reply is short".into());
        }
        let count = u16::from_le_bytes([reply[16], reply[17]]) as usize;
        let mut children = Vec::with_capacity(count);
        for index in 0..count {
            let start = 32 + index * 4;
            if start + 4 > reply.len() {
                return Err("x11 query tree reply is short".into());
            }
            children.push(u32::from_le_bytes(
                reply[start..start + 4].try_into().unwrap(),
            ));
        }
        Ok(children)
    }

    fn read_window_tree(window: &RawWindow) -> Result<GuestTree, String> {
        let budget = Budget::start();
        let conn = a11y_connection(budget)?;
        let frame = locate_frame(&conn, budget, window)?;
        let mut walk = Walk::default();
        walk.visit(
            &conn,
            budget,
            &frame.dest,
            frame.path.as_str(),
            Vec::new(),
            0,
            (window.x, window.y),
        )?;
        rooted_tree(window.window_id, walk)
    }

    fn press_path(
        desktop: &mut LinuxDesktop,
        query: &PressQuery,
        stop: &dyn Fn() -> bool,
    ) -> Result<Pressed, String> {
        if stop() {
            return Err("cancelled before the guest action".into());
        }
        let window = desktop.require_window(query.window_id, query.generation)?;
        let budget = Budget::start();
        let conn = a11y_connection(budget)?;
        let frame = locate_frame(&conn, budget, &window)?;
        let mut walk = Walk {
            want: Some(query.path.clone()),
            ..Walk::default()
        };
        walk.visit(
            &conn,
            budget,
            &frame.dest,
            frame.path.as_str(),
            Vec::new(),
            0,
            (window.x, window.y),
        )?;
        let located = walk.located.take();
        let tree = rooted_tree(window.window_id, walk)?;
        let again = desktop.require_window(query.window_id, query.generation)?;
        if again.x != window.x
            || again.y != window.y
            || again.width != window.width
            || again.height != window.height
            || again.pid != window.pid
            || again.start_token != window.start_token
        {
            return Err("issued window changed before the action".into());
        }
        let revision = tree_revision(&tree);
        if revision != query.tree_revision {
            return Err("tree revision changed before the action".into());
        }
        let Some(located) = located else {
            return Err("press path is not in the issued window tree".into());
        };
        if located.path != query.path {
            return Err("press path is not in the issued window tree".into());
        }
        if located.actions.is_empty() {
            return Err("element has no action".into());
        }
        let index = action_index(&located.actions);
        let action = located.actions[index].clone();
        if stop() {
            return Err("cancelled before the guest action".into());
        }
        do_action(
            &conn,
            budget,
            &located.dest,
            located.object.as_str(),
            index as i32,
        )?;
        Ok(Pressed {
            window_id: query.window_id,
            generation: query.generation,
            tree_revision: revision,
            path: query.path.clone(),
            role: located.role,
            action,
        })
    }

    fn rooted_tree(window_id: u32, walk: Walk) -> Result<GuestTree, String> {
        if walk
            .nodes
            .first()
            .is_none_or(|root| !root.path.is_empty() || root.depth != 0)
        {
            return Err("at-spi tree is not rooted at the issued frame".into());
        }
        Ok(GuestTree {
            window_id: u64::from(window_id),
            max_depth: AX_TREE_BOUNDS.max_depth,
            max_nodes: AX_TREE_BOUNDS.max_nodes,
            max_text: AX_TREE_BOUNDS.max_text,
            truncation: walk.truncation,
            nodes: walk.nodes,
        })
    }

    fn action_index(actions: &[String]) -> usize {
        actions
            .iter()
            .position(|name| {
                let lower = name.to_ascii_lowercase();
                lower == "press" || lower == "click"
            })
            .unwrap_or(0)
    }

    #[derive(Clone, Copy)]
    struct Budget {
        deadline: Instant,
    }

    impl Budget {
        fn start() -> Self {
            Self {
                deadline: Instant::now() + AX_TREE_BOUNDS.max_read(),
            }
        }

        fn remaining(self) -> Result<Duration, ()> {
            self.deadline
                .checked_duration_since(Instant::now())
                .filter(|left| !left.is_zero())
                .ok_or(())
        }
    }

    enum ReadFail {
        Budget,
        Failed(zbus::Error),
    }

    fn read_error(fail: ReadFail) -> String {
        match fail {
            ReadFail::Budget => "accessibility read exceeded its time budget".into(),
            ReadFail::Failed(error) if is_dbus_timeout(&error) => {
                "accessibility read exceeded its time budget".into()
            }
            ReadFail::Failed(error) => error.to_string(),
        }
    }

    fn is_dbus_timeout(error: &zbus::Error) -> bool {
        matches!(error, zbus::Error::InputOutput(io) if io.kind() == std::io::ErrorKind::TimedOut)
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum ActionStage {
        Lost,
        Declined,
        Other,
    }

    impl ActionStage {
        fn lost(self) -> bool {
            self == Self::Lost
        }
    }

    /// Classify the typed error from a call that has already been sent.
    fn action_stage(error: &zbus::Error) -> ActionStage {
        match error {
            zbus::Error::InputOutput(_) | zbus::Error::InvalidReply => ActionStage::Lost,
            zbus::Error::FDO(fdo) => match fdo.as_ref() {
                zbus::fdo::Error::NoReply(_)
                | zbus::fdo::Error::IOError(_)
                | zbus::fdo::Error::Timeout(_)
                | zbus::fdo::Error::TimedOut(_)
                | zbus::fdo::Error::Disconnected(_) => ActionStage::Lost,
                zbus::fdo::Error::UnknownMethod(_)
                | zbus::fdo::Error::UnknownInterface(_)
                | zbus::fdo::Error::NotSupported(_)
                | zbus::fdo::Error::InvalidArgs(_)
                | zbus::fdo::Error::AccessDenied(_) => ActionStage::Declined,
                zbus::fdo::Error::ZBus(inner) => action_stage(inner),
                _ => ActionStage::Other,
            },
            zbus::Error::MethodError(name, _, _) => {
                let name = name.as_str();
                if name.ends_with("NoReply")
                    || name.ends_with("IOError")
                    || name.ends_with("Timeout")
                    || name.ends_with("TimedOut")
                    || name.ends_with("Disconnected")
                {
                    ActionStage::Lost
                } else if name.ends_with("UnknownMethod")
                    || name.ends_with("UnknownInterface")
                    || name.ends_with("NotSupported")
                    || name.ends_with("InvalidArgs")
                    || name.ends_with("AccessDenied")
                {
                    ActionStage::Declined
                } else {
                    ActionStage::Other
                }
            }
            _ => ActionStage::Other,
        }
    }

    fn raced<T>(
        budget: Budget,
        fut: impl Future<Output = Result<T, zbus::Error>>,
    ) -> Result<T, ReadFail> {
        let remaining = budget.remaining().map_err(|_| ReadFail::Budget)?;
        let raced = zbus::block_on(async move {
            let mut call = std::pin::pin!(fut);
            let mut timer = std::pin::pin!(async_io::Timer::after(remaining));
            std::future::poll_fn(|cx| {
                if let Poll::Ready(result) = call.as_mut().poll(cx) {
                    return Poll::Ready(Ok(result));
                }
                if timer.as_mut().poll(cx).is_ready() {
                    return Poll::Ready(Err(ReadFail::Budget));
                }
                Poll::Pending
            })
            .await
        });
        match raced {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(error)) if is_dbus_timeout(&error) => Err(ReadFail::Budget),
            Ok(Err(error)) => Err(ReadFail::Failed(error)),
            Err(fail) => Err(fail),
        }
    }

    fn a11y_connection(budget: Budget) -> Result<zbus::Connection, String> {
        let session = zbus::connection::Builder::session()
            .map_err(|error| format!("session bus: {error}"))?
            .method_timeout(AX_TREE_BOUNDS.max_read());
        let session = raced(budget, session.build()).map_err(read_error)?;
        let bus = proxy(
            &session,
            budget,
            "org.a11y.Bus",
            "/org/a11y/bus",
            "org.a11y.Bus",
        )
        .map_err(read_error)?;
        let address: String = dbus_call(budget, &bus, "GetAddress", &()).map_err(read_error)?;
        let a11y = zbus::connection::Builder::address(address.as_str())
            .map_err(|error| format!("at-spi address: {error}"))?
            .method_timeout(AX_TREE_BOUNDS.max_read());
        raced(budget, a11y.build()).map_err(read_error)
    }

    fn proxy<'a>(
        conn: &zbus::Connection,
        budget: Budget,
        dest: &'a str,
        path: &'a str,
        interface: &'a str,
    ) -> Result<zbus::Proxy<'a>, ReadFail> {
        raced(budget, zbus::Proxy::new(conn, dest, path, interface))
    }

    fn dbus_call<B, R>(
        budget: Budget,
        proxy: &zbus::Proxy<'_>,
        method: &'static str,
        body: &B,
    ) -> Result<R, ReadFail>
    where
        B: serde::Serialize + zbus::zvariant::DynamicType,
        R: for<'de> zbus::zvariant::DynamicDeserialize<'de>,
    {
        raced(budget, proxy.call(method, body))
    }

    struct FrameHit {
        dest: String,
        path: OwnedObjectPath,
    }

    fn locate_frame(
        conn: &zbus::Connection,
        budget: Budget,
        window: &RawWindow,
    ) -> Result<FrameHit, String> {
        let apps = firefox_applications(conn, budget)?;
        let mut hits = Vec::new();
        let mut bound = false;
        for (dest, path) in apps {
            let pid = application_pid(conn, budget, &dest).map_err(read_error)?;
            if pid != window.pid {
                continue;
            }
            let token = start_token(pid)
                .ok_or_else(|| "issued process start token is missing".to_string())?;
            if token != window.start_token {
                return Err("issued process start token changed".into());
            }
            bound = true;
            let mut seen = 0u32;
            collect_frames(
                conn,
                budget,
                &dest,
                path.as_str(),
                0,
                window,
                &mut hits,
                &mut seen,
            )?;
        }
        if !bound {
            return Err("at-spi frame is not bound to the issued firefox process".into());
        }
        match hits.len() {
            1 => hits
                .pop()
                .ok_or_else(|| "no at-spi frame matches the issued window".to_string()),
            0 => Err("no at-spi frame matches the issued window".into()),
            count => Err(format!(
                "multiple at-spi frames match the issued window: {count}"
            )),
        }
    }

    fn collect_frames(
        conn: &zbus::Connection,
        budget: Budget,
        dest: &str,
        path: &str,
        depth: u32,
        window: &RawWindow,
        hits: &mut Vec<FrameHit>,
        seen: &mut u32,
    ) -> Result<(), String> {
        if *seen >= AX_TREE_BOUNDS.max_nodes {
            return Err("accessibility tree exceeded its node budget during frame match".into());
        }
        *seen += 1;
        if budget.remaining().is_err() {
            return Err("accessibility read exceeded its time budget".into());
        }
        if depth > 0 {
            let role = role_name(conn, budget, dest, path).map_err(read_error)?;
            if role.eq_ignore_ascii_case("frame") || role.eq_ignore_ascii_case("window") {
                if let Some(screen) = extents(conn, budget, dest, path).map_err(read_error)? {
                    if geometry_match(screen, window) {
                        let object =
                            OwnedObjectPath::try_from(path).map_err(|error| error.to_string())?;
                        hits.push(FrameHit {
                            dest: dest.to_string(),
                            path: object,
                        });
                    }
                }
            }
        }
        if depth >= AX_TREE_BOUNDS.max_depth {
            return Ok(());
        }
        let children = children_of(conn, budget, dest, path).map_err(read_error)?;
        for (child_dest, child_path) in children {
            collect_frames(
                conn,
                budget,
                &child_dest,
                child_path.as_str(),
                depth + 1,
                window,
                hits,
                seen,
            )?;
        }
        Ok(())
    }

    fn geometry_match(screen: (i32, i32, i32, i32), window: &RawWindow) -> bool {
        let Ok(width) = i32::try_from(window.width) else {
            return false;
        };
        let Ok(height) = i32::try_from(window.height) else {
            return false;
        };
        screen == (window.x, window.y, width, height)
    }

    fn window_local(screen: (i32, i32, i32, i32), origin_x: i32, origin_y: i32) -> [f64; 4] {
        [
            (i64::from(screen.0) - i64::from(origin_x)) as f64,
            (i64::from(screen.1) - i64::from(origin_y)) as f64,
            f64::from(screen.2),
            f64::from(screen.3),
        ]
    }

    fn application_pid(
        conn: &zbus::Connection,
        budget: Budget,
        dest: &str,
    ) -> Result<u32, ReadFail> {
        // Gecko's Application.Id is not the window process (Firefox ESR 153
        // reports 1). The bus credentials of the application object are.
        let bus = proxy(
            conn,
            budget,
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
        )?;
        let pid: u32 = dbus_call(budget, &bus, "GetConnectionUnixProcessID", &(dest,))?;
        if pid == 0 {
            return Err(ReadFail::Failed(zbus::Error::Failure(
                "at-spi application connection has no unix process".into(),
            )));
        }
        Ok(pid)
    }

    struct Located {
        dest: String,
        object: OwnedObjectPath,
        path: Vec<u32>,
        role: String,
        actions: Vec<String>,
    }

    struct Walk {
        nodes: Vec<GuestNode>,
        truncation: GuestTruncation,
        located: Option<Located>,
        want: Option<Vec<u32>>,
    }

    impl Default for Walk {
        fn default() -> Self {
            Self {
                nodes: Vec::new(),
                truncation: GuestTruncation::default(),
                located: None,
                want: None,
            }
        }
    }

    impl Walk {
        fn visit(
            &mut self,
            conn: &zbus::Connection,
            budget: Budget,
            dest: &str,
            path: &str,
            index_path: Vec<u32>,
            depth: u32,
            origin: (i32, i32),
        ) -> Result<(), String> {
            if budget.remaining().is_err() {
                return Err("accessibility read exceeded its time budget".into());
            }
            if self.nodes.len() as u32 >= AX_TREE_BOUNDS.max_nodes {
                self.truncation.nodes = true;
                return Ok(());
            }
            let children = children_of(conn, budget, dest, path).map_err(read_error)?;
            let (name, name_truncated, name_unreadable) =
                read_name(conn, budget, dest, path).map_err(read_error)?;
            if name_truncated {
                self.truncation.text = true;
            }
            if name_unreadable {
                self.truncation.names = true;
            }
            let role = role_name(conn, budget, dest, path).map_err(read_error)?;
            let (enabled, focused) = state_flags(conn, budget, dest, path).map_err(read_error)?;
            let frame = extents(conn, budget, dest, path)
                .map_err(read_error)?
                .map(|screen| window_local(screen, origin.0, origin.1));
            let actions = list_actions(conn, budget, dest, path).map_err(read_error)?;
            if self.want.as_ref().is_some_and(|want| want == &index_path) {
                let object = OwnedObjectPath::try_from(path).map_err(|error| error.to_string())?;
                self.located = Some(Located {
                    dest: dest.to_string(),
                    object,
                    path: index_path.clone(),
                    role: role.clone(),
                    actions: actions.clone(),
                });
            }
            let depth_limited = depth >= AX_TREE_BOUNDS.max_depth && !children.is_empty();
            if depth_limited {
                self.truncation.depth = true;
            }
            self.nodes.push(GuestNode {
                path: index_path.clone(),
                depth,
                role,
                name,
                name_truncated,
                name_unreadable,
                enabled,
                focused,
                frame,
                actions,
                children: u32::try_from(children.len()).unwrap_or(u32::MAX),
                depth_limited,
            });
            if depth_limited {
                return Ok(());
            }
            for (index, (child_dest, child_path)) in children.into_iter().enumerate() {
                if self.nodes.len() as u32 >= AX_TREE_BOUNDS.max_nodes {
                    self.truncation.nodes = true;
                    break;
                }
                let mut child_index = index_path.clone();
                child_index.push(u32::try_from(index).unwrap_or(u32::MAX));
                self.visit(
                    conn,
                    budget,
                    &child_dest,
                    child_path.as_str(),
                    child_index,
                    depth + 1,
                    origin,
                )?;
            }
            Ok(())
        }
    }

    fn firefox_applications(
        conn: &zbus::Connection,
        budget: Budget,
    ) -> Result<Vec<(String, OwnedObjectPath)>, String> {
        let root_children = children_of(
            conn,
            budget,
            "org.a11y.atspi.Registry",
            "/org/a11y/atspi/accessible/root",
        )
        .map_err(read_error)?;
        let mut apps = Vec::new();
        for (dest, path) in &root_children {
            let role = role_name(conn, budget, dest, path.as_str()).map_err(read_error)?;
            if role == "application" {
                apps.push((dest.clone(), path.clone()));
            } else {
                let nested = children_of(conn, budget, dest, path.as_str()).map_err(read_error)?;
                apps.extend(nested);
            }
        }
        let mut found = Vec::new();
        for (dest, path) in apps {
            let (name, _, unreadable) =
                read_name(conn, budget, &dest, path.as_str()).map_err(read_error)?;
            let toolkit = match property_string(
                conn,
                budget,
                &dest,
                path.as_str(),
                "org.a11y.atspi.Application",
                "ToolkitName",
            ) {
                Ok(value) => value,
                Err(ReadFail::Budget) => return Err(read_error(ReadFail::Budget)),
                Err(ReadFail::Failed(_)) => None,
            };
            if unreadable && toolkit.is_none() {
                return Err("at-spi application identity is unreadable".into());
            }
            let name = name.as_deref().unwrap_or("");
            let toolkit = toolkit.as_deref().unwrap_or("");
            let firefox = name.to_ascii_lowercase().contains("firefox")
                || toolkit.eq_ignore_ascii_case("gecko");
            if firefox {
                found.push((dest, path));
            }
        }
        if found.is_empty() {
            return Err("at-spi has no firefox application".into());
        }
        Ok(found)
    }

    fn children_of(
        conn: &zbus::Connection,
        budget: Budget,
        dest: &str,
        path: &str,
    ) -> Result<Vec<(String, OwnedObjectPath)>, ReadFail> {
        let accessible = proxy(conn, budget, dest, path, "org.a11y.atspi.Accessible")?;
        match dbus_call(budget, &accessible, "GetChildren", &()) {
            Ok(children) => Ok(children),
            Err(ReadFail::Failed(error)) if action_stage(&error) == ActionStage::Declined => {
                let count = owned_i64(property(
                    conn,
                    budget,
                    dest,
                    path,
                    "org.a11y.atspi.Accessible",
                    "ChildCount",
                )?)?;
                if count < 0 {
                    return Err(ReadFail::Failed(zbus::Error::Failure(
                        "at-spi child count is negative".into(),
                    )));
                }
                let mut children = Vec::new();
                let limit = (count as usize).min(AX_TREE_BOUNDS.max_nodes as usize);
                for index in 0..limit {
                    let child =
                        dbus_call(budget, &accessible, "GetChildAtIndex", &(index as i32,))?;
                    children.push(child);
                }
                Ok(children)
            }
            Err(other) => Err(other),
        }
    }

    fn role_name(
        conn: &zbus::Connection,
        budget: Budget,
        dest: &str,
        path: &str,
    ) -> Result<String, ReadFail> {
        let accessible = proxy(conn, budget, dest, path, "org.a11y.atspi.Accessible")?;
        dbus_call(budget, &accessible, "GetRoleName", &())
    }

    fn read_name(
        conn: &zbus::Connection,
        budget: Budget,
        dest: &str,
        path: &str,
    ) -> Result<(Option<String>, bool, bool), ReadFail> {
        match property_string(
            conn,
            budget,
            dest,
            path,
            "org.a11y.atspi.Accessible",
            "Name",
        ) {
            Ok(Some(name)) => {
                let mut chars = name.chars();
                let kept: String = chars.by_ref().take(AX_TREE_BOUNDS.max_text).collect();
                let truncated = chars.next().is_some();
                if kept.is_empty() {
                    Ok((None, false, false))
                } else {
                    Ok((Some(kept), truncated, false))
                }
            }
            Ok(None) => Ok((None, false, false)),
            Err(ReadFail::Budget) => Err(ReadFail::Budget),
            Err(ReadFail::Failed(_)) => Ok((None, false, true)),
        }
    }

    fn state_flags(
        conn: &zbus::Connection,
        budget: Budget,
        dest: &str,
        path: &str,
    ) -> Result<(Option<bool>, Option<bool>), ReadFail> {
        let accessible = proxy(conn, budget, dest, path, "org.a11y.atspi.Accessible")?;
        match dbus_call::<_, (Vec<u32>,)>(budget, &accessible, "GetState", &()) {
            Ok((states,)) => Ok((Some(state_bit(&states, 8)), Some(state_bit(&states, 12)))),
            Err(ReadFail::Budget) => Err(ReadFail::Budget),
            Err(ReadFail::Failed(_)) => Ok((None, None)),
        }
    }

    fn read_fail_message(fail: ReadFail) -> String {
        match fail {
            ReadFail::Budget => "accessibility read exceeded its time budget".into(),
            ReadFail::Failed(error) => error.to_string(),
        }
    }

    fn state_bit(states: &[u32], bit: u32) -> bool {
        let index = (bit / 32) as usize;
        let mask = 1u32 << (bit % 32);
        states.get(index).is_some_and(|word| word & mask != 0)
    }

    fn extents(
        conn: &zbus::Connection,
        budget: Budget,
        dest: &str,
        path: &str,
    ) -> Result<Option<(i32, i32, i32, i32)>, ReadFail> {
        let component = proxy(conn, budget, dest, path, "org.a11y.atspi.Component")?;
        match dbus_call::<_, (i32, i32, i32, i32)>(budget, &component, "GetExtents", &(0u32,)) {
            Ok(screen) => Ok(Some(screen)),
            Err(ReadFail::Budget) => Err(ReadFail::Budget),
            Err(ReadFail::Failed(_)) => Ok(None),
        }
    }

    fn list_actions(
        conn: &zbus::Connection,
        budget: Budget,
        dest: &str,
        path: &str,
    ) -> Result<Vec<String>, ReadFail> {
        let action = match proxy(conn, budget, dest, path, "org.a11y.atspi.Action") {
            Ok(action) => action,
            Err(ReadFail::Budget) => return Err(ReadFail::Budget),
            Err(ReadFail::Failed(_)) => return Ok(Vec::new()),
        };
        let count: i32 = match dbus_call(budget, &action, "GetNActions", &()) {
            Ok(count) => count,
            Err(ReadFail::Budget) => return Err(ReadFail::Budget),
            Err(ReadFail::Failed(error)) if action_stage(&error) == ActionStage::Declined => {
                // Gecko exposes NActions as a property and does not implement
                // GetNActions. A missing count is no actions, not a tree failure.
                let value = match property(
                    conn,
                    budget,
                    dest,
                    path,
                    "org.a11y.atspi.Action",
                    "NActions",
                ) {
                    Ok(value) => value,
                    Err(ReadFail::Budget) => return Err(ReadFail::Budget),
                    Err(ReadFail::Failed(_)) => return Ok(Vec::new()),
                };
                let Ok(count) = owned_i64(value) else {
                    return Ok(Vec::new());
                };
                i32::try_from(count).unwrap_or(0)
            }
            Err(ReadFail::Failed(_)) => return Ok(Vec::new()),
        };
        if count <= 0 {
            return Ok(Vec::new());
        }
        let mut names = Vec::new();
        let limit = (count as usize).min(32);
        for index in 0..limit {
            match dbus_call::<_, String>(budget, &action, "GetName", &(index as i32,)) {
                Ok(name) => names.push(name),
                Err(ReadFail::Budget) => return Err(ReadFail::Budget),
                Err(ReadFail::Failed(_)) => return Ok(Vec::new()),
            }
        }
        Ok(names)
    }

    fn do_action(
        conn: &zbus::Connection,
        budget: Budget,
        dest: &str,
        path: &str,
        index: i32,
    ) -> Result<(), String> {
        if budget.remaining().is_err() {
            return Err("accessibility read exceeded its time budget".into());
        }
        let action =
            proxy(conn, budget, dest, path, "org.a11y.atspi.Action").map_err(read_error)?;
        if budget.remaining().is_err() {
            return Err("accessibility read exceeded its time budget".into());
        }
        match dbus_call::<_, bool>(budget, &action, "DoAction", &(index,)) {
            Ok(true) => Ok(()),
            Ok(false) => Err("at-spi action was declined".into()),
            Err(ReadFail::Budget) => Err("action timed out".into()),
            Err(ReadFail::Failed(error)) if action_stage(&error).lost() => {
                Err("action receipt lost".into())
            }
            Err(ReadFail::Failed(error)) if action_stage(&error) == ActionStage::Declined => {
                Err(format!("at-spi action was declined: {error}"))
            }
            Err(ReadFail::Failed(error)) => Err(format!("at-spi action: {error}")),
        }
    }

    fn property_string(
        conn: &zbus::Connection,
        budget: Budget,
        dest: &str,
        path: &str,
        interface: &str,
        name: &str,
    ) -> Result<Option<String>, ReadFail> {
        let value = property(conn, budget, dest, path, interface, name)?;
        match String::try_from(value) {
            Ok(text) if text.is_empty() => Ok(None),
            Ok(text) => Ok(Some(text)),
            Err(_) => Err(ReadFail::Failed(zbus::Error::Failure(
                "at-spi property is not a string".into(),
            ))),
        }
    }

    fn property(
        conn: &zbus::Connection,
        budget: Budget,
        dest: &str,
        path: &str,
        interface: &str,
        name: &str,
    ) -> Result<OwnedValue, ReadFail> {
        let props = proxy(conn, budget, dest, path, "org.freedesktop.DBus.Properties")?;
        dbus_call(budget, &props, "Get", &(interface, name))
    }

    fn owned_i64(value: OwnedValue) -> Result<i64, ReadFail> {
        if let Ok(number) = i32::try_from(value.clone()) {
            return Ok(i64::from(number));
        }
        if let Ok(number) = u32::try_from(value.clone()) {
            return Ok(i64::from(number));
        }
        i64::try_from(value).map_err(|_| {
            ReadFail::Failed(zbus::Error::Failure(
                "at-spi property is not an integer".into(),
            ))
        })
    }

    #[cfg(test)]
    mod action_stage_tests {
        use super::{action_stage, ActionStage};
        use std::io;
        use std::sync::Arc;

        #[test]
        fn typed_loss_stays_lost_and_unknown_method_stays_declined() {
            let lost = [
                zbus::Error::FDO(Box::new(zbus::fdo::Error::NoReply("firefox exited".into()))),
                zbus::Error::FDO(Box::new(zbus::fdo::Error::IOError("socket closed".into()))),
                zbus::Error::InputOutput(Arc::new(io::Error::new(
                    io::ErrorKind::ConnectionReset,
                    "transport closed",
                ))),
            ];
            for error in lost {
                assert!(
                    action_stage(&error).lost(),
                    "{error} must stay a lost receipt"
                );
            }
            let declined = zbus::Error::FDO(Box::new(zbus::fdo::Error::UnknownMethod(
                "no action".into(),
            )));
            assert_eq!(action_stage(&declined), ActionStage::Declined);
        }
    }

    #[cfg(test)]
    mod tree_reply_tests {
        use super::{tree_children, tree_parent};

        #[test]
        fn query_tree_parent_is_not_the_root_and_a_short_child_list_is_rejected() {
            let mut reply = vec![0u8; 32];
            reply[8..12].copy_from_slice(&0x519u32.to_le_bytes());
            reply[12..16].copy_from_slice(&0x200359u32.to_le_bytes());
            reply[16..18].copy_from_slice(&1u16.to_le_bytes());
            assert!(
                tree_parent(&reply[..15]).is_err(),
                "the parent field needs bytes 12..16"
            );
            assert_eq!(tree_parent(&reply).unwrap(), 0x200359);
            assert!(
                tree_children(&reply).is_err(),
                "a declared child that does not fit the reply is not a partial tree"
            );
            reply.extend_from_slice(&0x800003u32.to_le_bytes());
            assert_eq!(tree_children(&reply).unwrap(), vec![0x800003]);
        }
    }
}

#[cfg(all(test, not(target_os = "linux")))]
mod generation_rejection {
    use super::{issue_generation, WindowGen};

    fn fresh() -> WindowGen {
        WindowGen {
            generation: 0,
            live: false,
            pid: 0,
            start_token: 0,
        }
    }

    #[test]
    fn same_xid_new_process_rejects_the_issued_generation() {
        let mut entry = fresh();
        let nonce = 0x51CE_0001;
        let epoch = 3;
        let issued = issue_generation(&mut entry, 10, 100, nonce, epoch);
        assert_eq!(issue_generation(&mut entry, 10, 100, nonce, epoch), issued);
        let new_pid = issue_generation(&mut entry, 11, 100, nonce, epoch);
        assert_ne!(
            new_pid, issued,
            "a live xid bound to a new pid must not keep the old generation"
        );
        let new_token = issue_generation(&mut entry, 11, 101, nonce, epoch);
        assert_ne!(
            new_token, new_pid,
            "a live xid bound to a new start token must not keep the old generation"
        );
        assert_eq!(
            issue_generation(&mut entry, 11, 101, nonce, epoch),
            new_token
        );
        entry.live = false;
        let returned = issue_generation(&mut entry, 11, 101, nonce, epoch);
        assert_ne!(
            returned, new_token,
            "a retired xid still advances when it returns"
        );
        let next_epoch = issue_generation(&mut entry, 11, 101, nonce, epoch + 1);
        assert_ne!(
            next_epoch, returned,
            "guest epoch still changes the issued generation"
        );
    }
}
