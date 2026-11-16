fn main() {
    println!("cargo:rerun-if-changed=brisk.rc");
    println!("cargo:rerun-if-changed=../../assets/brisk.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("brisk.rc", embed_resource::NONE)
            .manifest_optional()
            .unwrap();
    }
}
