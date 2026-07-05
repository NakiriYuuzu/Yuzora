// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // askpass client 模式：僅由 Yuzora spawn git 時注入的 env 觸發，正常啟動不會帶。
    if let Ok(endpoint) = std::env::var("YUZORA_ASKPASS_ENDPOINT") {
        if let Some(prompt) = std::env::args().nth(1) {
            std::process::exit(yuzora_lib::askpass::run_client(&endpoint, &prompt));
        }
    }
    yuzora_lib::run()
}
