fn main() {
    println!("cargo:rerun-if-changed=native/process_owner.c");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        cc::Build::new()
            .file("native/process_owner.c")
            .warnings(true)
            .compile("agentgate_process_owner");
    }
    // 仅 desktop feature 下生成 Tauri context;headless(cli)构建跳过,
    // 不依赖 tauri.conf.json,也不拉 GUI 上下文。
    if std::env::var("CARGO_FEATURE_DESKTOP").is_ok() {
        tauri_build::build();
    }
}
