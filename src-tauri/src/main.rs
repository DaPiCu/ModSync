// Kein zusätzliches Konsolenfenster unter Windows im Release-Build
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    modsync_lib::run()
}
