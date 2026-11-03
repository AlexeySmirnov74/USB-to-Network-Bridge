//! Console entry point; intentionally independent of the tray and transport lifetimes.
fn main() {
    #[cfg(windows)]
    let result = usbbridge::control::run(std::env::args().skip(1).collect());
    #[cfg(not(windows))]
    let result: std::io::Result<()> =
        Err(std::io::Error::other("Desktop control requires Windows"));
    if let Err(error) = result {
        eprintln!("usbbridge: {error}");
        std::process::exit(1);
    }
}
