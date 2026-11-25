use std::{
    io::{self, Write},
    net::TcpListener,
    path::PathBuf,
    time::Duration,
};

fn main() {
    let args: Vec<_> = std::env::args_os().collect();
    if args.get(1).is_some_and(|arg| arg == "version") {
        println!("sing-box version 1.14.4");
        return;
    }
    let config = args
        .windows(2)
        .find(|pair| pair[0] == "-c")
        .map(|pair| PathBuf::from(&pair[1]))
        .unwrap();
    let bytes = std::fs::read(config).unwrap();
    let value: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("invalid config: {error}");
            std::process::exit(2);
        }
    };
    if value["invalid"].as_bool() == Some(true) {
        eprintln!("invalid configuration");
        std::process::exit(2);
    }
    if args.get(1).is_some_and(|arg| arg == "check") {
        if let Some(delay) = value["checkDelayMs"].as_u64() {
            std::thread::sleep(Duration::from_millis(delay));
        }
        return;
    }
    if value["exit"].as_bool() == Some(true) {
        eprintln!("fixture startup failed");
        std::process::exit(3);
    }
    if let Some(path) = value["relativeFile"].as_str() {
        assert_eq!(std::fs::read_to_string(path).unwrap(), "relative data");
    }
    let port = value["port"].as_u64().unwrap_or(0) as u16;
    let _listener = TcpListener::bind(("127.0.0.1", port)).unwrap();
    println!("fixture ready on {}", _listener.local_addr().unwrap());
    eprintln!("fixture stderr");
    println!("{}", "x".repeat(20000));
    io::stdout().flush().unwrap();
    loop {
        std::thread::sleep(Duration::from_millis(50));
    }
}
