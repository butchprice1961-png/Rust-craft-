//! Embeds the icon and version metadata into the Windows executable.

fn main() {
    println!("cargo:rerun-if-changed=../../packaging/icons/rustcraft.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../../packaging/icons/rustcraft.ico")
            .set("ProductName", "RustCraft")
            .set("FileDescription", "RustCraft launcher")
            .set("LegalCopyright", "RustCraft contributors. Not affiliated with Mojang or Microsoft.");
        if let Err(e) = res.compile() {
            println!("cargo:warning=could not embed Windows resources: {e}");
        }
    }
}
