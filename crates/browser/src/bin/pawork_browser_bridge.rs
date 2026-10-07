//! `pawork_browser_bridge`: the Chrome / Edge native messaging host for
//! CU-15 external browser control. The browser spawns this binary with
//! the extension origin as argv[1]; stdout speaks native messaging and
//! the unix socket in the Pawork discovery directory serves Desktop.
#![cfg(unix)]

fn main() {
    let origin = std::env::args().nth(1);
    // The bridge identity must come from the spawning browser itself; on
    // macOS the parent process path is authoritative, elsewhere we trust
    // the pinned-origin check plus the extension's own hello report.
    // Tests and staged environments may not be a browser child; the
    // override only picks the label, it grants no extra trust.
    let browser = match std::env::var("PAWORK_BRIDGE_FORCE_BROWSER")
        .ok()
        .and_then(|value| pawork_browser::ext::BrowserKind::parse(value.trim()))
        .or_else(pawork_browser::ext::host::detect_parent_browser)
    {
        Some(browser) => browser,
        None => {
            eprintln!("pawork-bridge: parent process is not Chrome or Edge");
            std::process::exit(2);
        }
    };
    let data_dir = pawork_browser::ext::default_data_dir();
    let config = pawork_browser::ext::host::HostConfig {
        browser,
        data_dir: data_dir.clone(),
    };
    eprintln!(
        "pawork-bridge: serving {} via {}",
        browser.display(),
        data_dir.display()
    );
    if let Err(error) = pawork_browser::ext::host::serve(config, origin.as_deref()) {
        eprintln!("pawork-bridge: {error}");
        std::process::exit(1);
    }
}
