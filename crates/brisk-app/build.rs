fn main() {
    println!("cargo:rerun-if-changed=brisk.rc");
    println!("cargo:rerun-if-changed=../../assets/brisk.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        // The version info comes from Cargo.toml: brisk.rc holds placeholders.
        let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
        let mut nums: Vec<String> = version.split(['.', '-']).take(3).map(|s| s.parse::<u32>().unwrap_or(0).to_string()).collect();
        nums.resize(4, "0".into());
        let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default().replace('\\', "/");
        let rc = std::fs::read_to_string("brisk.rc")
            .expect("brisk.rc")
            .replace("@VERSION_COMMAS@", &nums.join(","))
            .replace("@VERSION@", &version)
            .replace("@ICON@", &format!("{manifest}/../../assets/brisk.ico"));
        let out = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR")).join("brisk.rc");
        std::fs::write(&out, rc).expect("write brisk.rc");
        embed_resource::compile(&out, embed_resource::NONE).manifest_optional().unwrap();
    }
}
