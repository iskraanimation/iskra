//! On Windows, the app's icon goes into `iskra.exe`, with its name and version under
//! *Properties ▸ Details*: Explorer, the taskbar and shortcuts show the file's own icon, not
//! the window's. Elsewhere there is nothing to do here; the window is given the icon when it
//! is made, and on macOS a bundle would carry its own.

fn main() {
    println!("cargo:rerun-if-changed=assets/iskra.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon("assets/iskra.ico").set("ProductName", "Iskra").set("FileDescription", "Iskra, a 2D animation app");
        // Without the Windows resource compiler the app is still built, only with no icon.
        if let Err(why) = resource.compile() {
            println!("cargo:warning=the icon could not be put into iskra.exe: {why}");
        }
    }
}
