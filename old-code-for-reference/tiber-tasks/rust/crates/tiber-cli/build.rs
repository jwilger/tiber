fn main() {
    println!("cargo:rerun-if-env-changed=TIBER_SOURCE_FINGERPRINT");
    let fingerprint = std::env::var("TIBER_SOURCE_FINGERPRINT")
        .unwrap_or_else(|_| "development-source-fingerprint".to_owned());
    println!("cargo:rustc-env=TIBER_SOURCE_FINGERPRINT={fingerprint}");
}
