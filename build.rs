//! Embeds the brand's icon into the Windows exe (Explorer, pinned taskbar entries,
//! shortcuts). The window and tray icons come from `brand::ICON` at run time.

fn main() {
    let jof = std::env::var_os("CARGO_FEATURE_JOF").is_some();
    let icon = if jof {
        "assets/jof/launcher-icon.ico"
    } else {
        "assets/launcher-icon.ico"
    };
    println!("cargo:rerun-if-changed={icon}");
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon(icon);
        resource.set("ProductName", if jof { "JoF Launcher" } else { "SJK Launcher" });
        resource.set("FileDescription", if jof { "JoF Launcher" } else { "SJK Launcher" });
        if let Err(error) = resource.compile() {
            panic!("could not embed the exe icon: {error}");
        }
    }
}
