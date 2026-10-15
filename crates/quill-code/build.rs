fn main() {
    println!("cargo:rerun-if-changed=../../assets/brand/app.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "windows") {
        winresource::WindowsResource::new().set_icon("../../assets/brand/app.ico").compile().expect("embed the app icon");
    }
}