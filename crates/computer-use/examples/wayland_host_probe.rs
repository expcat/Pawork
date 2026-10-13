//! CU-26 slice 2 manual probe: drives `NativeHost::wayland()` against a
//! running `computer_guest --wayland` on 127.0.0.1:5907.
//!
//! Not a product path. The grant below stands in for the user's GUI
//! approval; the probe never passes a port, path or pid to the host.
//! Usage: `wayland_host_probe <bundle_id> <out_dir> <text>` with
//! `PAWORK_ISOLATED_GUEST_TOKEN` set.

use pawork_computer_use::approval::{GrantKind, TargetIdentity};
use pawork_computer_use::target::{
    AppFamily, AppIdentity, KeyModifiers, Scope, TargetedInput, TargetedKey, TargetedKeyPress,
};
use pawork_computer_use::{NativeHost, NativeOutput, NativeRequest};
use serde_json::json;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [bundle, out, text] = args.as_slice() else {
        eprintln!("usage: wayland_host_probe <bundle_id> <out_dir> <text>");
        std::process::exit(2);
    };
    let host = NativeHost::wayland();
    let scope = Scope::new("cu26-probe", "slice2");
    host.open_scope(&scope);
    let report = |stage: &str, result: &Result<NativeOutput, _>| {
        let line = match result {
            Ok(output) => json!({
                "stage": stage,
                "ok": true,
                "detail": output.detail,
                "outcome": output.outcome,
                "input_dispatched": output.input_dispatched,
                "observation": output.observation_id,
                "observation_failure": output.observation_failure,
                "jpeg_bytes": output.jpeg.as_ref().map(Vec::len),
            }),
            Err(error) => json!({"stage": stage, "ok": false, "error": format!("{error:?}")}),
        };
        println!("{line}");
    };
    let save = |name: &str, output: &NativeOutput| {
        if let Some(jpeg) = &output.jpeg {
            std::fs::write(format!("{out}/{name}.jpg"), jpeg)
                .unwrap_or_else(|error| panic!("write {name}.jpg: {error}"));
        }
    };

    let bind = |label: &str| {
        let result = host.execute(
            &scope,
            NativeRequest::Bind {
                bundle_id: bundle.clone(),
            },
            &|| false,
        );
        report(label, &result);
        result
    };
    let _ = host.execute(&scope, NativeRequest::Discover, &|| false);
    let _ = bind("bind_without_grant");
    let target = TargetIdentity::application(AppIdentity {
        bundle_id: bundle.clone(),
        family: AppFamily::Browser,
    });
    host.grant(&scope, &target, GrantKind::ForRun)
        .expect("probe grant");
    let Ok(bound) = bind("bind") else {
        std::process::exit(1);
    };
    let window = bound.window_id.expect("bind returns a window");

    let shot = host.execute(
        &scope,
        NativeRequest::Screenshot {
            window: window.clone(),
        },
        &|| false,
    );
    report("screenshot", &shot);
    let Ok(shot) = shot else {
        std::process::exit(1);
    };
    save("before", &shot);
    let first = shot.observation_id.clone().expect("lease");

    let (width, height) = image_size(shot.jpeg.as_deref().unwrap_or_default());
    let clicked = host.execute(
        &scope,
        NativeRequest::ObservedClick {
            observation: first.clone(),
            x: f64::from(width) / 2.0,
            y: f64::from(height) / 2.0,
        },
        &|| false,
    );
    report("click_center", &clicked);
    let Ok(clicked) = clicked else {
        std::process::exit(1);
    };
    save("after_click", &clicked);

    let stale = host.execute(
        &scope,
        NativeRequest::ObservedKeys {
            observation: first,
            input: TargetedInput::InsertText("stale".into()),
        },
        &|| false,
    );
    report("stale_lease", &stale);

    let typed = host.execute(
        &scope,
        NativeRequest::ObservedKeys {
            observation: clicked.observation_id.clone().expect("fresh lease"),
            input: TargetedInput::InsertText(text.clone()),
        },
        &|| false,
    );
    report("type_text", &typed);
    let Ok(typed) = typed else {
        std::process::exit(1);
    };
    let entered = host.execute(
        &scope,
        NativeRequest::ObservedKeys {
            observation: typed.observation_id.clone().expect("fresh lease"),
            input: TargetedInput::KeyPress(TargetedKeyPress {
                key: TargetedKey::Return,
                modifiers: KeyModifiers::none(),
            }),
        },
        &|| false,
    );
    report("return", &entered);
    if let Ok(entered) = &entered {
        save("after_return", entered);
    }

    host.revoke(&scope, &target);
    let revoked = host.execute(&scope, NativeRequest::Screenshot { window }, &|| false);
    report("screenshot_after_revoke", &revoked);
    host.close_scope(&scope);
}

fn image_size(jpeg: &[u8]) -> (u32, u32) {
    image::load_from_memory_with_format(jpeg, image::ImageFormat::Jpeg)
        .map(|image| (image.width(), image.height()))
        .unwrap_or((1, 1))
}
