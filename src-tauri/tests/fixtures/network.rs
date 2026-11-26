fn main() {
    let args: Vec<_> = std::env::args_os().collect();
    if let Some(result) = sing_shell::session::tunnel::dispatch(&args) {
        if let Err(error) = result {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return;
    }
    let result = sing_shell::session::tunnel::fixture(&args);
    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
