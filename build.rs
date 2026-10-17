// Embeds the RadioMaster icon into the Windows .exe (Explorer / taskbar). Never fails the build.
fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    #[cfg(target_os = "windows")]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        res.set("ProductName", "RadioMaster");
        res.set("FileDescription", "RadioMaster radio automation");
        if let Err(e) = res.compile() {
            println!("cargo:warning=could not embed the exe icon: {e}");
        }
    }
}
