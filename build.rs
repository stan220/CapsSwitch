fn main() {
    println!("cargo:rerun-if-changed=assets/icon/CapsSwitch.ico");
    println!("cargo:rerun-if-changed=app.manifest");

    winresource::WindowsResource::new()
        .set_icon("assets/icon/CapsSwitch.ico")
        .set_manifest_file("app.manifest")
        .compile()
        .unwrap();
}
