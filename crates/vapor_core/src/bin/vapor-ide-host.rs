fn main() {
    if let Err(error) = vapor_core::ide_host::run_stdio() {
        eprintln!("vapor-ide-host: {error}");
        std::process::exit(1);
    }
}
