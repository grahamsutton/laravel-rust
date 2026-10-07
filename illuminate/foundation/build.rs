//! Record the compiler version for `artisan about`.

fn main() {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
    let version = std::process::Command::new(rustc)
        .arg("--version")
        .output()
        .ok()
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .and_then(|output| output.split_whitespace().nth(1).map(str::to_string))
        .unwrap_or_default();
    println!("cargo:rustc-env=LARAVEL_RUSTC_VERSION={version}");
    println!("cargo:rerun-if-env-changed=RUSTC");
}
