// checks / report(自检、诊断包)依赖 tools 模块,health_probe 只由桌面 setup 启动,
// cost_alert 依赖 tauri(系统通知 + 桌宠气泡):都仅桌面构建编译。
// speedtest / test_failure 被网关与模型层使用,cli headless 也需要。
#[cfg(feature = "desktop")]
pub mod checks;
#[cfg(feature = "desktop")]
pub mod cost_alert;
#[cfg(feature = "desktop")]
pub mod health_probe;
#[cfg(feature = "desktop")]
pub mod report;
pub mod speedtest;
pub mod test_failure;
