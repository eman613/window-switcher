fn main() {
    println!("cargo:rerun-if-changed=assets/app.rc");
    println!("cargo:rerun-if-changed=assets/app.manifest");
    println!("cargo:rerun-if-changed=assets/icon.ico");

    let resource_defines: &[&str] = if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("gnu")
    {
        &["WINDOW_SWITCHER_USE_TOOLCHAIN_MANIFEST"]
    } else {
        &[]
    };
    embed_resource::compile("assets/app.rc", resource_defines)
        .manifest_required()
        .expect("Failed to compile resource file");
}
