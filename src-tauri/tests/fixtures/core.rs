use std::{
    io::{self, Write},
    net::TcpListener,
    path::PathBuf,
    time::Duration,
};

fn main() {
    let args: Vec<_> = std::env::args_os().collect();
    let executable = std::env::current_exe().unwrap();
    let version = executable
        .parent()
        .and_then(|p| p.parent())
        .and_then(|p| p.file_name())
        .and_then(|p| p.to_str())
        .filter(|v| semver::Version::parse(v).is_ok())
        .unwrap_or("1.14.4");
    if args.get(1).is_some_and(|arg| arg == "version") {
        println!("sing-box version {version}");
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
    if value["invalid"].as_bool() == Some(true)
        || value["incompatibleVersion"].as_str() == Some(version)
    {
        eprintln!("invalid configuration");
        std::process::exit(2);
    }
    if args.get(1).is_some_and(|arg| arg == "check") {
        if let Some(delay) = value["checkDelayMs"].as_u64() {
            std::thread::sleep(Duration::from_millis(delay));
        }
        return;
    }
    if value["exit"].as_bool() == Some(true) || value["failVersion"].as_str() == Some(version) {
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
    let started = std::time::Instant::now();
    loop {
        if value["exitAfterMs"]
            .as_u64()
            .is_some_and(|ms| started.elapsed() >= Duration::from_millis(ms))
        {
            std::process::exit(4);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
