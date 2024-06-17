use std::env;

pub fn setup_cli() {
    let args: Vec<String> = env::args().collect();
    dbg!(args);
    let path = dirs::home_dir().unwrap().as_path().display().to_string();
    dbg!(path);
    print!("Hello, what's your name? ");
}