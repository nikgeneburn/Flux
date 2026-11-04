#![windows_subsystem = "windows"]
#![allow(non_snake_case, non_upper_case_globals)]

mod Application;
mod Control;
mod FileSystem;
mod Interface;
mod Platform;
mod Services;

use Services::Config::{AppPaths, Settings};
use Services::Diagnostics::{self, CrashOptions};
use windows::Win32::System::Ole::{OleInitialize, OleUninitialize};
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};

fn main() {
    let Arguments: Vec<String> = std::env::args().collect();
    if Arguments.get(1).map(String::as_str) == Some(Diagnostics::MonitorArgument) {
        std::process::exit(Diagnostics::RunMonitor(&Arguments));
    }
    let Paths = AppPaths::Discover();
    let Preferences = Settings::Load(&Paths.SettingsFile());
    if let Some(Register) = Arguments.iter().find_map(|Item| match Item.as_str() {
        "--register-shell" => Some(true),
        "--unregister-shell" => Some(false),
        _ => None,
    }) {
        std::process::exit(ShellRegistration(Register));
    }
    let IsTool = Arguments.iter().any(|Item| matches!(Item.as_str(), "--benchmark" | "--generate" | "--bench-ui" | "--new-window"));
    if !IsTool && Preferences.ReuseWindow && Platform::Integration::ForwardToRunningInstance(&Platform::Integration::ForwardText(&Arguments)) {
        std::process::exit(0);
    }
    Diagnostics::Install(CrashOptions { CrashDirectory: Paths.CrashDirectory(), UseMonitor: Preferences.CrashMonitor, FullMemoryDump: Preferences.FullMemoryDump });
    if !IsTool {
        Platform::Integration::ApplyTaskbarIdentity();
    }
    if Arguments.iter().any(|Item| Item == "--benchmark") {
        std::process::exit(Services::Performance::RunBenchmark(&Arguments));
    }
    if Arguments.iter().any(|Item| Item == "--generate") {
        std::process::exit(Services::Performance::GenerateFiles(&Arguments));
    }
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        if OleInitialize(None).is_err() {
            Platform::ShowError(None, "Flux", "COM/OLE initialization failed");
            std::process::exit(1);
        }
    }
    let Code = Application::Run(&Arguments);
    unsafe { OleUninitialize() };
    std::process::exit(Code);
}

fn ShellRegistration(Register: bool) -> i32 {
    let Result = match std::env::current_exe() {
        Ok(Executable) if Register => Platform::Integration::Register(&Executable),
        Ok(_) => Platform::Integration::Unregister(),
        Err(Error) => Err(Error.to_string()),
    };
    match Result {
        Ok(()) => 0,
        Err(Message) => {
            Platform::ShowError(None, "Flux", &Message);
            1
        }
    }
}
