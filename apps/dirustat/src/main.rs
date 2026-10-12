//! Desktop application. Prints a banner for now; the eframe window arrives in M1.
#![allow(clippy::print_stdout)]

use dirustat_engine::Session;

fn main() {
    println!("{}", dirustat_ui::banner(&Session::new()));
}
