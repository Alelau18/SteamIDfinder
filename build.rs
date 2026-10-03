fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico")
            .set("ProductName", "SteamIDfinder")
            .set("FileDescription", "SteamIDfinder: Steam ID to profile link")
            .set("OriginalFilename", "SteamIDfinder.exe");
        res.compile().expect("failed to embed Windows resources");
    }
}
