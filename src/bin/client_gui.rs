#![cfg_attr(windows, windows_subsystem = "windows")]
#![allow(non_snake_case)]
fn main() {
    #[cfg(windows)]
    usbbridge::launcher::main(usbbridge::desktop::Role::Client);
    #[cfg(not(windows))]
    {
        eprintln!("USB to Network Bridge desktop launchers require Windows");
        std::process::exit(1);
    }
}
