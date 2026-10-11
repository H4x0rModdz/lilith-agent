// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // Relançado elevado pelo UAC só para operações de janela (ver elevated.rs).
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some(lilith_companion_lib::elevated::FLAG) {
        std::process::exit(lilith_companion_lib::elevated::helper_main(&args[2..]));
    }
    lilith_companion_lib::run()
}
