use super::sway::{self, CompositorInstance, WindowLedger};
use super::wire::{split_event, Connection, Message};
use super::*;
use serde_json::json;
use std::io::Read;
use std::os::fd::RawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::time::Duration;

fn global(name: u32, interface: &str, version: u32) -> Global {
    Global {
        name,
        interface: interface.into(),
        version,
    }
}

fn full_globals() -> Vec<Global> {
    vec![
        global(1, "wl_shm", 2),
        global(2, "wl_seat", 9),
        global(3, "wl_output", 4),
        global(4, "zwlr_screencopy_manager_v1", 3),
        global(5, "zwlr_virtual_pointer_manager_v1", 2),
        global(6, "zwp_virtual_keyboard_manager_v1", 1),
    ]
}

#[test]
fn an_unset_display_is_refused_not_defaulted() {
    assert!(matches!(
        display_socket(None, Some("/run/user/1000")),
        Err(SeatError::NoDisplay(_))
    ));
    assert!(matches!(
        display_socket(Some(""), Some("/run/user/1000")),
        Err(SeatError::NoDisplay(_))
    ));
    assert!(matches!(
        display_socket(Some("wayland-1"), None),
        Err(SeatError::NoDisplay(_))
    ));
    assert!(matches!(
        display_socket(Some("wayland-1"), Some("rel")),
        Err(SeatError::NoDisplay(_))
    ));
    assert_eq!(
        display_socket(Some("wayland-1"), Some("/run/user/1000")).unwrap(),
        PathBuf::from("/run/user/1000/wayland-1")
    );
    assert_eq!(
        display_socket(Some("/tmp/w"), None).unwrap(),
        PathBuf::from("/tmp/w")
    );
}

#[test]
fn wire_messages_round_trip_and_malformed_headers_are_refused() {
    let bytes = Message::new(7, 3)
        .uint(42)
        .int(-5)
        .string("wl_seat")
        .encode()
        .unwrap();
    assert_eq!(bytes.len() % 4, 0);
    assert_eq!(split_event(&bytes[..bytes.len() - 1]).unwrap(), None);
    let (event, used) = split_event(&bytes).unwrap().unwrap();
    assert_eq!((event.object, event.opcode, used), (7, 3, bytes.len()));
    let mut args = event.args();
    assert_eq!(args.uint().unwrap(), 42);
    assert_eq!(args.int().unwrap(), -5);
    assert_eq!(args.string().unwrap(), "wl_seat");
    assert!(args.uint().is_err(), "reading past the body fails");

    let header = |size: u32| {
        let mut out = 1u32.to_ne_bytes().to_vec();
        out.extend_from_slice(&(size << 16).to_ne_bytes());
        out.resize(size.max(8) as usize, 0);
        out
    };
    for size in [4, 10, 4100] {
        assert!(
            matches!(split_event(&header(size)), Err(SeatError::Protocol(_))),
            "size {size}"
        );
    }
    let unterminated = Message::new(1, 0)
        .uint(4)
        .uint(u32::from_ne_bytes(*b"abcd"))
        .encode()
        .unwrap();
    let (event, _) = split_event(&unterminated).unwrap().unwrap();
    assert!(matches!(event.args().string(), Err(SeatError::Protocol(_))));
    let oversized = Message::new(1, 0).string(&"x".repeat(5000));
    assert!(matches!(oversized.encode(), Err(SeatError::Protocol(_))));
}

fn recv_fd(socket: &UnixStream, buffer: &mut [u8]) -> (usize, Option<OwnedFd>) {
    let mut iov = libc::iovec {
        iov_base: buffer.as_mut_ptr().cast(),
        iov_len: buffer.len(),
    };
    let mut control = [0u64; 8];
    let mut header: libc::msghdr = unsafe { std::mem::zeroed() };
    header.msg_iov = &mut iov;
    header.msg_iovlen = 1;
    header.msg_control = control.as_mut_ptr().cast();
    header.msg_controllen = std::mem::size_of_val(&control) as _;
    let read = unsafe { libc::recvmsg(socket.as_raw_fd(), &mut header, 0) };
    assert!(read >= 0);
    let cmsg = unsafe { libc::CMSG_FIRSTHDR(&header) };
    let fd = (!cmsg.is_null()).then(|| unsafe {
        OwnedFd::from_raw_fd(libc::CMSG_DATA(cmsg).cast::<RawFd>().read_unaligned())
    });
    (read as usize, fd)
}

#[test]
fn descriptors_travel_with_their_request() {
    let (client, server) = UnixStream::pair().unwrap();
    let mut conn = Connection::from_stream(client, Duration::from_secs(2)).unwrap();
    let file = memfd("pawork-test", 5).unwrap();
    file.write_all_at(b"keys\0", 0).unwrap();
    conn.send(&Message::new(9, 0).uint(1).fd(file.as_raw_fd()).uint(5))
        .unwrap();
    drop(file);
    let mut buffer = [0u8; 64];
    let (read, fd) = recv_fd(&server, &mut buffer);
    let (event, used) = split_event(&buffer[..read]).unwrap().unwrap();
    assert_eq!((event.object, event.opcode, used), (9, 0, read));
    let mut args = event.args();
    assert_eq!((args.uint().unwrap(), args.uint().unwrap()), (1, 5));
    let received = std::fs::File::from(fd.expect("SCM_RIGHTS descriptor"));
    let mut text = [0u8; 5];
    received.read_exact_at(&mut text, 0).unwrap();
    assert_eq!(&text, b"keys\0");
}

#[test]
fn globals_need_exactly_one_seat_output_and_each_protocol() {
    let selected = select_globals(&full_globals()).unwrap();
    assert_eq!(selected.shm, (1, 1));
    assert_eq!(selected.seat, (2, 1));
    assert_eq!(selected.output, (3, 2));
    assert_eq!(selected.screencopy, (4, 3));
    assert_eq!(selected.pointer, (5, 1));
    for missing in [
        "wl_shm",
        "wl_seat",
        "wl_output",
        "zwlr_screencopy_manager_v1",
        "zwlr_virtual_pointer_manager_v1",
        "zwp_virtual_keyboard_manager_v1",
    ] {
        let globals: Vec<_> = full_globals()
            .into_iter()
            .filter(|global| global.interface != missing)
            .collect();
        assert!(
            matches!(select_globals(&globals), Err(SeatError::Globals(_))),
            "{missing}"
        );
    }
    for duplicate in ["wl_seat", "wl_output"] {
        let mut globals = full_globals();
        globals.push(global(40, duplicate, 1));
        assert!(
            matches!(select_globals(&globals), Err(SeatError::Globals(_))),
            "{duplicate}"
        );
    }
}

#[test]
fn buffer_offers_are_bounded_and_channels_follow_the_format() {
    assert!(checked_buffer(0, 10, 40).is_err());
    assert!(checked_buffer(4097, 1, 4097 * 4).is_err());
    assert!(
        checked_buffer(4096, 2000, 4096 * 4).is_err(),
        "over 8M pixels"
    );
    assert!(checked_buffer(10, 10, 39).is_err(), "stride below width*4");
    assert_eq!(checked_buffer(2, 2, 12).unwrap(), 24);

    // 2x2 with a padded stride; memory order B,G,R,X for XRGB8888.
    let data = [
        1, 2, 3, 0, 4, 5, 6, 0, 9, 9, 9, 9, //
        7, 8, 9, 0, 10, 11, 12, 0, 9, 9, 9, 9,
    ];
    assert_eq!(
        shm_to_rgb(SHM_XRGB8888, 2, 2, 12, false, &data).unwrap(),
        vec![3, 2, 1, 6, 5, 4, 9, 8, 7, 12, 11, 10]
    );
    assert_eq!(
        shm_to_rgb(SHM_XBGR8888, 2, 2, 12, true, &data).unwrap(),
        vec![7, 8, 9, 10, 11, 12, 1, 2, 3, 4, 5, 6]
    );
    assert!(
        shm_to_rgb(0x3036_3452, 2, 2, 12, false, &data).is_err(),
        "unknown format"
    );
    assert!(shm_to_rgb(SHM_XRGB8888, 2, 2, 12, false, &data[..20]).is_err());
}

#[test]
fn frames_encode_within_the_image_budget() {
    let frame = SeatFrame {
        width: 2000,
        height: 1000,
        rgb: (0..2000 * 1000 * 3).map(|i| (i * 7 % 251) as u8).collect(),
    };
    let (jpeg, width, height) = frame.jpeg().unwrap();
    assert_eq!((width, height), (1280, 640));
    assert!(jpeg.len() <= crate::MAX_IMAGE_BYTES);
    let decoded = image::load_from_memory(&jpeg).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (1280, 640));
}

#[test]
fn text_is_validated_in_full_before_any_event() {
    let plan = text_plan("Hi! ~").unwrap();
    assert_eq!(plan.len(), 5);
    assert_eq!(plan[0], keymap::char_keycode('H').unwrap());
    for rejected in ["", "caf\u{e9}", "a\nb", "tab\t"] {
        assert!(
            matches!(text_plan(rejected), Err(SeatError::Input(_))),
            "{rejected:?}"
        );
    }
    assert!(text_plan(&"a".repeat(4096)).is_ok());
    assert!(text_plan(&"a".repeat(4097)).is_err());
}

#[test]
fn keymap_defines_every_planned_key_exactly_once() {
    let text = keymap::keymap_text();
    let mut codes: Vec<u32> = (' '..='~')
        .map(|c| keymap::char_keycode(c).unwrap())
        .collect();
    for name in [
        "return",
        "tab",
        "space",
        "backspace",
        "escape",
        "delete",
        "home",
        "end",
        "pageup",
        "pagedown",
        "left",
        "up",
        "right",
        "down",
    ] {
        codes.push(keymap::key_keycode(SeatKey::from_name(name).unwrap()));
    }
    let unique: std::collections::HashSet<_> = codes.iter().collect();
    assert_eq!(unique.len(), codes.len(), "keycodes collide");
    for code in codes {
        assert_eq!(
            text.matches(&format!("<K{code}> = {};", code + 8)).count(),
            1
        );
        assert_eq!(text.matches(&format!("key <K{code}> {{")).count(), 1);
    }
    assert!(
        text.contains("key <K66> { [ a ] };"),
        "'a' is 0x61 - 0x20 + 1"
    );
    assert!(text.contains("modifier_map Mod4 { <LWIN> };"));
    assert!(!text.contains('\0'));
    assert_eq!(SeatKey::from_name("F13"), None);
    let all = SeatModifiers {
        shift: true,
        control: true,
        alt: true,
        super_key: true,
    };
    assert_eq!(all.mask(), 1 | 4 | 8 | 64);
}

#[test]
fn sway_ipc_frames_round_trip_and_bad_replies_are_refused() {
    let request = sway::encode_request(sway::GET_TREE, "");
    assert_eq!(&request[..6], b"i3-ipc");
    assert_eq!(request.len(), 14);

    let reply = |magic: &[u8], kind: u32, payload: &[u8], len: u32| {
        let mut out = magic.to_vec();
        out.extend_from_slice(&len.to_ne_bytes());
        out.extend_from_slice(&kind.to_ne_bytes());
        out.extend_from_slice(payload);
        out
    };
    let ok = reply(b"i3-ipc", 4, br#"{"id":1}"#, 8);
    assert_eq!(
        sway::read_reply(&mut ok.as_slice(), 4).unwrap(),
        json!({"id": 1})
    );
    let magic = reply(b"i3-ipX", 4, b"{}", 2);
    assert!(sway::read_reply(&mut magic.as_slice(), 4).is_err());
    let kind = reply(b"i3-ipc", 3, b"{}", 2);
    assert!(sway::read_reply(&mut kind.as_slice(), 4).is_err());
    let huge = reply(b"i3-ipc", 4, b"", 64 * 1024 * 1024);
    assert!(sway::read_reply(&mut huge.as_slice(), 4).is_err());
}

fn toplevel(id: u64, shell: &str, app_id: Option<&str>) -> serde_json::Value {
    json!({
        "id": id, "type": "con", "pid": std::process::id(), "shell": shell,
        "app_id": app_id, "visible": true, "focused": false,
        "rect": {"x": 0, "y": 0, "width": 640, "height": 480}, "nodes": []
    })
}

#[test]
fn windows_need_native_identity_from_the_compositor() {
    let tree = json!({"id": 1, "type": "root", "nodes": [{"id": 2, "type": "output",
        "nodes": [{"id": 3, "type": "workspace", "nodes": [toplevel(5, "xdg_shell", Some("foot"))],
        "floating_nodes": [toplevel(6, "xdg_shell", Some("zenity"))]}]}]});
    let windows = sway::windows(&tree).unwrap();
    assert_eq!(windows.len(), 2);
    assert_eq!((windows[0].con_id, windows[0].app_id.as_str()), (5, "foot"));
    assert_eq!(windows[0].pid, std::process::id());
    assert_eq!((windows[1].con_id, windows[1].rect.width), (6, 640));

    let x11 = json!({"type": "root", "nodes": [toplevel(7, "xwayland", Some("xterm"))]});
    assert!(matches!(
        sway::windows(&x11),
        Err(SeatError::NotDedicated(_))
    ));
    let anonymous = json!({"type": "root", "nodes": [toplevel(8, "xdg_shell", None)]});
    assert!(matches!(sway::windows(&anonymous), Err(SeatError::Ipc(_))));
}

#[test]
fn a_seat_with_shared_hardware_is_refused() {
    let headless = json!([{"name": "HEADLESS-1"}]);
    let virtual_inputs = json!([
        {"identifier": "0:0:wlr_virtual_pointer_v1"},
        {"identifier": "0:0:wlr_virtual_keyboard_v1"}
    ]);
    let one_seat = json!([{"name": "seat0"}]);
    assert_eq!(
        sway::check_dedicated(&headless, &virtual_inputs, &one_seat).unwrap(),
        vec!["0:0:wlr_virtual_pointer_v1", "0:0:wlr_virtual_keyboard_v1"]
    );
    assert!(sway::check_dedicated(&headless, &json!([]), &one_seat)
        .unwrap()
        .is_empty());

    for outputs in [
        json!([{"name": "eDP-1"}]),
        json!([{"name": "WL-1"}]),
        json!([]),
        json!([{"name": "HEADLESS-1"}, {"name": "HEADLESS-2"}]),
    ] {
        assert!(matches!(
            sway::check_dedicated(&outputs, &virtual_inputs, &one_seat),
            Err(SeatError::NotDedicated(_))
        ));
    }
    let libinput = json!([{"identifier": "1:1:AT_Translated_Set_2_keyboard"}]);
    assert!(sway::check_dedicated(&headless, &libinput, &one_seat).is_err());
    let two_seats = json!([{"name": "seat0"}, {"name": "seat1"}]);
    assert!(sway::check_dedicated(&headless, &virtual_inputs, &two_seats).is_err());
}

#[test]
fn process_start_survives_hostile_command_names() {
    let tail = "S 1 1 1 0 -1 4194304 0 0 0 0 0 0 0 0 20 0 1 0 987654 0";
    assert_eq!(
        sway::parse_start_ticks(&format!("42 (sway) {tail}")),
        Some(987654)
    );
    assert_eq!(
        sway::parse_start_ticks(&format!("42 (a) b) (c) {tail}")),
        Some(987654)
    );
    assert_eq!(sway::parse_start_ticks("42 (sway) S 1"), None);
    assert!(sway::process_start(std::process::id()).is_some());
}

#[test]
fn ledger_forgets_vanished_windows_and_resets_on_compositor_restart() {
    let window =
        sway::windows(&json!({"type": "root", "nodes": [toplevel(5, "xdg_shell", Some("foot"))]}))
            .unwrap();
    let first = CompositorInstance {
        pid: 10,
        start_token: 1,
    };
    let mut ledger = WindowLedger::default();
    let generation = |entries: Vec<(sway::SwayWindow, crate::target::ProcessInstance, u64)>| {
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].1.pid, std::process::id());
        entries[0].2
    };
    let original = generation(ledger.sync(first, &window));
    assert_eq!(
        generation(ledger.sync(first, &window)),
        original,
        "stable while present"
    );
    assert!(ledger.sync(first, &[]).is_empty());
    let reappeared = generation(ledger.sync(first, &window));
    assert_ne!(
        reappeared, original,
        "a forgotten id never regains its generation"
    );
    let restarted = CompositorInstance {
        pid: 10,
        start_token: 2,
    };
    assert_ne!(generation(ledger.sync(restarted, &window)), reappeared);
}

/// One-connection fake compositor: answers the registry round-trip with
/// `reply`, then records every request until the client hangs up.
fn fake_compositor(
    reply: impl FnOnce(u32, u32) -> Vec<u8> + Send + 'static,
) -> (PathBuf, std::thread::JoinHandle<Vec<(u32, u16)>>) {
    let dir = std::env::temp_dir().join(format!(
        "pawork-cu26-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("wayland-test");
    let listener = UnixListener::bind(&path).unwrap();
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut inbox = Vec::new();
        let mut requests = Vec::new();
        let mut reply = Some(reply);
        let mut registry = 0;
        let mut chunk = [0u8; 4096];
        loop {
            while let Some((event, used)) = split_event(&inbox).unwrap() {
                inbox.drain(..used);
                requests.push((event.object, event.opcode));
                if event.object == 1 && event.opcode == 1 {
                    registry = event.args().uint().unwrap();
                }
                if event.object == 1 && event.opcode == 0 {
                    let callback = event.args().uint().unwrap();
                    if let Some(reply) = reply.take() {
                        use std::io::Write;
                        stream.write_all(&reply(registry, callback)).unwrap();
                    }
                }
            }
            match stream.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(read) => inbox.extend_from_slice(&chunk[..read]),
            }
        }
        let _ = std::fs::remove_dir_all(dir);
        requests
    });
    (path, handle)
}

fn announce(registry: u32, globals: &[Global]) -> Vec<u8> {
    let mut out = Vec::new();
    for global in globals {
        out.extend(
            Message::new(registry, 0)
                .uint(global.name)
                .string(&global.interface)
                .uint(global.version)
                .encode()
                .unwrap(),
        );
    }
    out
}

#[test]
fn a_missing_input_protocol_refuses_before_binding_anything() {
    let (path, server) = fake_compositor(|registry, callback| {
        let mut globals = full_globals();
        globals.retain(|global| global.interface != "zwp_virtual_keyboard_manager_v1");
        let mut out = announce(registry, &globals);
        out.extend(Message::new(callback, 0).uint(0).encode().unwrap());
        out
    });
    let result = WaylandSeat::connect(&path, Duration::ZERO);
    assert!(matches!(result, Err(SeatError::Globals(ref reason)) if reason.contains("keyboard")));
    drop(result);
    let requests = server.join().unwrap();
    assert_eq!(
        requests,
        vec![(1, 1), (1, 0)],
        "no bind, no device creation"
    );
}

#[test]
fn a_compositor_error_surfaces_with_its_reason() {
    let (path, server) = fake_compositor(|_, _| {
        Message::new(1, 0)
            .uint(2)
            .uint(3)
            .string("no such global")
            .encode()
            .unwrap()
    });
    let result = WaylandSeat::connect(&path, Duration::ZERO);
    assert!(matches!(
        result,
        Err(SeatError::Compositor { object: 2, code: 3, ref message }) if message == "no such global"
    ));
    drop(result);
    server.join().unwrap();
}

#[test]
fn step_delay_is_off_unless_the_probe_sets_a_bounded_value() {
    assert_eq!(super::step_delay(None), Duration::ZERO);
    assert_eq!(super::step_delay(Some("")), Duration::ZERO);
    assert_eq!(super::step_delay(Some("0")), Duration::ZERO);
    assert_eq!(super::step_delay(Some("1001")), Duration::ZERO);
    assert_eq!(super::step_delay(Some("-1")), Duration::ZERO);
    assert_eq!(super::step_delay(Some("nope")), Duration::ZERO);
    assert_eq!(super::step_delay(Some("200")), Duration::from_millis(200));
    assert_eq!(super::step_delay(Some("1")), Duration::from_millis(1));
    assert_eq!(super::step_delay(Some("1000")), Duration::from_millis(1000));
}

#[test]
fn an_absent_socket_is_no_display() {
    let missing = std::env::temp_dir().join("pawork-cu26-absent/wayland-0");
    assert!(matches!(
        WaylandSeat::connect(&missing, Duration::ZERO),
        Err(SeatError::NoDisplay(_))
    ));
}
