fn main() {
    println!("cargo:rerun-if-changed=assets/icon/CapsSwitch.ico");

    winresource::WindowsResource::new()
        .set_icon("assets/icon/CapsSwitch.ico")
        .compile()
        .unwrap();
}
